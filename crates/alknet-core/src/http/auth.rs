use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::{AuthToken, Identity, IdentityProvider};

#[derive(Clone)]
pub struct IdentityExt(pub Identity);

pub async fn auth_middleware(
    axum::extract::State(identity_provider): axum::extract::State<
        std::sync::Arc<dyn IdentityProvider>,
    >,
    mut request: Request,
    next: Next,
) -> Response {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token_str = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
    };

    let token = AuthToken {
        raw: token_str.as_bytes().to_vec(),
    };

    match identity_provider.resolve_from_token(&token) {
        Some(identity) => {
            request.extensions_mut().insert(IdentityExt(identity));
            next.run(request).await
        }
        None => axum::http::StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    struct MockIdentityProvider {
        valid_token: String,
        identity: Identity,
    }

    impl IdentityProvider for MockIdentityProvider {
        fn resolve_from_fingerprint(&self, _fingerprint: &str) -> Option<Identity> {
            None
        }

        fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
            let token_str = String::from_utf8_lossy(&token.raw);
            if token_str == self.valid_token {
                Some(self.identity.clone())
            } else {
                None
            }
        }
    }

    fn make_provider(valid_token: &str) -> Arc<dyn IdentityProvider> {
        let identity = Identity {
            id: "test-user".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        Arc::new(MockIdentityProvider {
            valid_token: valid_token.to_string(),
            identity,
        })
    }

    #[tokio::test]
    async fn auth_middleware_extracts_bearer_token() {
        let provider = make_provider("alk_validtoken1");
        let app = Router::new()
            .route(
                "/test",
                get(|request: Request| async move {
                    let has_identity = request.extensions().get::<IdentityExt>().is_some();
                    if has_identity {
                        StatusCode::OK.into_response()
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                provider,
                auth_middleware,
            ));

        let req = HttpRequest::builder()
            .uri("/test")
            .header("authorization", "Bearer alk_validtoken1")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_returns_401_for_missing_token() {
        let provider = make_provider("alk_validtoken1");
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::OK.into_response() }))
            .layer(axum::middleware::from_fn_with_state(
                provider,
                auth_middleware,
            ));

        let req = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_returns_401_for_invalid_token() {
        let provider = make_provider("alk_validtoken1");
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::OK.into_response() }))
            .layer(axum::middleware::from_fn_with_state(
                provider,
                auth_middleware,
            ));

        let req = HttpRequest::builder()
            .uri("/test")
            .header("authorization", "Bearer alk_wrongtoken1")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_attaches_identity_to_extensions() {
        let provider = make_provider("alk_testidentity1");
        let app = Router::new()
            .route(
                "/test",
                get(|request: Request| async move {
                    let identity = request.extensions().get::<IdentityExt>().unwrap();
                    identity.0.id.clone()
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                provider,
                auth_middleware,
            ));

        let req = HttpRequest::builder()
            .uri("/test")
            .header("authorization", "Bearer alk_testidentity1")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"test-user");
    }
}
