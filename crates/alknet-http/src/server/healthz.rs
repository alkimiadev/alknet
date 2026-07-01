//! `GET /healthz` — the one raw HTTP route (ADR-036).
//!
//! No auth, no call protocol, no `OperationContext`. Returns `200 OK` with a
//! plain-text body (`"ok"`) if the endpoint is healthy. This is the
//! infrastructure endpoint load balancers and orchestrators call; it must
//! work before identity is resolvable. See
//! `docs/architecture/crates/http/http-server.md` §"/healthz (raw route)".

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;

const HEALTHZ_BODY: &str = "ok";

pub async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
        HEALTHZ_BODY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;

    async fn call_healthz(req: Request<Body>) -> axum::response::Response {
        let app = axum::Router::new().route("/healthz", axum::routing::get(healthz));
        tower::ServiceExt::<Request<Body>>::oneshot(app, req)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn healthz_handler_returns_200_with_plain_text_ok() {
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = call_healthz(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string());
        assert_eq!(ctype.as_deref(), Some("text/plain"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"ok");
    }

    #[tokio::test]
    async fn healthz_works_with_no_authorization_header() {
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = call_healthz(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}