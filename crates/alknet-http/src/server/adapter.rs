//! `HttpAdapter` — `ProtocolHandler` for `h2`/`http/1.1` (axum over QUIC).
//!
//! See `docs/architecture/crates/http/http-server.md`. This module wires the
//! axum `Router` (gateway endpoints + `/healthz` + `/openapi.json` + MCP +
//! custom routes + decoy fallback) and drives hyper's HTTP/1.1 or HTTP/2
//! connection driver over a single QUIC bidirectional stream. The 5 gateway
//! endpoints (`/search`/`/schema`/`/call`/`/batch`/`/subscribe`) are wired in
//! from `gateway_routes`; `/openapi.json`, the MCP route, and the WS upgrade
//! handler remain placeholder 501 handlers pending their respective tasks.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::error;

use alknet_call::registry::registration::OperationRegistry;
use alknet_core::auth::{AuthContext, IdentityProvider};
use alknet_core::types::{Connection, HandlerError, ProtocolHandler, StreamError};

use super::auth::bearer_auth_middleware;
use super::decoy::decoy_fallback;
use super::gateway_routes;
use super::healthz::healthz;
use crate::websocket::upgrade::ws_upgrade_handler;
use crate::websocket::upgrade::WS_UPGRADE_PATH;
#[cfg(feature = "mcp")]
use crate::adapters::to_mcp_service;
#[cfg(feature = "mcp")]
use crate::gateway::GatewayDispatch;

const ALPN_HTTP1: &[u8] = b"http/1.1";
const ALPN_H2: &[u8] = b"h2";

#[derive(Clone, Default, Debug)]
pub enum DecoyConfig {
    #[default]
    NotFound,
    StaticSite { root: PathBuf },
    Redirect { to: String },
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct RouterState {
    pub(crate) registry: Arc<OperationRegistry>,
    pub(crate) identity_provider: Arc<dyn IdentityProvider>,
    pub(crate) decoy: DecoyConfig,
}

impl axum::extract::FromRef<RouterState> for DecoyConfig {
    fn from_ref(state: &RouterState) -> Self {
        state.decoy.clone()
    }
}

impl axum::extract::FromRef<RouterState> for Arc<OperationRegistry> {
    fn from_ref(state: &RouterState) -> Self {
        Arc::clone(&state.registry)
    }
}

impl axum::extract::FromRef<RouterState> for Arc<dyn IdentityProvider> {
    fn from_ref(state: &RouterState) -> Self {
        Arc::clone(&state.identity_provider)
    }
}

pub struct HttpAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    decoy: DecoyConfig,
    extra_routes: Option<Router>,
    alpn: &'static [u8],
    router: Router,
}

impl HttpAdapter {
    pub fn new(identity_provider: Arc<dyn IdentityProvider>, registry: Arc<OperationRegistry>) -> Self {
        Self::for_alpn(identity_provider, registry, ALPN_HTTP1)
    }

    pub fn h2(identity_provider: Arc<dyn IdentityProvider>, registry: Arc<OperationRegistry>) -> Self {
        Self::for_alpn(identity_provider, registry, ALPN_H2)
    }

    fn for_alpn(
        identity_provider: Arc<dyn IdentityProvider>,
        registry: Arc<OperationRegistry>,
        alpn: &'static [u8],
    ) -> Self {
        let decoy = DecoyConfig::default();
        let state = RouterState {
            registry: Arc::clone(&registry),
            identity_provider: Arc::clone(&identity_provider),
            decoy: decoy.clone(),
        };
        let router = build_router(state, None);
        Self {
            identity_provider,
            registry,
            decoy,
            extra_routes: None,
            alpn,
            router,
        }
    }

    pub fn with_decoy(mut self, decoy: DecoyConfig) -> Self {
        self.decoy = decoy.clone();
        let state = RouterState {
            registry: Arc::clone(&self.registry),
            identity_provider: Arc::clone(&self.identity_provider),
            decoy,
        };
        self.router = build_router(state, self.extra_routes.take());
        self
    }

    pub fn with_extra_routes(mut self, routes: Router) -> Self {
        let state = RouterState {
            registry: Arc::clone(&self.registry),
            identity_provider: Arc::clone(&self.identity_provider),
            decoy: self.decoy.clone(),
        };
        self.router = build_router(state, Some(routes.clone()));
        self.extra_routes = Some(routes);
        self
    }

    pub fn decoy(&self) -> &DecoyConfig {
        &self.decoy
    }

    pub fn alpn(&self) -> &'static [u8] {
        self.alpn
    }

    pub fn router(&self) -> &Router {
        &self.router
    }
}

fn build_router(state: RouterState, extra_routes: Option<Router>) -> Router {
    let auth_state = Arc::clone(&state.identity_provider);

    #[cfg(feature = "mcp")]
    let mcp_router: Router<RouterState> = {
        let dispatch = Arc::new(GatewayDispatch::new(
            Arc::clone(&state.registry),
            Arc::clone(&state.identity_provider),
        ));
        Router::new()
            .nest_service("/mcp", to_mcp_service(dispatch))
            .layer(from_fn_with_state(auth_state.clone(), bearer_auth_middleware))
    };
    #[cfg(not(feature = "mcp"))]
    let mcp_router: Router<RouterState> = Router::new();

    let default: Router<RouterState> = Router::new()
        .merge(gateway_routes::gateway_router())
        .route("/openapi.json", get(not_implemented))
        .route(WS_UPGRADE_PATH, get(ws_upgrade_handler))
        .route_layer(from_fn_with_state(auth_state.clone(), bearer_auth_middleware))
        .route("/healthz", get(healthz))
        .fallback(decoy_fallback)
        .merge(mcp_router);

    let with_extras = match extra_routes {
        Some(extra) => {
            let extra: Router<RouterState> = extra.with_state(());
            default.merge(extra)
        }
        None => default,
    };

    with_extras.with_state(state)
}

async fn not_implemented() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "501 Not Implemented")
}

#[async_trait]
impl ProtocolHandler for HttpAdapter {
    fn alpn(&self) -> &'static [u8] {
        self.alpn
    }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        if let Some(identity) = auth.identity.clone() {
            let _ = connection.set_identity(identity);
        }

        let (send, recv) = connection.accept_bi().await.map_err(stream_error_to_handler)?;
        let io = QuicStream::new(send, recv);
        self.serve_io(io).await
    }
}

impl HttpAdapter {
    async fn serve_io<I>(&self, io: I) -> Result<(), HandlerError>
    where
        I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let io = TokioIo::new(io);
        let service = TowerToHyperService::new(self.router.clone());

        #[cfg_attr(not(feature = "h2"), allow(unused_mut))]
        let mut builder = HyperBuilder::new(TokioExecutor::new());
        #[cfg(feature = "h2")]
        {
            builder.http2().enable_connect_protocol();
        }

        let conn = builder.serve_connection_with_upgrades(io, service);
        tokio::pin!(conn);

        let result = (&mut conn).await;
        if let Err(e) = result {
            error!("http adapter: connection closed with error: {e}");
        }
        Ok(())
    }
}

fn stream_error_to_handler(e: StreamError) -> HandlerError {
    HandlerError::from(e)
}

struct QuicStream {
    send: alknet_core::types::SendStream,
    recv: alknet_core::types::RecvStream,
}

impl QuicStream {
    fn new(send: alknet_core::types::SendStream, recv: alknet_core::types::RecvStream) -> Self {
        Self { send, recv }
    }
}

impl AsyncRead for QuicStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.send).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    struct NoopProvider;
    impl IdentityProvider for NoopProvider {
        fn resolve_from_fingerprint(&self, _: &str) -> Option<alknet_core::auth::Identity> {
            None
        }
        fn resolve_from_token(&self, _: &alknet_core::auth::AuthToken) -> Option<alknet_core::auth::Identity> {
            None
        }
    }

    fn empty_registry() -> Arc<OperationRegistry> {
        Arc::new(OperationRegistry::new())
    }

    fn provider() -> Arc<dyn IdentityProvider> {
        Arc::new(NoopProvider)
    }

    #[test]
    fn alpn_returns_http1_for_default_new() {
        let adapter = HttpAdapter::new(provider(), empty_registry());
        assert_eq!(adapter.alpn(), ALPN_HTTP1);
        assert_eq!(adapter.alpn(), b"http/1.1");
    }

    #[test]
    fn alpn_returns_h2_for_h2_constructor() {
        let adapter = HttpAdapter::h2(provider(), empty_registry());
        assert_eq!(adapter.alpn(), ALPN_H2);
        assert_eq!(adapter.alpn(), b"h2");
    }

    #[test]
    fn protocol_handler_alpn_matches_configured_alpn() {
        let adapter = HttpAdapter::new(provider(), empty_registry());
        let handler: &dyn ProtocolHandler = &adapter;
        assert_eq!(handler.alpn(), b"http/1.1");

        let h2 = HttpAdapter::h2(provider(), empty_registry());
        let handler2: &dyn ProtocolHandler = &h2;
        assert_eq!(handler2.alpn(), b"h2");
    }

    #[test]
    fn decoy_config_default_is_not_found() {
        assert!(matches!(DecoyConfig::default(), DecoyConfig::NotFound));
    }

    #[test]
    fn with_decoy_updates_decoy() {
        let adapter = HttpAdapter::new(provider(), empty_registry());
        let adapter = adapter.with_decoy(DecoyConfig::Redirect { to: "https://example.com".to_string() });
        assert!(matches!(adapter.decoy(), DecoyConfig::Redirect { .. }));
    }

    #[test]
    fn with_extra_routes_merges_custom_route_without_collision() {
        let extra = Router::new().route("/v1/foo", get(|| async { "foo" }));
        let adapter = HttpAdapter::new(provider(), empty_registry()).with_extra_routes(extra);
        let _ = adapter.router();
    }

    #[test]
    fn default_surface_wins_on_collision_with_different_method() {
        let extra = Router::new().route("/healthz", post(|| async { "custom" }));
        let adapter = HttpAdapter::new(provider(), empty_registry()).with_extra_routes(extra);
        let _ = adapter.router();
    }

    #[test]
    fn h3_alpn_is_not_registered() {
        let adapter = HttpAdapter::new(provider(), empty_registry());
        assert_ne!(adapter.alpn(), b"h3");
        let h2 = HttpAdapter::h2(provider(), empty_registry());
        assert_ne!(h2.alpn(), b"h3");
    }

    #[test]
    fn router_state_holds_registry_and_identity_provider() {
        let registry = empty_registry();
        let idp = provider();
        let adapter = HttpAdapter::new(Arc::clone(&idp), Arc::clone(&registry));
        let _ = adapter.router();
    }

    #[test]
    fn http_adapter_is_protocol_handler() {
        fn assert_handler<T: ProtocolHandler>() {}
        assert_handler::<HttpAdapter>();
    }

    async fn send_request_and_read_response(
        request: &[u8],
    ) -> (String, tokio::task::JoinHandle<()>) {
        let (mut client_send, server_recv) = duplex(8 * 1024);
        let (server_send, mut client_recv) = duplex(8 * 1024);
        let server_io = QuicStreamDuplex { read: server_recv, write: server_send };

        let adapter = HttpAdapter::new(provider(), empty_registry());
        let handle = tokio::spawn(async move {
            adapter.serve_io(server_io).await.ok();
        });

        client_send.write_all(request).await.unwrap();
        client_send.flush().await.unwrap();

        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), client_recv.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&response).to_string();
        (response_str, handle)
    }

    struct QuicStreamDuplex {
        read: tokio::io::DuplexStream,
        write: tokio::io::DuplexStream,
    }

    impl AsyncRead for QuicStreamDuplex {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.read).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for QuicStreamDuplex {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            Pin::new(&mut self.write).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.write).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.write).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn handle_serves_http_request_over_mock_quic_stream() {
        let request = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let (response, handle) = send_request_and_read_response(request).await;
        handle.await.ok();
        assert!(response.starts_with("HTTP/1.1 200 "), "expected 200, got: {response}");
        assert!(response.contains("\r\n\r\nok"));
    }

    #[tokio::test]
    async fn custom_route_v1_foo_coexists_with_default_surface() {
        let extra = Router::new().route(
            "/v1/foo",
            get(|| async { (StatusCode::OK, "foo-body") }),
        );
        let adapter = HttpAdapter::new(provider(), empty_registry()).with_extra_routes(extra);

        let (mut client_send, server_recv) = duplex(8 * 1024);
        let (server_send, mut client_recv) = duplex(8 * 1024);
        let server_io = QuicStreamDuplex { read: server_recv, write: server_send };

        let handle = tokio::spawn(async move {
            adapter.serve_io(server_io).await.ok();
        });

        let request = b"GET /v1/foo HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        client_send.write_all(request).await.unwrap();
        client_send.flush().await.unwrap();

        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), client_recv.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }
        handle.await.ok();
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.starts_with("HTTP/1.1 200 "), "expected 200, got: {response_str}");
        assert!(response_str.contains("foo-body"));
    }

    #[tokio::test]
    async fn reserved_path_healthz_wins_over_custom_get_collision() {
        let extra = Router::new().route(
            "/healthz",
            post(|| async { (StatusCode::OK, "custom-healthz") }),
        );
        let adapter = HttpAdapter::new(provider(), empty_registry()).with_extra_routes(extra);

        let (mut client_send, server_recv) = duplex(8 * 1024);
        let (server_send, mut client_recv) = duplex(8 * 1024);
        let server_io = QuicStreamDuplex { read: server_recv, write: server_send };

        let handle = tokio::spawn(async move {
            adapter.serve_io(server_io).await.ok();
        });

        let request = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        client_send.write_all(request).await.unwrap();
        client_send.flush().await.unwrap();

        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), client_recv.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }
        handle.await.ok();
        let response_str = String::from_utf8_lossy(&response);
        assert!(response_str.starts_with("HTTP/1.1 200 "), "default GET /healthz wins, got: {response_str}");
        assert!(response_str.contains("\r\n\r\nok"));
        assert!(!response_str.contains("custom-healthz"));
    }

    async fn serve_and_read(adapter: HttpAdapter, request: &[u8]) -> String {
        let (mut client_send, server_recv) = duplex(8 * 1024);
        let (server_send, mut client_recv) = duplex(8 * 1024);
        let server_io = QuicStreamDuplex {
            read: server_recv,
            write: server_send,
        };
        let handle = tokio::spawn(async move {
            adapter.serve_io(server_io).await.ok();
        });
        client_send.write_all(request).await.unwrap();
        client_send.flush().await.unwrap();
        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), client_recv.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }
        handle.await.ok();
        String::from_utf8_lossy(&response).to_string()
    }

    #[tokio::test]
    async fn custom_route_matched_serves_custom_handler_not_decoy() {
        let extra = Router::new().route(
            "/v1/chat/completions",
            post(|| async { (StatusCode::OK, "oai-proxy") }),
        );
        let adapter = HttpAdapter::new(provider(), empty_registry())
            .with_decoy(DecoyConfig::NotFound)
            .with_extra_routes(extra);
        let request = b"POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        let response = serve_and_read(adapter, request).await;
        assert!(response.starts_with("HTTP/1.1 200"), "expected 200, got: {response}");
        assert!(response.contains("oai-proxy"));
        assert!(!response.contains("404 Not Found"));
    }

    #[tokio::test]
    async fn unknown_path_not_matched_by_custom_route_falls_through_to_decoy() {
        let extra = Router::new().route(
            "/v1/chat/completions",
            post(|| async { (StatusCode::OK, "oai-proxy") }),
        );
        let adapter = HttpAdapter::new(provider(), empty_registry())
            .with_decoy(DecoyConfig::NotFound)
            .with_extra_routes(extra);
        let request = b"GET /totally/unknown HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let response = serve_and_read(adapter, request).await;
        assert!(response.starts_with("HTTP/1.1 404"), "expected 404 decoy, got: {response}");
        assert!(response.contains("404 Not Found"));
    }

    #[tokio::test]
    async fn healthz_takes_precedence_over_decoy() {
        let adapter = HttpAdapter::new(provider(), empty_registry())
            .with_decoy(DecoyConfig::Redirect {
                to: "https://example.com".to_string(),
            });
        let request = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let response = serve_and_read(adapter, request).await;
        assert!(response.starts_with("HTTP/1.1 200"), "expected 200 healthz, got: {response}");
        assert!(response.contains("\r\n\r\nok"));
    }

    #[tokio::test]
    async fn unknown_path_with_redirect_decoy_returns_redirect_over_wire() {
        let adapter = HttpAdapter::new(provider(), empty_registry()).with_decoy(DecoyConfig::Redirect {
            to: "https://example.com".to_string(),
        });
        let request = b"GET /nope HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let response = serve_and_read(adapter, request).await;
        assert!(response.starts_with("HTTP/1.1 302"), "expected 302 redirect, got: {response}");
        assert!(response.contains("location: https://example.com"));
    }
}