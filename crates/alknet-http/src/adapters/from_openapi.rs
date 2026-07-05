//! `from_openapi` adapter: parse an OpenAPI 3.x document into
//! [`HandlerRegistration`] bundles with reqwest-backed forwarding handlers.
//!
//! The forwarding handler is the no-env-vars credential injection point
//! (ADR-014): it reads `OperationContext.capabilities`, never
//! `std::env::var`. Provenance is `FromOpenAPI` (leaf,
//! `composition_authority: None`, `scoped_env: None`, `Internal` by
//! default — ADR-015/022). Imported error codes are prefixed `HTTP_<status>`
//! to avoid collision with the protocol-level codes (ADR-023).
//!
//! See `docs/architecture/crates/http/http-adapters.md` and the TypeScript
//! prior art at `/workspace/@alkdev/operations/src/from_openapi.ts`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use alknet_call::client::{AdapterError, OperationAdapter};
use alknet_call::protocol::wire::{CallError, ResponseEnvelope};
use alknet_call::registry::context::OperationContext;
use alknet_call::registry::registration::{
    make_handler, make_streaming_handler, HandlerKind, HandlerRegistration, OperationProvenance,
    ResponseStream,
};
use alknet_call::registry::spec::{
    AccessControl, ErrorDefinition, OperationSpec, OperationType, Visibility,
};
use alknet_core::types::Capabilities;
use async_trait::async_trait;
use futures::stream;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Method;
use serde_json::Value;
use url::Url;

use crate::client::SharedHttpClient;

const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

#[derive(Clone)]
pub enum HttpAuthScheme {
    Bearer,
    ApiKey { header_name: String },
    Basic,
}

pub struct HttpServiceConfig {
    pub namespace: String,
    pub base_url: String,
    pub auth: Option<HttpAuthScheme>,
    pub default_headers: HashMap<String, String>,
}

#[derive(Debug)]
pub struct OpenAPISpec {
    pub info: OpenAPIInfo,
    pub paths: BTreeMap<String, PathItem>,
    pub components: Option<Components>,
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct OpenAPIInfo {
    pub title: String,
    pub version: String,
}

#[derive(Clone, Debug)]
pub struct PathItem {
    pub operations: Vec<(String, Operation)>,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub operation_id: Option<String>,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    pub responses: BTreeMap<String, Response>,
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub name: String,
    pub in_: String,
    pub required: bool,
    pub schema: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct RequestBody {
    pub content: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub content: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct Components {
    pub schemas: HashMap<String, Value>,
}

impl OpenAPISpec {
    pub fn from_json(doc: &str) -> Result<Self, AdapterError> {
        let raw: Value = serde_json::from_str(doc).map_err(|e| AdapterError::SchemaParse {
            message: format!("invalid JSON: {e}"),
        })?;
        Self::from_value(raw)
    }

    pub fn from_value(raw: Value) -> Result<Self, AdapterError> {
        if !raw.is_object() {
            return Err(AdapterError::SchemaParse {
                message: "OpenAPI document must be a JSON object".into(),
            });
        }

        let info_obj = raw.get("info").ok_or_else(|| AdapterError::SchemaParse {
            message: "OpenAPI document missing `info`".into(),
        })?;
        let info = OpenAPIInfo {
            title: info_obj
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            version: info_obj
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("1.0.0")
                .to_string(),
        };

        let paths_raw = raw.get("paths").ok_or_else(|| AdapterError::SchemaParse {
            message: "OpenAPI document missing `paths`".into(),
        })?;
        if !paths_raw.is_object() {
            return Err(AdapterError::SchemaParse {
                message: "`paths` must be a JSON object".into(),
            });
        }

        let mut paths = BTreeMap::new();
        for (path, item) in paths_raw.as_object().expect("paths is object") {
            if !item.is_object() {
                continue;
            }
            let mut operations = Vec::new();
            for method in HTTP_METHODS {
                if let Some(op_raw) = item.get(*method) {
                    if let Some(op) = parse_operation(op_raw) {
                        operations.push((method.to_string(), op));
                    }
                }
            }
            if operations.is_empty() {
                continue;
            }
            paths.insert(path.clone(), PathItem { operations });
        }

        let components = raw
            .get("components")
            .and_then(|c| c.get("schemas"))
            .and_then(|schemas| {
                if !schemas.is_object() {
                    return None;
                }
                let mut map = HashMap::new();
                for (k, v) in schemas.as_object().expect("schemas is object") {
                    map.insert(k.clone(), v.clone());
                }
                Some(Components { schemas: map })
            });

        Ok(Self {
            info,
            paths,
            components,
            raw,
        })
    }

    fn resolve_ref(&self, reference: &str) -> Result<Value, AdapterError> {
        if !reference.starts_with("#/") {
            return Err(AdapterError::SchemaParse {
                message: format!("external $ref not supported: {reference}"),
            });
        }
        let mut current: &Value = &self.raw;
        for part in reference.trim_start_matches("#/").split('/') {
            current = current.get(part).ok_or_else(|| AdapterError::SchemaParse {
                message: format!("cannot resolve $ref: {reference}"),
            })?;
        }
        Ok(current.clone())
    }

    fn resolve_refs_recursive(&self, schema: &Value) -> Result<Value, AdapterError> {
        match schema {
            Value::Object(obj) => {
                if let Some(Value::String(reference)) = obj.get("$ref") {
                    let resolved = self.resolve_ref(reference)?;
                    return self.resolve_refs_recursive(&resolved);
                }
                let mut out = serde_json::Map::new();
                for (k, v) in obj {
                    out.insert(k.clone(), self.resolve_refs_recursive(v)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    out.push(self.resolve_refs_recursive(v)?);
                }
                Ok(Value::Array(out))
            }
            other => Ok(other.clone()),
        }
    }
}

fn parse_operation(raw: &Value) -> Option<Operation> {
    if !raw.is_object() {
        return None;
    }
    let operation_id = raw
        .get("operationId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let parameters = raw
        .get("parameters")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?.to_string();
                    let in_ = p.get("in")?.as_str()?.to_string();
                    let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                    let schema = p.get("schema").cloned();
                    Some(Parameter {
                        name,
                        in_,
                        required,
                        schema,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let request_body = raw.get("requestBody").and_then(|rb| {
        let content_obj = rb.get("content")?.as_object()?;
        let mut content = BTreeMap::new();
        for (k, v) in content_obj {
            let schema = v.get("schema").cloned().unwrap_or(Value::Null);
            content.insert(k.clone(), schema);
        }
        Some(RequestBody { content })
    });

    let mut responses = BTreeMap::new();
    if let Some(resp_obj) = raw.get("responses").and_then(|v| v.as_object()) {
        for (code, body) in resp_obj {
            let content_obj = body.get("content").and_then(|v| v.as_object());
            let mut content = BTreeMap::new();
            if let Some(content_obj) = content_obj {
                for (k, v) in content_obj {
                    let schema = v.get("schema").cloned().unwrap_or(Value::Null);
                    content.insert(k.clone(), schema);
                }
            }
            responses.insert(code.clone(), Response { content });
        }
    }

    Some(Operation {
        operation_id,
        parameters,
        request_body,
        responses,
    })
}

pub struct FromOpenAPI {
    spec: OpenAPISpec,
    config: HttpServiceConfig,
    http_client: Arc<SharedHttpClient>,
}

impl FromOpenAPI {
    pub fn new(
        spec: OpenAPISpec,
        config: HttpServiceConfig,
        http_client: Arc<SharedHttpClient>,
    ) -> Self {
        Self {
            spec,
            config,
            http_client,
        }
    }

    fn normalize_operation_id(op: &Operation, method: &str, path: &str) -> String {
        if let Some(id) = &op.operation_id {
            return id.clone();
        }
        let parts: Vec<&str> = path
            .split('/')
            .filter(|p| !p.is_empty() && !p.starts_with('{'))
            .collect();
        let base = if parts.is_empty() {
            "root".to_string()
        } else {
            parts.join("_")
        };
        format!("{method}_{base}")
    }

    fn detect_op_type(method: &str, op: &Operation) -> OperationType {
        let success = op.responses.get("200").or_else(|| op.responses.get("201"));
        if let Some(resp) = success {
            if resp.content.contains_key("text/event-stream") {
                return OperationType::Subscription;
            }
        }
        if method.eq_ignore_ascii_case("get") {
            OperationType::Query
        } else {
            OperationType::Mutation
        }
    }

    fn build_input_schema(&self, op: &Operation) -> Result<Value, AdapterError> {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param in &op.parameters {
            let schema = match &param.schema {
                Some(s) => self.spec.resolve_refs_recursive(s)?,
                None => serde_json::json!({"type": "string"}),
            };
            properties.insert(param.name.clone(), schema);
            if param.required {
                required.push(param.name.clone());
            }
        }

        if let Some(body) = &op.request_body {
            if let Some(json_schema) = body.content.get("application/json") {
                let resolved = self.spec.resolve_refs_recursive(json_schema)?;
                properties.insert("body".to_string(), resolved);
                required.push("body".to_string());
            }
        }

        if properties.is_empty() {
            return Ok(serde_json::json!({"type": "object"}));
        }

        Ok(serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }))
    }

    fn build_output_schema(&self, op: &Operation) -> Result<Value, AdapterError> {
        let success = op.responses.get("200").or_else(|| op.responses.get("201"));
        let Some(resp) = success else {
            return Ok(serde_json::json!({}));
        };
        if let Some(json_schema) = resp.content.get("application/json") {
            return self.spec.resolve_refs_recursive(json_schema);
        }
        if let Some(sse_schema) = resp.content.get("text/event-stream") {
            return self.spec.resolve_refs_recursive(sse_schema);
        }
        Ok(serde_json::json!({}))
    }

    fn build_error_schemas(&self, op: &Operation) -> Result<Vec<ErrorDefinition>, AdapterError> {
        let mut out = Vec::new();
        for (code, resp) in &op.responses {
            let status: Option<u16> = code.parse::<u16>().ok();
            let is_2xx = matches!(status, Some(s) if (200..300).contains(&s));
            if is_2xx {
                continue;
            }
            let schema = if let Some(json_schema) = resp.content.get("application/json") {
                self.spec.resolve_refs_recursive(json_schema)?
            } else {
                serde_json::json!({})
            };
            let status_code = status.unwrap_or(0);
            out.push(ErrorDefinition {
                code: format!("HTTP_{status_code}"),
                description: format!("HTTP {status_code} response"),
                schema,
                http_status: status,
            });
        }
        Ok(out)
    }

    fn build_registration(
        &self,
        method: &str,
        path: &str,
        op: &Operation,
    ) -> Result<HandlerRegistration, AdapterError> {
        let name = Self::normalize_operation_id(op, method, path);
        let qualified_name = format!("{}/{name}", self.config.namespace);
        let op_type = Self::detect_op_type(method, op);
        let input_schema = self.build_input_schema(op)?;
        let output_schema = self.build_output_schema(op)?;
        let error_schemas = self.build_error_schemas(op)?;

        let spec = OperationSpec::new(
            qualified_name,
            op_type,
            Visibility::Internal,
            input_schema,
            output_schema,
            error_schemas,
            AccessControl::default(),
            None,
        );

        let path_template = path.to_string();
        let method_upper = method.to_ascii_uppercase();
        let auth_scheme = self.config.auth.clone();
        let default_headers = self.config.default_headers.clone();
        let base_url = self.config.base_url.clone();
        let namespace = self.config.namespace.clone();
        let http_client = Arc::clone(&self.http_client);
        let error_status_codes: Vec<(u16, String)> = spec
            .error_schemas
            .iter()
            .map(|e| (e.http_status.unwrap_or(0), e.code.clone()))
            .collect();

        let handler = if op_type == OperationType::Subscription {
            let stream_handler =
                make_streaming_handler(move |input: Value, context: OperationContext| {
                    let path_template = path_template.clone();
                    let method_upper = method_upper.clone();
                    let auth_scheme = auth_scheme.clone();
                    let default_headers = default_headers.clone();
                    let base_url = base_url.clone();
                    let namespace = namespace.clone();
                    let http_client = Arc::clone(&http_client);
                    let error_status_codes = error_status_codes.clone();
                    forward_stream(
                        &http_client,
                        &base_url,
                        &path_template,
                        &method_upper,
                        &auth_scheme,
                        &default_headers,
                        &namespace,
                        &error_status_codes,
                        input,
                        context,
                    )
                });
            HandlerKind::Stream(stream_handler)
        } else {
            let once_handler = make_handler(move |input: Value, context: OperationContext| {
                let path_template = path_template.clone();
                let method_upper = method_upper.clone();
                let auth_scheme = auth_scheme.clone();
                let default_headers = default_headers.clone();
                let base_url = base_url.clone();
                let namespace = namespace.clone();
                let http_client = Arc::clone(&http_client);
                let error_status_codes = error_status_codes.clone();
                let op_type = op_type;
                async move {
                    forward(
                        &http_client,
                        &base_url,
                        &path_template,
                        &method_upper,
                        &auth_scheme,
                        &default_headers,
                        &namespace,
                        &error_status_codes,
                        op_type,
                        input,
                        context,
                    )
                    .await
                }
            });
            HandlerKind::Once(once_handler)
        };

        let capabilities = Capabilities::new();
        Ok(HandlerRegistration::new(
            spec,
            handler,
            OperationProvenance::FromOpenAPI,
            None,
            None,
            capabilities,
        ))
    }
}

#[async_trait]
impl OperationAdapter for FromOpenAPI {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
        let mut bundles = Vec::new();
        for (path, item) in &self.spec.paths {
            for (method, op) in &item.operations {
                let registration = self.build_registration(method, path, op)?;
                bundles.push(registration);
            }
        }
        Ok(bundles)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_request(
    base_url: &str,
    path_template: &str,
    method: &str,
    auth_scheme: &Option<HttpAuthScheme>,
    default_headers: &HashMap<String, String>,
    namespace: &str,
    input: &Value,
    context: &OperationContext,
) -> Result<(Method, Url, Option<Value>, HeaderMap), CallError> {
    let input_obj = input.as_object();

    let mut url_path = path_template.to_string();
    let mut query_params: Vec<(String, String)> = Vec::new();
    let mut body: Option<Value> = None;

    if let Some(obj) = input_obj {
        for (key, value) in obj {
            let placeholder = format!("{{{key}}}");
            if url_path.contains(&placeholder) {
                let rendered = value_to_path_segment(value);
                url_path = url_path.replace(&placeholder, &rendered);
            } else if key == "body" {
                body = Some(value.clone());
            } else {
                query_params.push((key.clone(), value_to_query(value)));
            }
        }
    }

    let base = Url::parse(base_url)
        .map_err(|e| CallError::internal(format!("invalid base_url `{base_url}`: {e}")))?;
    let mut url = base
        .join(url_path.trim_start_matches('/'))
        .map_err(|e| CallError::internal(format!("invalid path `{url_path}`: {e}")))?;
    if !query_params.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &query_params {
            pairs.append_pair(k, v);
        }
    }

    let mut headers = HeaderMap::new();
    for (k, v) in default_headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::try_from(v.as_str()),
        ) {
            headers.insert(name, value);
        }
    }

    if body.is_some() {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }

    if let Some(scheme) = auth_scheme {
        if let Some(secret) = context.capabilities.get(namespace) {
            let credential = secret.expose_secret().clone();
            match scheme {
                HttpAuthScheme::Bearer => {
                    let header_value = format!("Bearer {credential}");
                    if let Ok(value) = HeaderValue::try_from(header_value) {
                        headers.insert(AUTHORIZATION, value);
                    }
                }
                HttpAuthScheme::ApiKey { header_name } => {
                    if let (Ok(name), Ok(value)) = (
                        HeaderName::try_from(header_name.as_str()),
                        HeaderValue::try_from(credential.as_str()),
                    ) {
                        headers.insert(name, value);
                    }
                }
                HttpAuthScheme::Basic => {
                    let header_value = format!("Basic {credential}");
                    if let Ok(value) = HeaderValue::try_from(header_value) {
                        headers.insert(AUTHORIZATION, value);
                    }
                }
            }
        }
    }

    let http_method = Method::from_bytes(method.as_bytes())
        .map_err(|_| CallError::internal(format!("invalid HTTP method `{method}`")))?;
    Ok((http_method, url, body, headers))
}

fn value_to_path_segment(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn value_to_query(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    http_client: &Arc<SharedHttpClient>,
    base_url: &str,
    path_template: &str,
    method: &str,
    auth_scheme: &Option<HttpAuthScheme>,
    default_headers: &HashMap<String, String>,
    namespace: &str,
    error_status_codes: &[(u16, String)],
    op_type: OperationType,
    input: Value,
    context: OperationContext,
) -> ResponseEnvelope {
    let request_id = context.request_id.clone();

    let (http_method, url, body, headers) = match build_request(
        base_url,
        path_template,
        method,
        auth_scheme,
        default_headers,
        namespace,
        &input,
        &context,
    ) {
        Ok(parts) => parts,
        Err(err) => return ResponseEnvelope::error(request_id, err),
    };

    let http_client = http_client.client();

    let request_builder = http_client
        .request(http_method, url.as_str())
        .headers(headers);

    let request_builder = if op_type == OperationType::Subscription {
        request_builder.header(ACCEPT, "text/event-stream")
    } else {
        request_builder.header(ACCEPT, "*/*")
    };

    let request_builder = match body.as_ref() {
        Some(b) => {
            let serialized = serde_json::to_string(b).unwrap_or_else(|_| String::from("null"));
            request_builder.body(serialized)
        }
        None => request_builder,
    };

    let response: reqwest::Response = match request_builder.send().await {
        Ok(r) => r,
        Err(err) => {
            return ResponseEnvelope::error(
                request_id,
                CallError::internal(format!("HTTP request failed: {err}")),
            );
        }
    };

    let status = response.status();

    if !status.is_success() {
        let code = error_status_codes
            .iter()
            .find(|(s, _)| *s == status.as_u16())
            .map(|(_, code)| code.clone())
            .unwrap_or_else(|| format!("HTTP_{}", status.as_u16()));
        let message = format!(
            "HTTP {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        return ResponseEnvelope::error(request_id, CallError::new(code, message, false));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("application/json") {
        match response.json::<Value>().await {
            Ok(v) => ResponseEnvelope::ok(request_id, v),
            Err(err) => ResponseEnvelope::error(
                request_id,
                CallError::internal(format!("failed to decode JSON body: {err}")),
            ),
        }
    } else if content_type.starts_with("text/") {
        match response.text().await {
            Ok(t) => ResponseEnvelope::ok(request_id, Value::String(t)),
            Err(err) => ResponseEnvelope::error(
                request_id,
                CallError::internal(format!("failed to decode text body: {err}")),
            ),
        }
    } else {
        match response.bytes().await {
            Ok(b) => {
                let arr: Vec<Value> = b.iter().map(|byte| Value::Number((*byte).into())).collect();
                ResponseEnvelope::ok(request_id, Value::Array(arr))
            }
            Err(err) => ResponseEnvelope::error(
                request_id,
                CallError::internal(format!("failed to read body: {err}")),
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn forward_stream(
    http_client: &Arc<SharedHttpClient>,
    base_url: &str,
    path_template: &str,
    method: &str,
    auth_scheme: &Option<HttpAuthScheme>,
    default_headers: &HashMap<String, String>,
    namespace: &str,
    error_status_codes: &[(u16, String)],
    input: Value,
    context: OperationContext,
) -> ResponseStream {
    let request_id = context.request_id.clone();

    let (http_method, url, body, headers) = match build_request(
        base_url,
        path_template,
        method,
        auth_scheme,
        default_headers,
        namespace,
        &input,
        &context,
    ) {
        Ok(parts) => parts,
        Err(err) => {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::error(request_id, err)
            }));
        }
    };

    let http_client = Arc::clone(http_client);
    let error_status_codes = error_status_codes.to_vec();

    let request_id_stream = request_id.clone();
    let error_status_codes_stream = error_status_codes.clone();

    let init = async move {
        let request_builder = http_client
            .client()
            .request(http_method, url.as_str())
            .headers(headers)
            .header(ACCEPT, "text/event-stream");
        let request_builder = match body.as_ref() {
            Some(b) => {
                let serialized = serde_json::to_string(b).unwrap_or_else(|_| String::from("null"));
                request_builder.body(serialized)
            }
            None => request_builder,
        };
        request_builder.send().await
    };

    let sse = stream::once(init).flat_map(move |result| {
        let request_id = request_id_stream.clone();
        let error_status_codes = error_status_codes_stream.clone();
        match result {
            Err(err) => Box::pin(stream::once(async move {
                ResponseEnvelope::error(
                    request_id,
                    CallError::internal(format!("HTTP request failed: {err}")),
                )
            })) as ResponseStream,
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    let code = error_status_codes
                        .iter()
                        .find(|(s, _)| *s == status.as_u16())
                        .map(|(_, c)| c.clone())
                        .unwrap_or_else(|| format!("HTTP_{}", status.as_u16()));
                    let message = format!(
                        "HTTP {}: {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    );
                    Box::pin(stream::once(async move {
                        ResponseEnvelope::error(request_id, CallError::new(code, message, false))
                    })) as ResponseStream
                } else {
                    let request_id_inner = request_id.clone();
                    Box::pin(
                        stream::unfold(
                            (response.bytes_stream(), String::new()),
                            move |(mut bytes, mut buffer)| {
                                let request_id = request_id_inner.clone();
                                async move {
                                    match bytes.next().await {
                                        Some(Ok(chunk)) => {
                                            buffer.push_str(&String::from_utf8_lossy(&chunk));
                                            let (events, remaining) = parse_sse_frames(&buffer);
                                            let envelopes: Vec<ResponseEnvelope> = events
                                                .into_iter()
                                                .map(|e| {
                                                    let parsed = if e.data.trim().is_empty() {
                                                        Value::Null
                                                    } else {
                                                        serde_json::from_str(&e.data).unwrap_or(
                                                            Value::String(e.data.clone()),
                                                        )
                                                    };
                                                    ResponseEnvelope::ok(&request_id, parsed)
                                                })
                                                .collect();
                                            Some((envelopes, (bytes, remaining)))
                                        }
                                        Some(Err(err)) => {
                                            let error = CallError::internal(format!(
                                                "SSE stream error: {err}"
                                            ));
                                            Some((
                                                vec![ResponseEnvelope::error(request_id, error)],
                                                (bytes, buffer),
                                            ))
                                        }
                                        None => None,
                                    }
                                }
                            },
                        )
                        .flat_map(stream::iter),
                    ) as ResponseStream
                }
            }
        }
    });

    Box::pin(sse)
}

struct SseEvent {
    data: String,
}

fn parse_sse_frames(buffer: &str) -> (Vec<SseEvent>, String) {
    let mut events = Vec::new();
    let text = if let Some(stripped) = buffer.strip_prefix('\u{feff}') {
        stripped
    } else {
        buffer
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let mut data_buffer: Vec<String> = Vec::new();
    let mut remaining = String::new();

    for (i, line) in lines.iter().enumerate() {
        if i == lines.len() - 1 {
            remaining = line.to_string();
            break;
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !data_buffer.is_empty() {
                events.push(SseEvent {
                    data: data_buffer.join("\n"),
                });
            }
            data_buffer.clear();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some((field, value)) = line.split_once(':') {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if field == "data" {
                data_buffer.push(value.to_string());
            }
        } else if line == "data" {
            data_buffer.push(String::new());
        }
    }

    (events, remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::HttpClientConfig;
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn noop_context(request_id: &str, capabilities: Capabilities) -> OperationContext {
        struct NoopEnv;
        #[async_trait]
        impl alknet_call::registry::env::OperationEnv for NoopEnv {
            async fn invoke_with_policy(
                &self,
                _ns: &str,
                _op: &str,
                _input: Value,
                parent: &OperationContext,
                _policy: alknet_call::registry::context::AbortPolicy,
            ) -> ResponseEnvelope {
                ResponseEnvelope::ok(parent.request_id.clone(), Value::Null)
            }
            fn contains(&self, _name: &str) -> bool {
                false
            }
        }
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities,
            metadata: HashMap::new(),
            scoped_env: alknet_call::registry::context::ScopedPeerEnv::empty(),
            env: Arc::new(NoopEnv),
            abort_policy: alknet_call::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: true,
            ownership: None,
        }
    }

    fn config(namespace: &str, base_url: &str, auth: Option<HttpAuthScheme>) -> HttpServiceConfig {
        HttpServiceConfig {
            namespace: namespace.to_string(),
            base_url: base_url.to_string(),
            auth,
            default_headers: HashMap::new(),
        }
    }

    fn test_http_client() -> Arc<SharedHttpClient> {
        Arc::new(SharedHttpClient::new(HttpClientConfig::default()).unwrap())
    }

    fn adapter(spec: OpenAPISpec, config: HttpServiceConfig) -> FromOpenAPI {
        FromOpenAPI::new(spec, config, test_http_client())
    }

    fn minimal_spec_json() -> &'static str {
        r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/widgets": {
                    "get": {
                        "operationId": "listWidgets",
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": { "type": "array", "items": { "type": "string" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#
    }

    #[tokio::test]
    async fn import_minimal_doc_yields_one_registration() {
        let spec = OpenAPISpec::from_json(minimal_spec_json()).unwrap();
        let adapter = adapter(spec, config("widgets", "https://api.example.com", None));
        let bundles = adapter.import().await.unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.name, "widgets/listWidgets");
        assert_eq!(bundles[0].spec.namespace, "widgets");
        assert_eq!(bundles[0].spec.op_type, OperationType::Query);
        assert_eq!(bundles[0].spec.visibility, Visibility::Internal);
        assert_eq!(bundles[0].provenance, OperationProvenance::FromOpenAPI);
        assert!(bundles[0].composition_authority.is_none());
        assert!(bundles[0].scoped_env.is_none());
    }

    #[tokio::test]
    async fn parse_failure_returns_schema_parse() {
        let result = OpenAPISpec::from_json("not json");
        assert!(matches!(result, Err(AdapterError::SchemaParse { .. })));
    }

    #[tokio::test]
    async fn missing_paths_returns_schema_parse() {
        let result = OpenAPISpec::from_json(r#"{"info":{"title":"x","version":"1"}}"#);
        match result {
            Err(AdapterError::SchemaParse { message }) => assert!(message.contains("paths")),
            other => panic!("expected SchemaParse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn generated_operation_id_when_absent() {
        let doc = r#"{
            "openapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "paths": { "/users/{id}/posts": { "get": { "responses": { "200": { "content": { "application/json": { "schema": {} } } } } } } }
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let adapter = adapter(spec, config("svc", "https://api.example.com", None));
        let bundles = adapter.import().await.unwrap();
        assert_eq!(bundles[0].spec.name, "svc/get_users_posts");
    }

    #[tokio::test]
    async fn op_type_detection() {
        let get_doc = r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{"/g":{"get":{"operationId":"g","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}}"#;
        let post_doc = r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{"/p":{"post":{"operationId":"p","responses":{"201":{"content":{"application/json":{"schema":{}}}}}}}}}"#;
        let sse_doc = r##"{"openapi":"3.0.0","info":{"title":"T","version":"1"},"paths":{"/s":{"post":{"operationId":"s","responses":{"200":{"content":{"text/event-stream":{"schema":{}}}}}}}}}"##;

        let spec = OpenAPISpec::from_json(get_doc).unwrap();
        assert_eq!(
            adapter(spec, config("ns", "https://x", None))
                .import()
                .await
                .unwrap()[0]
                .spec
                .op_type,
            OperationType::Query
        );
        let spec = OpenAPISpec::from_json(post_doc).unwrap();
        assert_eq!(
            adapter(spec, config("ns", "https://x", None))
                .import()
                .await
                .unwrap()[0]
                .spec
                .op_type,
            OperationType::Mutation
        );
        let spec = OpenAPISpec::from_json(sse_doc).unwrap();
        assert_eq!(
            adapter(spec, config("ns", "https://x", None))
                .import()
                .await
                .unwrap()[0]
                .spec
                .op_type,
            OperationType::Subscription
        );
    }

    #[tokio::test]
    async fn error_response_becomes_http_status_definition() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/x":{"get":{"operationId":"x","responses":{
                "200":{"content":{"application/json":{"schema":{}}}},
                "404":{"content":{"application/json":{"schema":{"type":"object","properties":{"msg":{"type":"string"}}}}}}
            }}}}
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("ns", "https://x", None))
            .import()
            .await
            .unwrap();
        let errors = &bundles[0].spec.error_schemas;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "HTTP_404");
        assert_eq!(errors[0].http_status, Some(404));
    }

    #[tokio::test]
    async fn input_schema_from_params_and_body() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/u/{id}":{"get":{
                "operationId":"u",
                "parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}},{"name":"q","in":"query","schema":{"type":"string"}}],
                "responses":{"200":{"content":{"application/json":{"schema":{}}}}}
            }}}
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("ns", "https://x", None))
            .import()
            .await
            .unwrap();
        let schema = &bundles[0].spec.input_schema;
        let props = schema.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("id"));
        assert!(props.contains_key("q"));
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v == "id"));
    }

    #[tokio::test]
    async fn ref_resolution_in_input_schema() {
        let doc = r##"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "components":{"schemas":{"Widget":{"type":"object","properties":{"name":{"type":"string"}}}}},
            "paths":{"/w":{"post":{
                "operationId":"w",
                "requestBody":{"content":{"application/json":{"schema":{"$ref":"#/components/schemas/Widget"}}}},
                "responses":{"200":{"content":{"application/json":{"schema":{}}}}}
            }}}
        }"##;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("ns", "https://x", None))
            .import()
            .await
            .unwrap();
        let props = bundles[0]
            .spec
            .input_schema
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap();
        let body = props.get("body").unwrap();
        assert_eq!(body.get("type").unwrap(), "object");
        assert!(body
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("name"));
    }

    #[tokio::test]
    async fn build_request_injects_bearer_from_capabilities() {
        let caps = Capabilities::new().with_http_token("github", "tok-123".to_string());
        let ctx = noop_context("req-1", caps);
        let (method, url, body, headers) = build_request(
            "https://api.github.com",
            "/repos/{owner}/{repo}/issues",
            "GET",
            &Some(HttpAuthScheme::Bearer),
            &HashMap::new(),
            "github",
            &serde_json::json!({"owner":"a","repo":"b"}),
            &ctx,
        )
        .unwrap();
        assert_eq!(method, Method::GET);
        assert_eq!(url.path(), "/repos/a/b/issues");
        assert_eq!(url.host_str(), Some("api.github.com"));
        assert!(headers.get(AUTHORIZATION).is_none() || body.is_none());
        let auth = headers.get(AUTHORIZATION).unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer tok-123");
    }

    #[tokio::test]
    async fn build_request_api_key_header_from_capabilities() {
        let caps = Capabilities::new().with_api_key("vastai", "key-xyz".to_string());
        let ctx = noop_context("req-2", caps);
        let (_, _, _, headers) = build_request(
            "https://api.vast.ai",
            "/machines",
            "GET",
            &Some(HttpAuthScheme::ApiKey {
                header_name: "X-API-Key".to_string(),
            }),
            &HashMap::new(),
            "vastai",
            &serde_json::json!({}),
            &ctx,
        )
        .unwrap();
        assert_eq!(
            headers.get("X-API-Key").unwrap().to_str().unwrap(),
            "key-xyz"
        );
    }

    #[tokio::test]
    async fn build_request_path_and_query_split() {
        let ctx = noop_context("req-3", Capabilities::new());
        let (_, url, _, _) = build_request(
            "https://api.example.com",
            "/widgets/{id}",
            "GET",
            &None,
            &HashMap::new(),
            "svc",
            &serde_json::json!({"id":42,"filter":"active"}),
            &ctx,
        )
        .unwrap();
        assert_eq!(url.path(), "/widgets/42");
        assert_eq!(url.query().unwrap(), "filter=active");
    }

    async fn spawn_echo_server(
        status: u16,
        body: &'static str,
        content_type: &'static str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let status_line = match status {
                    200 => "200 OK",
                    201 => "201 Created",
                    404 => "404 Not Found",
                    500 => "500 Internal Server Error",
                    _ => "200 OK",
                };
                let body_bytes = body.as_bytes();
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body_bytes.len(),
                    body
                );
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                sock.write_all(response.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn integration_forwarding_handler_calls_external_endpoint() {
        let base = spawn_echo_server(200, r#"{"ok":true}"#, "application/json").await;
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/data":{"get":{"operationId":"data","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-10", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        assert_eq!(response.request_id, "req-10");
        match response.result {
            Ok(v) => assert_eq!(v, serde_json::json!({"ok":true})),
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn integration_non_2xx_returns_declared_error() {
        let base = spawn_echo_server(404, r#"{"error":"missing"}"#, "application/json").await;
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/missing":{"get":{"operationId":"missing","responses":{
                "200":{"content":{"application/json":{"schema":{}}}},
                "404":{"content":{"application/json":{"schema":{"type":"object"}}}}
            }}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-11", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "HTTP_404");
                assert!(!e.retryable);
            }
            other => panic!("expected HTTP_404 error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscription_op_registration_is_handler_kind_stream() {
        let spec = OpenAPISpec::from_json(
            r##"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/stream":{"post":{"operationId":"stream","responses":{"200":{"content":{"text/event-stream":{"schema":{}}}}}}}}}"##,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", "https://x", None))
            .import()
            .await
            .unwrap();
        assert!(matches!(bundles[0].handler, HandlerKind::Stream(_)));
    }

    #[tokio::test]
    async fn query_op_registration_is_handler_kind_once() {
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/data":{"get":{"operationId":"data","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", "https://x", None))
            .import()
            .await
            .unwrap();
        assert!(matches!(bundles[0].handler, HandlerKind::Once(_)));
    }

    #[tokio::test]
    async fn integration_sse_subscription_streams_responded_events() {
        let sse_body = "data: {\"n\":1}\n\ndata: {\"n\":2}\n\n";
        let base = spawn_echo_server(200, sse_body, "text/event-stream").await;
        let spec = OpenAPISpec::from_json(
            r##"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/stream":{"post":{"operationId":"stream","responses":{"200":{"content":{"text/event-stream":{"schema":{}}}}}}}}}"##,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-12", Capabilities::new());
        let stream = match &registration.handler {
            HandlerKind::Stream(h) => h(serde_json::json!({}), ctx),
            _ => panic!("expected Stream handler"),
        };
        let collected: Vec<ResponseEnvelope> = stream.collect().await;
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].result, Ok(serde_json::json!({"n":1})));
        assert_eq!(collected[1].result, Ok(serde_json::json!({"n":2})));
        assert_eq!(collected[0].request_id, "req-12");
        assert_eq!(collected[1].request_id, "req-12");
    }

    #[tokio::test]
    async fn integration_sse_subscription_http_error_returns_single_error_envelope() {
        let base = spawn_echo_server(404, r#"{"error":"missing"}"#, "application/json").await;
        let spec = OpenAPISpec::from_json(
            r##"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/stream":{"post":{"operationId":"stream","responses":{
                "200":{"content":{"text/event-stream":{"schema":{}}}},
                "404":{"content":{"application/json":{"schema":{"type":"object"}}}}
            }}}}}"##,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-err", Capabilities::new());
        let stream = match &registration.handler {
            HandlerKind::Stream(h) => h(serde_json::json!({}), ctx),
            _ => panic!("expected Stream handler"),
        };
        let collected: Vec<ResponseEnvelope> = stream.collect().await;
        assert_eq!(collected.len(), 1);
        match &collected[0].result {
            Err(e) => assert_eq!(e.code, "HTTP_404"),
            other => panic!("expected HTTP_404 error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn integration_query_forwarding_unchanged_single_response() {
        let base = spawn_echo_server(200, r#"{"ok":true}"#, "application/json").await;
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/data":{"get":{"operationId":"data","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-q", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        assert_eq!(response.request_id, "req-q");
        assert_eq!(response.result, Ok(serde_json::json!({"ok":true})));
    }

    #[test]
    fn no_env_vars_read_in_build_request() {
        std::env::set_var("OPENAI_API_KEY", "should-not-be-used");
        let ctx = noop_context("req-13", Capabilities::new());
        let (_, _, _, headers) = build_request(
            "https://api.openai.com",
            "/v1/chat",
            "POST",
            &Some(HttpAuthScheme::Bearer),
            &HashMap::new(),
            "openai",
            &serde_json::json!({"body":{"prompt":"hi"}}),
            &ctx,
        )
        .unwrap();
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "no auth header when capabilities absent"
        );
        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn sse_frames_parse_multi_event_buffer() {
        let (events, remaining) = parse_sse_frames("data: a\n\ndata: b\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
        assert_eq!(remaining, "");
    }

    #[test]
    fn sse_frames_handle_partial_trailing_line() {
        let (events, remaining) = parse_sse_frames("data: a\n\ndata: par");
        assert_eq!(events.len(), 1);
        assert_eq!(remaining, "data: par");
    }

    #[test]
    fn sse_frames_skip_comment_lines() {
        let (events, _) = parse_sse_frames(": comment\ndata: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn sse_frames_join_multi_line_data() {
        let (events, _) = parse_sse_frames("data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn parse_sse_frames_strips_bom() {
        let (events, _) = parse_sse_frames("\u{feff}data: a\n\n");
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn http_service_config_struct_fields() {
        let cfg = config(
            "ns",
            "https://api.example.com",
            Some(HttpAuthScheme::Bearer),
        );
        assert_eq!(cfg.namespace, "ns");
        assert_eq!(cfg.base_url, "https://api.example.com");
        assert!(matches!(cfg.auth, Some(HttpAuthScheme::Bearer)));
    }

    #[test]
    fn openapi_info_parsed_from_doc() {
        let spec = OpenAPISpec::from_json(minimal_spec_json()).unwrap();
        assert_eq!(spec.info.title, "Test");
        assert_eq!(spec.info.version, "1.0.0");
    }

    #[test]
    fn openapi_components_parsed_when_present() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "components":{"schemas":{"Foo":{"type":"object"}}},
            "paths":{"/x":{"get":{"operationId":"x","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        assert!(spec.components.is_some());
        assert!(spec
            .components
            .as_ref()
            .unwrap()
            .schemas
            .contains_key("Foo"));
    }

    #[tokio::test]
    async fn import_multiple_operations() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{
                "/a":{"get":{"operationId":"getA","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}},
                "/b":{"post":{"operationId":"postB","responses":{"201":{"content":{"application/json":{"schema":{}}}}}}}
            }
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("svc", "https://x", None))
            .import()
            .await
            .unwrap();
        assert_eq!(bundles.len(), 2);
        assert_eq!(bundles[0].spec.name, "svc/getA");
        assert_eq!(bundles[1].spec.name, "svc/postB");
    }

    #[tokio::test]
    async fn basic_auth_injection_from_capabilities() {
        let caps = Capabilities::new().with_http_token("svc", "dXNlcjpwYXNz".to_string());
        let ctx = noop_context("req-14", caps);
        let (_, _, _, headers) = build_request(
            "https://api.example.com",
            "/x",
            "GET",
            &Some(HttpAuthScheme::Basic),
            &HashMap::new(),
            "svc",
            &serde_json::json!({}),
            &ctx,
        )
        .unwrap();
        assert_eq!(
            headers.get(AUTHORIZATION).unwrap().to_str().unwrap(),
            "Basic dXNlcjpwYXNz"
        );
    }

    #[test]
    fn resolve_ref_rejects_external_refs() {
        let spec = OpenAPISpec::from_json(minimal_spec_json()).unwrap();
        let err = spec.resolve_ref("https://other/file.json").unwrap_err();
        assert!(matches!(err, AdapterError::SchemaParse { .. }));
    }

    #[tokio::test]
    async fn resolve_ref_missing_target_returns_schema_parse() {
        let spec = OpenAPISpec::from_json(minimal_spec_json()).unwrap();
        let err = spec
            .resolve_ref("#/components/schemas/Missing")
            .unwrap_err();
        assert!(matches!(err, AdapterError::SchemaParse { .. }));
    }

    #[test]
    fn default_headers_applied_to_request() {
        let ctx = noop_context("req-15", Capabilities::new());
        let mut defaults = HashMap::new();
        defaults.insert("X-Trace".to_string(), "abc".to_string());
        let (_, _, _, headers) = build_request(
            "https://api.example.com",
            "/x",
            "GET",
            &None,
            &defaults,
            "svc",
            &serde_json::json!({}),
            &ctx,
        )
        .unwrap();
        assert_eq!(headers.get("X-Trace").unwrap().to_str().unwrap(), "abc");
    }

    #[derive(Deserialize)]
    struct CapturedRequest {
        method: String,
        path: String,
        query: String,
        headers: HashMap<String, String>,
        body: String,
    }

    async fn spawn_capturing_server() -> (String, tokio::sync::oneshot::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let mut lines = raw.split("\r\n");
            let request_line = lines.next().unwrap_or("");
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let raw_path = parts.next().unwrap_or("");
            let (path, query) = match raw_path.split_once('?') {
                Some((p, q)) => (p.to_string(), q.to_string()),
                None => (raw_path.to_string(), String::new()),
            };
            let mut headers = HashMap::new();
            for line in lines.by_ref() {
                if line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.to_lowercase(), v.trim().to_string());
                }
            }
            let body = lines.collect::<Vec<_>>().join("\r\n");
            let _ = tx.send(CapturedRequest {
                method,
                path,
                query,
                headers,
                body,
            });
            let response =
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn integration_forwarding_handler_sends_body_and_query() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/items/{id}":{"post":{
                "operationId":"updateItem",
                "parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],
                "requestBody":{"content":{"application/json":{"schema":{"type":"object"}}}},
                "responses":{"200":{"content":{"application/json":{"schema":{}}}}}
            }}}
        }"#;
        let (base, rx) = spawn_capturing_server().await;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-16", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => {
                h(
                    serde_json::json!({"id":"42","filter":"new","body":{"name":"widget"}}),
                    ctx,
                )
                .await
            }
            _ => panic!("expected Once handler"),
        };
        assert!(
            response.result.is_ok(),
            "expected Ok, got {:?}",
            response.result
        );
        let captured = rx.await.unwrap();
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/items/42");
        assert_eq!(captured.query, "filter=new");
        assert_eq!(
            captured.headers.get("content-type").unwrap(),
            "application/json"
        );
        assert!(captured.body.contains("\"name\":\"widget\""));
    }

    #[tokio::test]
    async fn integration_bearer_token_injected_on_outbound_request() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/me":{"get":{"operationId":"me","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}
        }"#;
        let (base, rx) = spawn_capturing_server().await;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("openai", &base, Some(HttpAuthScheme::Bearer)))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let caps = Capabilities::new().with_http_token("openai", "sk-test-token".to_string());
        let ctx = noop_context("req-17", caps);
        let _ = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        let captured = rx.await.unwrap();
        assert_eq!(
            captured.headers.get("authorization").unwrap(),
            "Bearer sk-test-token"
        );
    }

    #[tokio::test]
    async fn import_returns_empty_vec_for_paths_with_no_http_methods() {
        let doc = r#"{
            "openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/x":{"summary":"no methods here"}}
        }"#;
        let spec = OpenAPISpec::from_json(doc).unwrap();
        let bundles = adapter(spec, config("svc", "https://x", None))
            .import()
            .await
            .unwrap();
        assert!(bundles.is_empty());
    }

    #[tokio::test]
    async fn text_response_returned_as_string() {
        let base = spawn_echo_server(200, "hello world", "text/plain").await;
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/t":{"get":{"operationId":"t","responses":{"200":{"content":{"text/plain":{"schema":{}}}}}}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-18", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Ok(Value::String(s)) => assert_eq!(s, "hello world"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn undeclared_error_status_returns_http_status_code() {
        let base = spawn_echo_server(500, "boom", "text/plain").await;
        let spec = OpenAPISpec::from_json(
            r#"{"openapi":"3.0.0","info":{"title":"T","version":"1"},
            "paths":{"/x":{"get":{"operationId":"x","responses":{"200":{"content":{"application/json":{"schema":{}}}}}}}}}"#,
        )
        .unwrap();
        let bundles = adapter(spec, config("svc", &base, None))
            .import()
            .await
            .unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-19", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Err(e) => assert_eq!(e.code, "HTTP_500"),
            other => panic!("expected HTTP_500, got {other:?}"),
        }
    }
}
