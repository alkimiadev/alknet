//! `AlknetEndpoint` — the central runtime type for accepting inbound connections.
//!
//! Takes pre-built transports via builder methods, runs their accept loops
//! inside `run()`, and dispatches each accepted connection to the registered
//! `ProtocolHandler` by ALPN.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::watch;

use alknet_core::auth::IdentityProvider;
use alknet_core::config::DynamicConfig;

use crate::registry::HandlerRegistry;

#[cfg(feature = "tcp")]
pub(crate) type TcpTlsListener = (tokio::net::TcpListener, tokio_rustls::TlsAcceptor);

pub struct AlknetEndpoint {
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_tls: std::sync::Mutex<Option<TcpTlsListener>>,
    handlers: Arc<HandlerRegistry>,
    #[allow(dead_code)]
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    #[allow(dead_code)]
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_tx: watch::Sender<bool>,
    #[allow(dead_code)]
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
}

impl std::fmt::Debug for AlknetEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlknetEndpoint")
            .field("handlers", &self.handlers)
            .field("drain_timeout", &self.drain_timeout)
            .finish()
    }
}

impl AlknetEndpoint {
    pub fn new(
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        drain_timeout: Duration,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            #[cfg(feature = "quinn")]
            quinn: None,
            #[cfg(feature = "iroh")]
            iroh: None,
            #[cfg(feature = "tcp")]
            tcp_tls: std::sync::Mutex::new(None),
            handlers: Arc::new(handlers),
            dynamic,
            identity_provider,
            shutdown_tx,
            shutdown_rx,
            drain_timeout,
        }
    }

    #[cfg(feature = "quinn")]
    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self {
        self.quinn = Some(endpoint);
        self
    }

    #[cfg(feature = "iroh")]
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self {
        self.iroh = Some(endpoint);
        self
    }

    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(
        self,
        listener: tokio::net::TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self {
        *self.tcp_tls.lock().unwrap_or_else(|e| e.into_inner()) = Some((listener, acceptor));
        self
    }

    pub fn shutdown_sender(&self) -> watch::Sender<bool> {
        self.shutdown_tx.clone()
    }

    pub async fn run(self: Arc<Self>) {
        #[allow(unused_mut)]
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            let quinn = quinn.clone();
            let handlers = self.handlers.clone();
            let identity_provider = self.identity_provider.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                crate::accept::quinn::run_accept_loop(
                    quinn,
                    handlers,
                    identity_provider,
                    &mut shutdown_rx,
                )
                .await;
            }));
        }

        #[cfg(feature = "iroh")]
        if let Some(iroh) = &self.iroh {
            let iroh = iroh.clone();
            let handlers = self.handlers.clone();
            let identity_provider = self.identity_provider.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                crate::accept::iroh::run_accept_loop(
                    iroh,
                    handlers,
                    identity_provider,
                    &mut shutdown_rx,
                )
                .await;
            }));
        }

        #[cfg(feature = "tcp")]
        if let Some((listener, acceptor)) = self.tcp_tls.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let handlers = self.handlers.clone();
            let identity_provider = self.identity_provider.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                crate::accept::tcp_tls::run_accept_loop(
                    listener,
                    acceptor,
                    handlers,
                    identity_provider,
                    &mut shutdown_rx,
                )
                .await;
            }));
        }

        for task in tasks {
            let _ = task.await;
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            quinn.close(0u32.into(), b"shutdown");
        }

        #[cfg(feature = "iroh")]
        if let Some(iroh) = &self.iroh {
            iroh.close().await;
        }

        tokio::time::sleep(self.drain_timeout).await;

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            quinn.wait_idle().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
    use alknet_core::config::DynamicConfig;
    #[cfg(feature = "iroh")]
    use alknet_core::auth::AuthContext;
    #[cfg(feature = "iroh")]
    use alknet_core::types::{Connection, HandlerError};
    #[cfg(feature = "iroh")]
    use async_trait::async_trait;

    #[cfg(feature = "iroh")]
    struct DummyHandler {
        alpn: &'static [u8],
    }

    #[cfg(feature = "iroh")]
    #[async_trait]
    impl alknet_core::types::ProtocolHandler for DummyHandler {
        fn alpn(&self) -> &'static [u8] {
            self.alpn
        }
        async fn handle(
            &self,
            _connection: Connection,
            _auth: &AuthContext,
        ) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    #[cfg(feature = "iroh")]
    fn make_handler(alpn: &'static [u8]) -> Arc<dyn alknet_core::types::ProtocolHandler> {
        Arc::new(DummyHandler { alpn })
    }

    struct NoProvider;
    impl IdentityProvider for NoProvider {
        fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
            None
        }
    }

    #[test]
    fn debug_for_alknet_endpoint_is_implemented_without_panicking() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let registry = HandlerRegistry::new();
        let endpoint = AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(10));
        let s = format!("{endpoint:?}");
        assert!(s.contains("AlknetEndpoint"));
        assert!(s.contains("drain_timeout"));
    }

    #[cfg(feature = "iroh")]
    #[tokio::test]
    async fn endpoint_constructs_with_iroh_raw_key_identity() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let mut registry = HandlerRegistry::new();
        registry.register(make_handler(b"alknet/test"));

        let iroh_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(iroh::SecretKey::generate())
            .alpns(vec![b"alknet/test".to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("iroh endpoint binds");

        let endpoint = AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(10))
            .with_iroh(iroh_endpoint);
        assert!(endpoint.shutdown_sender().send(true).is_ok());
        endpoint.shutdown().await;
    }

    #[cfg(feature = "iroh")]
    #[tokio::test]
    async fn iroh_endpoint_runs_accept_loop_and_shutdown() {
        use std::sync::Mutex;
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));

        let connected = Arc::new(Mutex::new(false));
        let connected_clone = connected.clone();
        struct CountingHandler {
            alpn: &'static [u8],
            connected: Arc<Mutex<bool>>,
        }
        #[async_trait]
        impl alknet_core::types::ProtocolHandler for CountingHandler {
            fn alpn(&self) -> &'static [u8] {
                self.alpn
            }
            async fn handle(
                &self,
                _conn: Connection,
                _auth: &AuthContext,
            ) -> Result<(), HandlerError> {
                *self.connected.lock().unwrap() = true;
                Ok(())
            }
        }
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(CountingHandler {
            alpn: b"alknet/test",
            connected: connected_clone,
        }));

        let iroh_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(iroh::SecretKey::generate())
            .alpns(vec![b"alknet/test".to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("iroh endpoint binds");

        let endpoint = Arc::new(
            AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(20))
                .with_iroh(iroh_endpoint),
        );

        let run_endpoint = endpoint.clone();
        let run_task = tokio::spawn(async move {
            run_endpoint.run().await;
        });

        let _ = endpoint.shutdown_sender().send(true);
        endpoint.shutdown().await;
        let _ = run_task.await;
        assert!(!*connected.lock().unwrap());
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn with_iroh_sets_field() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let registry = HandlerRegistry::new();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let iroh_endpoint = rt.block_on(async {
            iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
                .secret_key(iroh::SecretKey::generate())
                .alpns(vec![b"alknet/test".to_vec()])
                .relay_mode(iroh::RelayMode::Disabled)
                .bind()
                .await
                .expect("iroh endpoint binds")
        });

        let endpoint = AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(10))
            .with_iroh(iroh_endpoint);
        assert!(endpoint.iroh.is_some());
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn without_iroh_field_is_none() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let registry = HandlerRegistry::new();
        let endpoint = AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(10));
        assert!(endpoint.iroh.is_none());
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn endpoint_works_without_iroh() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let mut registry = HandlerRegistry::new();
        registry.register(make_handler(b"alknet/test"));
        let endpoint = AlknetEndpoint::new(registry, dynamic, provider, Duration::from_millis(10));
        assert!(endpoint.iroh.is_none());
        assert!(endpoint.shutdown_sender().send(true).is_ok());
    }
}
