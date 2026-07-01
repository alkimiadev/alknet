//! CallError → HTTP status/response mapping (ADR-023).
//!
//! See `docs/architecture/crates/http/http-server.md` §"Error Mapping" and
//! ADR-023 for the protocol-level vs operation-level code distinction.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde_json::Value;

use alknet_call::protocol::wire::CallError;
use alknet_core::auth::Identity;

const PROTOCOL_CODE_NOT_FOUND: &str = "NOT_FOUND";
const PROTOCOL_CODE_FORBIDDEN: &str = "FORBIDDEN";
const PROTOCOL_CODE_INVALID_INPUT: &str = "INVALID_INPUT";
const PROTOCOL_CODE_TIMEOUT: &str = "TIMEOUT";
const PROTOCOL_CODE_INTERNAL: &str = "INTERNAL";

const HTTP_PREFIX: &str = "HTTP_";

const STATUS_NOT_FOUND: u16 = 404;
const STATUS_UNAUTHORIZED: u16 = 401;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_UNPROCESSABLE: u16 = 422;
const STATUS_TIMEOUT: u16 = 504;
const STATUS_INTERNAL: u16 = 500;

const RETRY_AFTER_STATUSES: &[u16] = &[429, 503];

pub fn call_error_to_http_status(error: &CallError) -> u16 {
    call_error_to_http_status_with_identity(error, None)
}

pub fn call_error_to_http_status_with_identity(error: &CallError, identity: Option<&Identity>) -> u16 {
    match error.code.as_str() {
        PROTOCOL_CODE_NOT_FOUND => STATUS_NOT_FOUND,
        PROTOCOL_CODE_FORBIDDEN => {
            if identity.is_some() {
                STATUS_FORBIDDEN
            } else {
                STATUS_UNAUTHORIZED
            }
        }
        PROTOCOL_CODE_INVALID_INPUT => STATUS_UNPROCESSABLE,
        PROTOCOL_CODE_TIMEOUT => STATUS_TIMEOUT,
        PROTOCOL_CODE_INTERNAL => STATUS_INTERNAL,
        code if code.starts_with(HTTP_PREFIX) => code[HTTP_PREFIX.len()..]
            .parse::<u16>()
            .unwrap_or(STATUS_INTERNAL),
        _ => STATUS_INTERNAL,
    }
}

pub fn call_error_to_http_response(error: &CallError) -> Response {
    let status_code = call_error_to_http_status(error);
    let status = status_code_from_u16(status_code);
    let body = serde_json::to_value(error).unwrap_or(Value::Null);

    let retry_after = retry_after_value(error, status_code);

    if let Some(retry_after) = retry_after {
        let header_value = HeaderValue::from_str(&retry_after)
            .unwrap_or_else(|_| HeaderValue::from_static("0"));
        (status, [(header::RETRY_AFTER, header_value)], Json(body)).into_response()
    } else {
        (status, Json(body)).into_response()
    }
}

fn status_code_from_u16(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn retry_after_value(error: &CallError, status_code: u16) -> Option<String> {
    if !error.retryable || !RETRY_AFTER_STATUSES.contains(&status_code) {
        return None;
    }
    error
        .details
        .as_ref()
        .and_then(|details| details.get("retry_after"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            error
                .details
                .as_ref()
                .and_then(|details| details.get("retry_after"))
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_core::auth::Identity;
    use std::collections::HashMap;

    fn identity() -> Identity {
        Identity {
            id: "caller".to_string(),
            scopes: vec!["read".to_string()],
            resources: HashMap::new(),
        }
    }

    #[test]
    fn not_found_maps_to_404() {
        let error = CallError::not_found("fs/missing");
        assert_eq!(call_error_to_http_status(&error), 404);
    }

    #[test]
    fn invalid_input_maps_to_422() {
        let error = CallError::invalid_input("bad input");
        assert_eq!(call_error_to_http_status(&error), 422);
    }

    #[test]
    fn timeout_maps_to_504() {
        let error = CallError::timeout("timed out");
        assert_eq!(call_error_to_http_status(&error), 504);
    }

    #[test]
    fn internal_maps_to_500() {
        let error = CallError::internal("boom");
        assert_eq!(call_error_to_http_status(&error), 500);
    }

    #[test]
    fn forbidden_with_none_identity_maps_to_401() {
        let error = CallError::forbidden("auth required");
        assert_eq!(call_error_to_http_status_with_identity(&error, None), 401);
    }

    #[test]
    fn forbidden_with_some_identity_maps_to_403() {
        let error = CallError::forbidden("insufficient scopes");
        let id = identity();
        assert_eq!(call_error_to_http_status_with_identity(&error, Some(&id)), 403);
    }

    #[test]
    fn forbidden_default_uses_401() {
        let error = CallError::forbidden("auth required");
        assert_eq!(call_error_to_http_status(&error), 401);
    }

    #[test]
    fn operation_level_code_with_http_prefix_maps_to_status_number() {
        let error = CallError::new("HTTP_404", "not found", false);
        assert_eq!(call_error_to_http_status(&error), 404);
    }

    #[test]
    fn http_429_maps_to_429_not_collided_with_protocol_codes() {
        let error = CallError::new("HTTP_429", "rate limited", true);
        assert_eq!(call_error_to_http_status(&error), 429);
    }

    #[test]
    fn operation_level_domain_code_without_http_status_maps_to_500() {
        let error = CallError::new("FILE_NOT_FOUND", "file missing", false);
        assert_eq!(call_error_to_http_status(&error), 500);
    }

    #[test]
    fn operation_level_code_with_http_prefix_422_maps_to_422() {
        let error = CallError::new("HTTP_422", "validation error", false);
        assert_eq!(call_error_to_http_status(&error), 422);
    }

    #[test]
    fn unknown_protocol_like_code_maps_to_500() {
        let error = CallError::new("SOMETHING_NEW", "unexpected", false);
        assert_eq!(call_error_to_http_status(&error), 500);
    }

    #[test]
    fn http_prefix_with_non_numeric_suffix_maps_to_500() {
        let error = CallError::new("HTTP_NOTANUMBER", "bad", false);
        assert_eq!(call_error_to_http_status(&error), 500);
    }

    #[test]
    fn call_error_to_http_response_has_status_and_json_body() {
        let error = CallError::not_found("fs/missing");
        let response = call_error_to_http_response(&error);
        assert_eq!(response.status(), StatusCode::from_u16(404).unwrap());
        assert_eq!(
            response.headers().get(axum::http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn retryable_503_adds_retry_after_header_when_details_present() {
        let error = CallError::new("HTTP_503", "slow down", true)
            .with_details(serde_json::json!({ "retry_after": "5" }));
        let response = call_error_to_http_response(&error);
        assert_eq!(response.status(), StatusCode::from_u16(503).unwrap());
        let retry_after = response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("Retry-After header present");
        assert_eq!(retry_after.to_str().unwrap(), "5");
    }

    #[test]
    fn retryable_503_no_retry_after_in_details_omits_header() {
        let error = CallError::new("HTTP_503", "slow down", true);
        let response = call_error_to_http_response(&error);
        assert_eq!(response.status(), StatusCode::from_u16(503).unwrap());
        assert!(response.headers().get(axum::http::header::RETRY_AFTER).is_none());
    }

    #[test]
    fn non_retryable_503_omits_retry_after_header() {
        let error = CallError::new("HTTP_503", "down", false)
            .with_details(serde_json::json!({ "retry_after": "5" }));
        let response = call_error_to_http_response(&error);
        assert!(response.headers().get(axum::http::header::RETRY_AFTER).is_none());
    }

    #[test]
    fn retryable_429_adds_retry_after_header() {
        let error = CallError::new("HTTP_429", "too fast", true)
            .with_details(serde_json::json!({ "retry_after": "10" }));
        let response = call_error_to_http_response(&error);
        assert_eq!(response.status(), StatusCode::from_u16(429).unwrap());
        assert!(response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .is_some());
    }

    #[test]
    fn retryable_timeout_504_omits_retry_after_header() {
        let error = CallError::timeout("timed out");
        let response = call_error_to_http_response(&error);
        assert_eq!(response.status(), StatusCode::from_u16(504).unwrap());
        assert!(response.headers().get(axum::http::header::RETRY_AFTER).is_none());
    }

    #[test]
    fn retry_after_numeric_u64_is_used() {
        let error = CallError::new("HTTP_503", "slow", true)
            .with_details(serde_json::json!({ "retry_after": 30 }));
        let response = call_error_to_http_response(&error);
        let retry_after = response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("Retry-After present");
        assert_eq!(retry_after.to_str().unwrap(), "30");
    }

    #[test]
    fn with_identity_function_consistent_with_default_for_non_forbidden() {
        let error = CallError::not_found("op");
        let id = identity();
        assert_eq!(
            call_error_to_http_status_with_identity(&error, Some(&id)),
            404
        );
        assert_eq!(call_error_to_http_status_with_identity(&error, None), 404);
    }
}