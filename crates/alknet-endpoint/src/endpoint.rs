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
