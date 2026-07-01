//! Shared Bearer auth axum middleware.
//!
//! Resolves the `Authorization: Bearer` header via
//! `IdentityProvider::resolve_from_token()` and stashes the resolved
//! `Option<Identity>` in request extensions. Shared by the HTTP gateway
//! endpoints and the `to_mcp` rmcp service (research §4.4). See
//! `docs/architecture/crates/http/http-server.md` §"Auth" and
//! [ADR-004](../../../docs/architecture/decisions/004-auth-as-shared-core.md).
//!
//! Resolution semantics:
//! - No `Authorization` header → `None` (request proceeds; the route
//!   handler / `AccessControl` decides whether to reject).
//! - Malformed `Authorization` header (not `Bearer <token>`) → `None`
//!   (treated as no-token, not an error — Bearer-only is the auth
//!   mechanism).
//! - Token present but resolution fails → `None` (treat as
//!   unauthenticated, matching the `CallAdapter`'s per-request identity
//!   resolution behavior).
//!
//! This middleware resolves identity and stashes it; it does NOT enforce
//! `AccessControl` (the route handlers / `GatewayDispatch::invoke()` do)
//! or map `CallError` codes to HTTP status (the error-mapping task does).

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use http::request::Parts;

use alknet_core::auth::{AuthToken, Identity, IdentityProvider};

/// Axum middleware that resolves the `Authorization: Bearer` header via
/// `IdentityProvider::resolve_from_token()` and stashes the resolved
/// `Option<Identity>` in request extensions. Shared by the HTTP gateway
/// endpoints and the `to_mcp` rmcp service (research §4.4).
///
/// The state is `Arc<dyn IdentityProvider>` so the middleware can be applied
/// via `middleware::from_fn_with_state(idp.clone(), bearer_auth_middleware)`
/// around both HTTP routes and a nested rmcp service.
pub async fn bearer_auth_middleware(
    State(identity_provider): State<Arc<dyn IdentityProvider>>,
    mut request: Request,
    next: Next,
) -> Response {
    let identity = extract_bearer_identity(&request, identity_provider.as_ref());
    request.extensions_mut().insert(identity);
    next.run(request).await
}

/// Extract the `Authorization: Bearer <token>` header and resolve it to
/// an `Option<Identity>`. Returns `None` if no token is present (the
/// request proceeds unauthenticated; the route handler / `AccessControl`
/// decides whether to reject). Returns `None` if the token is present
/// but resolution fails (treat as unauthenticated, not as an error —
/// matches the `CallAdapter`'s per-request identity resolution behavior).
pub fn extract_bearer_identity(
    request: &Request,
    identity_provider: &dyn IdentityProvider,
) -> Option<Identity> {
    let header = request.headers().get(AUTHORIZATION)?;
    let token_str = header.to_str().ok()?.strip_prefix("Bearer ")?;
    let token = AuthToken {
        raw: token_str.as_bytes().to_vec(),
    };
    identity_provider.resolve_from_token(&token)
}

/// Axum extractor: the resolved bearer identity (or `None` if
/// unauthenticated). Read from request extensions (stashed by
/// `bearer_auth_middleware`).
#[derive(Clone, Debug)]
pub struct ResolvedIdentity(pub Option<Identity>);

impl<S> FromRequestParts<S> for ResolvedIdentity
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let identity = parts.extensions.get::<Option<Identity>>().cloned().flatten();
        Ok(ResolvedIdentity(identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as AxumRequest, StatusCode};
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn sample_identity() -> Identity {
        Identity {
            id: "worker-a".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        }
    }

    struct StaticProvider {
        identity: Option<Identity>,
    }

    impl IdentityProvider for StaticProvider {
        fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
            self.identity.clone()
        }
    }

    fn provider(identity: Option<Identity>) -> Arc<dyn IdentityProvider> {
        Arc::new(StaticProvider { identity })
    }

    fn request_with_authorization(value: Option<&str>) -> Request {
        let mut builder = AxumRequest::builder();
        if let Some(v) = value {
            builder = builder.header(AUTHORIZATION, v);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn extract_returns_some_for_valid_bearer_when_provider_resolves() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("Bearer alk_testsecret"));
        let identity = extract_bearer_identity(&req, idp.as_ref());
        assert!(identity.is_some());
        assert_eq!(identity.unwrap().id, "worker-a");
    }

    #[test]
    fn extract_returns_none_for_missing_authorization_header() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(None);
        let identity = extract_bearer_identity(&req, idp.as_ref());
        assert!(identity.is_none());
    }

    #[test]
    fn extract_returns_none_for_malformed_authorization_header() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("not-a-bearer-scheme"));
        let identity = extract_bearer_identity(&req, idp.as_ref());
        assert!(identity.is_none());
    }

    #[test]
    fn extract_returns_none_for_basic_auth_bearer_only() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("Basic dXNlcjpwYXNz"));
        let identity = extract_bearer_identity(&req, idp.as_ref());
        assert!(identity.is_none());
    }

    #[test]
    fn extract_returns_none_when_token_present_but_resolution_fails() {
        let idp = provider(None);
        let req = request_with_authorization(Some("Bearer alk_unknown"));
        let identity = extract_bearer_identity(&req, idp.as_ref());
        assert!(identity.is_none());
    }

    async fn run_middleware(
        idp: Arc<dyn IdentityProvider>,
        request: Request,
    ) -> Response {
        let app: Router<()> = Router::new()
            .route(
                "/",
                get(|req: Request| async move {
                    let identity = req.extensions().get::<Option<Identity>>().cloned().flatten();
                    if let Some(id) = identity {
                        (StatusCode::OK, id.id)
                    } else {
                        (StatusCode::OK, "none".to_string())
                    }
                }),
            )
            .layer(from_fn_with_state(idp, bearer_auth_middleware));

        app.oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn middleware_stashes_some_identity_for_valid_bearer() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("Bearer alk_testsecret"));
        let response = run_middleware(idp, req).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"worker-a");
    }

    #[tokio::test]
    async fn middleware_stashes_none_when_no_authorization_header() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(None);
        let response = run_middleware(idp, req).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"none");
    }

    #[tokio::test]
    async fn middleware_stashes_none_for_malformed_authorization() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("garbage"));
        let response = run_middleware(idp, req).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"none");
    }

    #[tokio::test]
    async fn middleware_stashes_none_for_basic_auth() {
        let idp = provider(Some(sample_identity()));
        let req = request_with_authorization(Some("Basic dXNlcjpwYXNz"));
        let response = run_middleware(idp, req).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"none");
    }

    #[tokio::test]
    async fn middleware_stashes_none_when_resolution_fails() {
        let idp = provider(None);
        let req = request_with_authorization(Some("Bearer alk_unknown"));
        let response = run_middleware(idp, req).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"none");
    }

    #[tokio::test]
    async fn resolved_identity_extractor_retrieves_stashed_some() {
        let idp = provider(Some(sample_identity()));
        let app: Router<()> = Router::new()
            .route(
                "/",
                get(
                    |ResolvedIdentity(identity): ResolvedIdentity| async move {
                        match identity {
                            Some(id) => (StatusCode::OK, id.id),
                            None => (StatusCode::OK, "none".to_string()),
                        }
                    },
                ),
            )
            .layer(from_fn_with_state(idp, bearer_auth_middleware));

        let req = request_with_authorization(Some("Bearer alk_testsecret"));
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"worker-a");
    }

    #[tokio::test]
    async fn resolved_identity_extractor_retrieves_stashed_none() {
        let idp = provider(Some(sample_identity()));
        let app: Router<()> = Router::new()
            .route(
                "/",
                get(
                    |ResolvedIdentity(identity): ResolvedIdentity| async move {
                        match identity {
                            Some(id) => (StatusCode::OK, id.id),
                            None => (StatusCode::OK, "none".to_string()),
                        }
                    },
                ),
            )
            .layer(from_fn_with_state(idp, bearer_auth_middleware));

        let req = request_with_authorization(None);
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"none");
    }
}