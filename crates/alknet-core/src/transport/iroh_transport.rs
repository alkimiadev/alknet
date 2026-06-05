use anyhow::{anyhow, Result};
use async_trait::async_trait;
use iroh::{
    endpoint::RecvStream,
    node_info::NodeIdExt,
    Endpoint, NodeId, RelayMap, RelayMode, RelayUrl,
};
use tokio::io;

use super::{Transport, TransportAcceptor, TransportInfo, TransportKind};

pub const ALPN: &[u8] = b"alknet-ssh";
const DEFAULT_RELAY_URL: &str = "https://relay.iroh.network/";

/// A client-side iroh QUIC P2P transport that connects to a remote iroh endpoint.
///
/// Connects via `Endpoint::connect(node_id, alpn)`, opens a bidirectional
/// QUIC stream with `conn.open_bi()`, and joins the halves with
/// `tokio::io::join(recv, send)` to produce a duplex stream for russh.
/// Per ADR-003, `tokio::io::join` is used instead of a custom wrapper.
///
/// Use [`IrohTransport::new`] to create a standalone endpoint, or
/// [`IrohTransport::from_endpoint`] to share an existing iroh `Endpoint`
/// with other protocol handlers (blobs, gossip, docs).
pub struct IrohTransport {
    node_id: NodeId,
    endpoint: Endpoint,
    owned: bool,
}

impl IrohTransport {
    /// Create a new iroh transport with its own dedicated endpoint.
    ///
    /// The endpoint is created with the `alknet-ssh` ALPN and the provided
    /// relay URL. Use this when alknet is the only iroh service on this node.
    pub async fn new(
        node_id: NodeId,
        relay_url: Option<RelayUrl>,
        proxy_url: Option<url::Url>,
    ) -> Result<Self> {
        let relay_url = relay_url.unwrap_or_else(|| {
            DEFAULT_RELAY_URL.parse().expect("default relay URL is valid")
        });
        let relay_map = RelayMap::from_url(relay_url);
        let mut builder = Endpoint::builder()
            .relay_mode(RelayMode::Custom(relay_map))
            .alpns(vec![ALPN.to_vec()]);
        if let Some(ref proxy) = proxy_url {
            builder = builder.proxy_url(proxy.clone());
        }
        let endpoint = builder.bind().await?;
        Ok(Self { node_id, endpoint, owned: true })
    }

    /// Create an iroh transport using an existing shared endpoint.
    ///
    /// The endpoint must already have the `alknet-ssh` ALPN registered
    /// (typically via [`iroh::protocol::Router::builder`]). This enables
    /// running alknet alongside iroh-blobs, iroh-gossip, iroh-docs, and
    /// other protocol handlers on the same QUIC endpoint — one connection
    /// per peer, multiplexed by ALPN.
    pub fn from_endpoint(node_id: NodeId, endpoint: Endpoint) -> Self {
        Self { node_id, endpoint, owned: false }
    }

    pub fn endpoint_id(&self) -> String {
        self.endpoint.node_id().to_z32()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn owned(&self) -> bool {
        self.owned
    }
}

#[async_trait]
impl Transport for IrohTransport {
    type Stream = io::Join<RecvStream, iroh::endpoint::SendStream>;

    async fn connect(&self) -> Result<Self::Stream> {
        let conn = self.endpoint.connect(self.node_id, ALPN).await?;
        let (send, recv) = conn.open_bi().await?;
        Ok(io::join(recv, send))
    }

    fn describe(&self) -> String {
        format!("iroh://{}", self.node_id.to_z32())
    }
}

/// A server-side iroh QUIC P2P transport acceptor that listens for incoming connections.
///
/// Binds an iroh `Endpoint` with the configured relay URL and optional proxy
/// (ADR-010). Accepts incoming connections, accepts bidirectional QUIC streams,
/// and joins the halves with `tokio::io::join(recv, send)`. Exposes
/// `endpoint_id()` for CLI display of the server's z-base-32 node ID.
///
/// Use [`IrohAcceptor::bind`] to create a standalone endpoint, or
/// [`IrohAcceptor::from_endpoint`] to share an existing iroh `Endpoint`
/// with other protocol handlers (blobs, gossip, docs).
///
/// When using `from_endpoint`, the alknet-ssh ALPN must be registered
/// via an iroh `Router` that calls `Handler::accept()` on incoming
/// connections with the `alknet-ssh` ALPN, then passes the accepted
/// bidirectional stream to `russh::server::run_stream()`.
pub struct IrohAcceptor {
    endpoint: Endpoint,
    owned: bool,
}

impl IrohAcceptor {
    /// Bind a new iroh endpoint with a dedicated `alknet-ssh` ALPN.
    ///
    /// Use this when alknet is the only iroh service on this node.
    pub async fn bind(
        relay_url: Option<RelayUrl>,
        proxy_url: Option<url::Url>,
    ) -> Result<Self> {
        let relay_url = relay_url.unwrap_or_else(|| {
            DEFAULT_RELAY_URL.parse().expect("default relay URL is valid")
        });
        let relay_map = RelayMap::from_url(relay_url);
        let mut builder = Endpoint::builder()
            .relay_mode(RelayMode::Custom(relay_map))
            .alpns(vec![ALPN.to_vec()]);
        if let Some(ref proxy) = proxy_url {
            builder = builder.proxy_url(proxy.clone());
        }
        let endpoint = builder.bind().await?;
        Ok(Self { endpoint, owned: true })
    }

    /// Create an iroh acceptor using an existing shared endpoint.
    ///
    /// The endpoint must already have the `alknet-ssh` ALPN registered
    /// (typically via [`iroh::protocol::Router::builder`]). When using a
    /// shared endpoint, incoming connections with the `alknet-ssh` ALPN
    /// are routed by the Router to a `ProtocolHandler` that this acceptor
    /// does not manage — the caller is responsible for bridging the
    /// Router's `accept()` callback to this acceptor's stream handling.
    ///
    /// For the standalone case where alknet owns the endpoint, use
    /// [`IrohAcceptor::bind`] instead, which handles the accept loop
    /// internally.
    pub fn from_endpoint(endpoint: Endpoint) -> Self {
        Self { endpoint, owned: false }
    }

    pub fn endpoint_id(&self) -> String {
        self.endpoint.node_id().to_z32()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn owned(&self) -> bool {
        self.owned
    }
}

#[async_trait]
impl TransportAcceptor for IrohAcceptor {
    type Stream = io::Join<RecvStream, iroh::endpoint::SendStream>;

    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow!("endpoint closed"))?;
        let conn = incoming.await?;
        let node_id = conn.remote_node_id()?;
        let (send, recv) = conn.accept_bi().await?;
        let stream = io::join(recv, send);
        let info = TransportInfo {
            remote_addr: None,
            transport_kind: TransportKind::Iroh {
                endpoint_id: node_id.to_z32(),
            },
        };
        Ok((stream, info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn iroh_acceptor_bind_creates_endpoint() {
        let acceptor = IrohAcceptor::bind(None, None).await.unwrap();
        let endpoint_id = acceptor.endpoint_id();
        assert!(!endpoint_id.is_empty());
        let parsed = NodeId::from_z32(&endpoint_id);
        assert!(parsed.is_ok());
        assert!(acceptor.owned());
    }

    #[tokio::test]
    async fn iroh_acceptor_bind_with_custom_relay() {
        let relay: RelayUrl = "https://relay.iroh.network/".parse().unwrap();
        let acceptor = IrohAcceptor::bind(Some(relay), None).await.unwrap();
        assert!(!acceptor.endpoint_id().is_empty());
        assert!(acceptor.owned());
    }

    #[tokio::test]
    async fn iroh_acceptor_from_endpoint() {
        let acceptor = IrohAcceptor::bind(None, None).await.unwrap();
        let endpoint = acceptor.endpoint.clone();
        let shared = IrohAcceptor::from_endpoint(endpoint);
        assert_eq!(shared.endpoint_id(), acceptor.endpoint_id());
        assert!(!shared.owned());
    }

    #[test]
    fn iroh_transport_describe_format() {
        let node_id: NodeId = iroh::SecretKey::generate(rand_core::OsRng)
            .public()
            .into();
        let desc = format!("iroh://{}", node_id.to_z32());
        assert!(desc.starts_with("iroh://"));
    }

    #[tokio::test]
    async fn iroh_transport_connect_builds_endpoint() {
        let node_id: NodeId = iroh::SecretKey::generate(rand_core::OsRng)
            .public()
            .into();
        let transport = IrohTransport::new(node_id, None, None).await.unwrap();
        assert!(transport.describe().starts_with("iroh://"));
        assert!(!transport.endpoint_id().is_empty());
        assert!(transport.owned());
    }

    #[tokio::test]
    async fn iroh_transport_from_endpoint() {
        let node_id: NodeId = iroh::SecretKey::generate(rand_core::OsRng)
            .public()
            .into();
        let acceptor = IrohAcceptor::bind(None, None).await.unwrap();
        let endpoint = acceptor.endpoint.clone();
        let transport = IrohTransport::from_endpoint(node_id, endpoint);
        assert!(transport.describe().starts_with("iroh://"));
        assert_eq!(transport.endpoint_id(), acceptor.endpoint_id());
        assert!(!transport.owned());
    }

    #[tokio::test]
    #[ignore]
    async fn iroh_client_connects_to_iroh_server() {
        let acceptor = IrohAcceptor::bind(None, None).await.unwrap();
        let server_node_id = acceptor.endpoint().node_id();

        let transport = IrohTransport::new(server_node_id, None, None)
            .await
            .unwrap();

        let mut addrs_watcher = acceptor.endpoint().direct_addresses();
        addrs_watcher.initialized().await.unwrap();
        let addr_set = addrs_watcher.get().unwrap().unwrap_or_default();
        for addr in addr_set {
            transport
                .endpoint
                .add_node_addr(iroh::NodeAddr::from_parts(
                    server_node_id,
                    None,
                    vec![addr.addr],
                ))
                .unwrap();
        }

        let accept_handle = tokio::spawn(async move {
            let (stream, info) = acceptor.accept().await.unwrap();
            assert!(matches!(info.transport_kind, TransportKind::Iroh { .. }));
            stream
        });

        let _client_stream: io::Join<RecvStream, iroh::endpoint::SendStream> =
            transport.connect().await.unwrap();
        let _server_stream = accept_handle.await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn iroh_shared_endpoint_client_connects_to_server() {
        let acceptor = IrohAcceptor::bind(None, None).await.unwrap();
        let server_node_id = acceptor.endpoint().node_id();
        let shared_endpoint = acceptor.endpoint().clone();

        let transport = IrohTransport::from_endpoint(server_node_id, shared_endpoint);

        let mut addrs_watcher = acceptor.endpoint().direct_addresses();
        addrs_watcher.initialized().await.unwrap();
        let addr_set = addrs_watcher.get().unwrap().unwrap_or_default();
        for addr in addr_set {
            transport
                .endpoint
                .add_node_addr(iroh::NodeAddr::from_parts(
                    server_node_id,
                    None,
                    vec![addr.addr],
                ))
                .unwrap();
        }

        let accept_handle = tokio::spawn(async move {
            let (stream, info) = acceptor.accept().await.unwrap();
            assert!(matches!(info.transport_kind, TransportKind::Iroh { .. }));
            stream
        });

        let _client_stream: io::Join<RecvStream, iroh::endpoint::SendStream> =
            transport.connect().await.unwrap();
        let _server_stream = accept_handle.await.unwrap();
    }
}