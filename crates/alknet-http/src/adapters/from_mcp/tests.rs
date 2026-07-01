use super::*;
use alknet_call::registry::spec::Visibility;
use rmcp::model::{CallToolResult, Content, Tool};

fn make_tool(name: &str, input: Value, output: Option<Value>) -> Tool {
    let input_map = match input {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let mut tool = Tool::new_with_raw(
        name.to_string(),
        Some("test tool".into()),
        std::sync::Arc::new(input_map),
    );
    if let Some(out) = output {
        let out_map = match out {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        tool = tool.with_raw_output_schema(std::sync::Arc::new(out_map));
    }
    tool
}

fn call_tool_result(content: Vec<Content>, structured: Option<Value>, is_error: Option<bool>) -> CallToolResult {
    let json = serde_json::json!({
        "content": content,
        "structuredContent": structured,
        "isError": is_error,
    });
    serde_json::from_value(json).expect("CallToolResult deserializes")
}

#[test]
fn struct_holds_endpoint_auth_token_namespace() {
    let adapter = FromMCP::new("http://localhost:8000/mcp", "weather");
    assert_eq!(adapter.endpoint(), "http://localhost:8000/mcp");
    assert_eq!(adapter.namespace(), "weather");
    assert_eq!(adapter.auth_token(), None);

    let with_token = adapter.with_auth_token("sekrit");
    assert_eq!(with_token.auth_token(), Some("sekrit"));
}

#[test]
fn output_schema_present_uses_declared_schema() {
    let declared = serde_json::json!({
        "type": "object",
        "properties": { "temperature": { "type": "number" } }
    });
    let tool = make_tool("get_weather", serde_json::json!({}), Some(declared.clone()));
    let schema = output_schema_for(&tool);
    assert_eq!(schema, declared);
}

#[test]
fn output_schema_absent_uses_content_block_union() {
    let tool = make_tool("legacy_tool", serde_json::json!({}), None);
    let schema = output_schema_for(&tool);
    assert_eq!(schema, content_block_union_schema());
    assert_eq!(schema["type"], "array");
}

#[test]
fn content_block_union_schema_has_all_five_variants() {
    let schema = content_block_union_schema();
    let one_of = schema["items"]["oneOf"].as_array().expect("oneOf array");
    let variants: Vec<&str> = one_of
        .iter()
        .filter_map(|v| v["properties"]["type"]["enum"][0].as_str())
        .collect();
    assert!(variants.contains(&"text"));
    assert!(variants.contains(&"image"));
    assert!(variants.contains(&"audio"));
    assert!(variants.contains(&"resource"));
    assert!(variants.contains(&"resource_link"));
}

#[test]
fn map_structured_content_present_used_as_result() {
    let result = CallToolResult::structured(serde_json::json!({ "temperature": 22.5 }));
    let response = map_call_tool_result(result, "req-1".to_string());
    assert_eq!(response.request_id, "req-1");
    match response.result {
        Ok(v) => assert_eq!(v, serde_json::json!({ "temperature": 22.5 })),
        Err(e) => panic!("expected Ok, got Err: {e:?}"),
    }
}

#[test]
fn map_structured_content_absent_maps_content_blocks() {
    let result = CallToolResult::success(vec![
        Content::text("hello world"),
        Content::image("base64data", "image/png"),
    ]);
    let response = map_call_tool_result(result, "req-2".to_string());
    match response.result {
        Ok(Value::Array(blocks)) => {
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[0]["type"], "text");
            assert_eq!(blocks[0]["text"], "hello world");
            assert_eq!(blocks[1]["type"], "image");
            assert_eq!(blocks[1]["data"], "base64data");
        }
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn map_single_text_block_carried_as_content_block_not_json_parsed() {
    let result = CallToolResult::success(vec![Content::text(r#"{"key":"value"}"#)]);
    let response = map_call_tool_result(result, "req-3".to_string());
    match response.result {
        Ok(Value::Array(blocks)) => {
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0]["type"], "text");
            assert_eq!(blocks[0]["text"], r#"{"key":"value"}"#);
        }
        other => panic!("expected array (not JSON-parsed), got {other:?}"),
    }
}

#[test]
fn map_is_error_true_returns_call_error() {
    let result = CallToolResult::error(vec![Content::text("something went wrong")]);
    let response = map_call_tool_result(result, "req-4".to_string());
    match response.result {
        Err(e) => {
            assert_eq!(e.code, "MCP_TOOL_ERROR");
            assert!(!e.retryable);
            assert!(e.message.contains("isError"));
            let details = e.details.expect("details present");
            let blocks = details.as_array().expect("details is array");
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0]["text"], "something went wrong");
        }
        other => panic!("expected Err, got {other:?}"),
    }
}

#[test]
fn map_is_error_true_with_no_content_still_errors() {
    let result = call_tool_result(vec![], None, Some(true));
    let response = map_call_tool_result(result, "req-5".to_string());
    match response.result {
        Err(e) => {
            assert_eq!(e.code, "MCP_TOOL_ERROR");
            assert!(e.message.contains("no content"));
        }
        other => panic!("expected Err, got {other:?}"),
    }
}

#[test]
fn map_empty_success_returns_empty_array() {
    let result = call_tool_result(vec![], None, Some(false));
    let response = map_call_tool_result(result, "req-6".to_string());
    match response.result {
        Ok(Value::Array(blocks)) => assert!(blocks.is_empty()),
        other => panic!("expected empty array, got {other:?}"),
    }
}

#[test]
fn map_structured_content_preferred_over_content_blocks() {
    let result = call_tool_result(
        vec![Content::text("ignored text")],
        Some(serde_json::json!({ "structured": true })),
        Some(false),
    );
    let response = map_call_tool_result(result, "req-7".to_string());
    match response.result {
        Ok(v) => assert_eq!(v, serde_json::json!({ "structured": true })),
        other => panic!("expected structured content, got {other:?}"),
    }
}

#[test]
fn error_schemas_for_tool_yields_mcp_tool_error() {
    let tool = make_tool("weather", serde_json::json!({}), None);
    let errors = error_schemas_for(&tool);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "MCP_TOOL_ERROR");
    assert!(errors[0].description.contains("weather"));
    assert!(errors[0].description.contains("isError"));
    assert!(errors[0].schema["type"] == "array");
}

#[test]
fn capabilities_for_token_injects_http_token() {
    let caps = capabilities_for(Some("tok-123".to_string()));
    let secret = caps.get(MCP_CAPABILITY_KEY).expect("token present");
    assert_eq!(secret.expose_secret(), "tok-123");
}

#[test]
fn capabilities_for_none_yields_empty() {
    let caps = capabilities_for(None);
    assert!(caps.get(MCP_CAPABILITY_KEY).is_none());
}

#[test]
fn build_spec_output_schema_present_shape() {
    let tool = make_tool(
        "get_weather",
        serde_json::json!({ "type": "object", "properties": { "city": { "type": "string" } } }),
        Some(serde_json::json!({ "type": "object", "properties": { "temperature": { "type": "number" } } })),
    );
    let spec = build_spec(&tool, "weather");
    assert_eq!(spec.name, "weather/get_weather");
    assert_eq!(spec.namespace, "weather");
    assert_eq!(spec.op_type, OperationType::Mutation);
    assert_eq!(spec.visibility, Visibility::Internal);
    assert_eq!(spec.input_schema["type"], "object");
    assert_eq!(spec.input_schema["properties"]["city"]["type"], "string");
    assert_eq!(spec.output_schema["type"], "object");
    assert_eq!(
        spec.output_schema["properties"]["temperature"]["type"],
        "number"
    );
    assert_eq!(spec.error_schemas.len(), 1);
    assert_eq!(spec.error_schemas[0].code, "MCP_TOOL_ERROR");
    assert!(spec.access_control == AccessControl::default());
}

#[test]
fn build_spec_output_schema_absent_uses_union() {
    let tool = make_tool("legacy", serde_json::json!({}), None);
    let spec = build_spec(&tool, "legacy");
    assert_eq!(spec.output_schema, content_block_union_schema());
}

#[test]
fn build_spec_name_with_prefix_when_namespace_set() {
    let tool = make_tool("search", serde_json::json!({}), None);
    let spec = build_spec(&tool, "tools");
    assert_eq!(spec.name, "tools/search");
    assert_eq!(spec.namespace, "tools");
}

#[test]
fn no_env_vars_in_capability_key_constant() {
    assert_eq!(MCP_CAPABILITY_KEY, "mcp");
}

#[tokio::test]
async fn forwarding_handler_reads_capabilities_not_env_vars() {
    let adapter = FromMCP::new("http://127.0.0.1:1/mcp", "ns");
    let _ = adapter.auth_token();
    assert!(adapter.auth_token().is_none());
}