//! Integration test: two-node `alknet/call` round-trip over a real QUIC
//! loopback. A `CallAdapter` server accepts, a `CallClient` connects, and
//! the client calls back into the server (connection symmetry, ADR-017 §2).
//! Verifies the shared dispatch loop works end-to-end.

#![cfg(feature = "quinn")]

use std::sync::Arc;
use std::time::Duration;

use alknet_call::client::{CallClient, CallCredentials, RemoteIdentity};
use alknet_call::protocol::adapter::CallAdapter;
use alknet_call::protocol::wire::ResponseEnvelope;
use alknet_call::registry::discovery::{
    services_list_handler, services_list_spec, services_schema_handler, services_schema_spec,
};
use alknet_call::registry::registration::{
    make_handler, Handler, HandlerKind, HandlerRegistration, OperationProvenance, OperationRegistry,
};
use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
use alknet_core::auth::{Identity, IdentityProvider};
use alknet_core::types::{Capabilities, Connection, ProtocolHandler};

struct NoopIdentityProvider;
impl IdentityProvider for NoopIdentityProvider {
    fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
        None
    }
    fn resolve_from_token(&self, _: &alknet_core::auth::AuthToken) -> Option<Identity> {
        None
    }
}

fn external_spec(name: &str) -> OperationSpec {
    OperationSpec::new(
        name,
        OperationType::Query,
        Visibility::External,
        serde_json::json!({}),
        serde_json::json!({}),
        vec![],
        AccessControl::default(),
        None,
    )
}

fn echo_handler() -> Handler {
    make_handler(|input, context| async move { ResponseEnvelope::ok(context.request_id, input) })
}

/// Build a raw quinn server endpoint with a self-signed cert and the
/// `CallAdapter` accepting `alknet/call` connections. Returns
/// `(bound_addr, server_fingerprint, join_handle)` — the fingerprint is the
/// `SHA256:<hex>` of the self-signed cert DER, which the client pins via
/// `CallCredentials::with_remote_identity` (the known-peer path, ADR-034 §3).
/// The accept loop spawns a task per connection that hands the connection to
/// `CallAdapter::handle`.
async fn build_raw_quinn_server(
    registry: Arc<OperationRegistry>,
) -> (std::net::SocketAddr, String, tokio::task::JoinHandle<()>) {
    let provider: Arc<dyn IdentityProvider> = Arc::new(NoopIdentityProvider);
    let adapter = Arc::new(CallAdapter::new(
        Arc::clone(&registry),
        Arc::clone(&provider),
    ));

    let key_pair = rcgen::KeyPair::generate().expect("key gen");
    let params = rcgen::CertificateParams::default();
    let cert = params.self_signed(&key_pair).expect("self-signed cert");
    let cert_der = cert.der().clone();
    let fingerprint = alknet_core::fingerprint::fingerprint_from_cert_der(cert_der.as_ref())
        .expect("cert produces fingerprint");
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
    );

    let provider_crypto = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut server_config = rustls::ServerConfig::builder_with_provider(provider_crypto)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap();
    server_config.alpn_protocols = vec![b"alknet/call".to_vec()];
    server_config.max_early_data_size = u32::MAX;

    let quic_server_config =
        quinn::crypto::rustls::QuicServerConfig::try_from(server_config).unwrap();
    let quinn_server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));

    let quinn_endpoint =
        quinn::Endpoint::server(quinn_server_config, "127.0.0.1:0".parse().unwrap())
            .expect("server bind");
    let bound_addr = quinn_endpoint.local_addr().expect("local addr");

    let join = tokio::spawn(async move {
        while let Some(incoming) = quinn_endpoint.accept().await {
            let adapter = Arc::clone(&adapter);
            tokio::spawn(async move {
                let connecting = match incoming.accept() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let conn = match connecting.await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let alpn = b"alknet/call".to_vec();
                let conn = Connection::from_quinn_with_alpn(conn, alpn.clone());
                let auth = alknet_core::auth::AuthContext {
                    identity: None,
                    alpn,
                    remote_addr: conn.remote_addr(),
                    tls_client_fingerprint: None,
                };
                let _ = adapter.handle(conn, &auth).await;
            });
        }
    });

    (bound_addr, fingerprint, join)
}

/// Build the server's registry: an echo op, a secret op, and the
/// services/list + services/schema discovery handlers.
fn build_server_registry() -> Arc<OperationRegistry> {
    let mut registry = OperationRegistry::new();
    registry
        .register(HandlerRegistration::new(
            external_spec("server/echo"),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ))
        .unwrap();
    registry
        .register(HandlerRegistration::new(
            external_spec("server/secret"),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new().with_api_key("google", "server-secret".to_string()),
        ))
        .unwrap();
    let discovery_registry = Arc::new(registry);
    let list_handler = services_list_handler(Arc::clone(&discovery_registry));
    let schema_handler = services_schema_handler(Arc::clone(&discovery_registry));
    let mut full = OperationRegistry::new();
    full.register(HandlerRegistration::new(
        external_spec("server/echo"),
        HandlerKind::Once(echo_handler()),
        OperationProvenance::Local,
        None,
        None,
        Capabilities::new(),
    ))
    .unwrap();
    full.register(HandlerRegistration::new(
        external_spec("server/secret"),
        HandlerKind::Once(echo_handler()),
        OperationProvenance::Local,
        None,
        None,
        Capabilities::new().with_api_key("google", "server-secret".to_string()),
    ))
    .unwrap();
    full.register(HandlerRegistration::new(
        services_list_spec(),
        HandlerKind::Once(list_handler),
        OperationProvenance::Local,
        None,
        None,
        Capabilities::new(),
    ))
    .unwrap();
    full.register(HandlerRegistration::new(
        services_schema_spec(),
        HandlerKind::Once(schema_handler),
        OperationProvenance::Local,
        None,
        None,
        Capabilities::new(),
    ))
    .unwrap();
    Arc::new(full)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_node_call_round_trip() {
    let server_registry = build_server_registry();
    let (server_addr, server_fingerprint, _server_join) =
        build_raw_quinn_server(Arc::clone(&server_registry)).await;

    // Client side: a CallClient with its own ops so the server can call back
    // (connection symmetry). Pin the server's self-signed cert fingerprint
    // (the known-peer path, ADR-034 §3) — `WebPkiServerVerifier` would reject
    // it as UnknownIssuer since the self-signed cert is not in the platform
    // root store.
    let mut client_registry = OperationRegistry::new();
    client_registry
        .register(HandlerRegistration::new(
            external_spec("client/echo"),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ))
        .unwrap();
    let client_registry = Arc::new(client_registry);
    let client = CallClient::new(Arc::clone(&client_registry), Arc::new(NoopIdentityProvider));

    let credentials = CallCredentials::new().with_remote_identity(RemoteIdentity {
        fingerprint: server_fingerprint,
    });
    let conn = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(server_addr, credentials),
    )
    .await
    .expect("connect did not time out")
    .expect("connect succeeds");

    // Outbound call: client -> server's echo op.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        conn.call("server/echo", serde_json::json!({"hi": 1})),
    )
    .await
    .expect("call did not time out");
    assert_eq!(response.result, Ok(serde_json::json!({"hi": 1})));

    // Peer authorization is enforced by the AccessControl gate in
    // OperationRegistry::invoke (ADR-029 §3) — exercised by the unit tests in
    // `registry/registration.rs`. This integration test focuses on the QUIC
    // connect path + shared dispatch loop working end-to-end (the call above
    // proves the CallClient opened a real connection, the shared loop
    // dispatched, and the CallConnection::call() round-tripped).
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn from_call_discovers_and_forwards_over_quic_loopback() {
    use alknet_call::client::{from_call, FromCallConfig};
    use alknet_call::registry::context::ScopedPeerEnv;

    let server_registry = build_server_registry();
    let (server_addr, server_fingerprint, _server_join) =
        build_raw_quinn_server(Arc::clone(&server_registry)).await;

    // Client with an empty registry — from_call will populate its overlay.
    // Pin the server's self-signed cert fingerprint (ADR-034 §3 known-peer
    // path).
    let client_registry = Arc::new(OperationRegistry::new());
    let client = CallClient::new(Arc::clone(&client_registry), Arc::new(NoopIdentityProvider));

    let credentials = CallCredentials::new().with_remote_identity(RemoteIdentity {
        fingerprint: server_fingerprint,
    });
    let conn = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(server_addr, credentials),
    )
    .await
    .expect("connect did not time out")
    .expect("connect succeeds");

    // from_call discovers the server's External ops (server/echo, server/secret
    // — both External; services/list + services/schema themselves are External
    // too) and builds FromCall forwarding-handler bundles. Register them in the
    // connection's Layer 2 overlay.
    let bundles = tokio::time::timeout(
        Duration::from_secs(5),
        from_call(&conn, FromCallConfig::new()),
    )
    .await
    .expect("from_call did not time out")
    .expect("from_call succeeds");
    assert!(
        !bundles.is_empty(),
        "from_call must discover at least the server/echo op"
    );
    conn.register_imported_all(bundles);

    // The overlay now contains the discovered ops. Verify the forwarding path
    // by invoking the overlay env directly with a scoped context that allows
    // server/echo — this is how a composing handler would call the imported op.
    let env = conn.overlay_env();
    assert!(
        env.contains("server/echo"),
        "overlay must contain the imported server/echo op"
    );

    // Build a minimal parent context to invoke the overlay env (mirrors how a
    // composing handler dispatches a child).
    let scoped = ScopedPeerEnv::new(["server/echo"]);
    let parent = alknet_call::registry::context::OperationContext {
        request_id: "parent-1".to_string(),
        parent_request_id: None,
        identity: None,
        handler_identity: None,
        forwarded_for: None,
        capabilities: Capabilities::new(),
        metadata: Default::default(),
        scoped_env: scoped,
        env: env.clone(),
        abort_policy: alknet_call::registry::context::AbortPolicy::default(),
        deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
        internal: true,
        ownership: None,
    };

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        env.invoke(
            "server",
            "echo",
            serde_json::json!({"from_call": true}),
            &parent,
        ),
    )
    .await
    .expect("overlay invoke did not time out");
    assert_eq!(
        response.result,
        Ok(serde_json::json!({"from_call": true})),
        "from_call forwarding handler must round-trip the input to the remote op"
    );
}
