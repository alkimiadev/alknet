use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

use crate::auth::IdentityProvider;
use crate::http::auth::auth_middleware;

async fn default_404() -> impl IntoResponse {
    axum::http::StatusCode::NOT_FOUND
}

pub fn build_router(identity_provider: Arc<dyn IdentityProvider>) -> Router {
    Router::new()
        .fallback(default_404)
        .layer(axum::middleware::from_fn_with_state(
            identity_provider,
            auth_middleware,
        ))
}

pub async fn serve_connection<S>(stream: S, identity_provider: Arc<dyn IdentityProvider>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let app = build_router(identity_provider);
    let io = TokioIo::new(stream);

    let hyper_service = TowerToHyperService::new(app.into_service::<hyper::body::Incoming>());

    let result = Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(io, hyper_service)
        .await;

    if let Err(e) = result {
        tracing::debug!("http connection error: {e}");
    }
}

pub async fn serve_connection_from_reader<S>(
    reader: BufReader<S>,
    identity_provider: Arc<dyn IdentityProvider>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    serve_connection(reader, identity_provider).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthToken, Identity};
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::response::IntoResponse;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    struct NullIdentityProvider;

    impl IdentityProvider for NullIdentityProvider {
        fn resolve_from_fingerprint(&self, _fingerprint: &str) -> Option<Identity> {
            None
        }

        fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
            None
        }
    }

    #[tokio::test]
    async fn default_404_handler_returns_not_found() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(MockValidProvider);
        let app = build_router(provider);

        let req = HttpRequest::builder()
            .uri("/anything")
            .header("authorization", "Bearer alk_sometoken1")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_auth_returns_401_before_404() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(MockValidProvider);
        let app = build_router(provider);

        let req = HttpRequest::builder()
            .uri("/anything")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_auth_returns_401_before_404() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NullIdentityProvider);
        let app = build_router(provider);

        let req = HttpRequest::builder()
            .uri("/anything")
            .header("authorization", "Bearer alk_sometoken1")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unmatched_route_returns_404_with_valid_auth() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(MockValidProvider);
        let app = build_router(provider);

        let req = HttpRequest::builder()
            .uri("/v1/unknown/op")
            .header("authorization", "Bearer alk_valid")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    struct MockValidProvider;

    impl IdentityProvider for MockValidProvider {
        fn resolve_from_fingerprint(&self, _fingerprint: &str) -> Option<Identity> {
            None
        }

        fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
            Some(Identity {
                id: "test".to_string(),
                scopes: vec![],
                resources: HashMap::new(),
            })
        }
    }
}
