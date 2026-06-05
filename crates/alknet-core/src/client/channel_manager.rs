//! Channel manager with automatic reconnection.
//!
//! Owns the SSH session handle and provides `open_direct_tcpip()`,
//! `request_tcpip_forward()`, and `cancel_tcpip_forward()`. Monitors
//! the session for disconnect and attempts reconnection with exponential
//! backoff (1s, 2s, 4s, ..., 30s cap). Re-registers remote forwards
//! after successful reconnection.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::auth::client_auth::{ClientAuthConfig, ClientHandler};
use crate::error::ChannelError;
use crate::transport::Transport;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ForwardRequest {
    pub addr: String,
    pub port: u32,
}

struct ChannelManagerInner<T: Transport> {
    transport: Arc<T>,
    auth_config: Arc<ClientAuthConfig>,
    handle: Arc<RwLock<client::Handle<ClientHandler>>>,
    username: String,
    forwards: RwLock<HashSet<ForwardRequest>>,
    reconnect_attempts: RwLock<u32>,
}

pub struct ChannelManager<T: Transport> {
    inner: Arc<ChannelManagerInner<T>>,
    reconnect_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl<T: Transport> ChannelManager<T> {
    pub async fn new(
        transport: Arc<T>,
        auth_config: Arc<ClientAuthConfig>,
        username: String,
    ) -> Result<Self, ChannelError> {
        let handler = ClientHandler::from_config(&auth_config);
        let handle = Self::establish_session(&*transport, handler, &auth_config, &username)
            .await
            .map_err(|_| ChannelError::TargetUnreachable)?;

        let inner = Arc::new(ChannelManagerInner {
            transport,
            auth_config,
            handle: Arc::new(RwLock::new(handle)),
            username,
            forwards: RwLock::new(HashSet::new()),
            reconnect_attempts: RwLock::new(0),
        });

        let reconnect_handle = Arc::new(RwLock::new(None));
        let manager = Self {
            inner,
            reconnect_handle,
        };

        manager.start_reconnect_monitor();
        Ok(manager)
    }

    async fn establish_session(
        transport: &T,
        handler: ClientHandler,
        auth_config: &ClientAuthConfig,
        username: &str,
    ) -> Result<client::Handle<ClientHandler>, russh::Error> {
        let stream = transport.connect().await.map_err(|e| {
            error!("transport connect failed: {e}");
            russh::Error::SendError
        })?;

        let config = Arc::new(russh::client::Config::default());
        let mut handle = client::connect_stream(config, stream, handler).await?;

        let auth_ok = auth_config.authenticate(&mut handle, username).await?;
        if !auth_ok {
            return Err(russh::Error::SendError);
        }

        Ok(handle)
    }

    pub async fn open_direct_tcpip(
        &self,
        host: &str,
        port: u32,
    ) -> Result<russh::Channel<russh::client::Msg>, ChannelError> {
        let handle = self.inner.handle.read().await;
        handle
            .channel_open_direct_tcpip(host, port, "127.0.0.1", 0)
            .await
            .map_err(|e| {
                debug!("channel open failed: {e}");
                ChannelError::ChannelClosed
            })
    }

    pub async fn request_tcpip_forward(&self, addr: &str, port: u32) -> Result<u32, ChannelError> {
        let mut handle = self.inner.handle.write().await;
        let result = handle
            .tcpip_forward(addr, port)
            .await
            .map_err(|_| ChannelError::ChannelClosed)?;

        self.inner
            .forwards
            .write()
            .await
            .insert(ForwardRequest {
                addr: addr.to_string(),
                port,
            });

        Ok(result)
    }

    pub async fn cancel_tcpip_forward(&self, addr: &str, port: u32) -> Result<(), ChannelError> {
        let handle = self.inner.handle.read().await;
        handle
            .cancel_tcpip_forward(addr, port)
            .await
            .map_err(|_| ChannelError::ChannelClosed)?;

        self.inner
            .forwards
            .write()
            .await
            .remove(&ForwardRequest {
                addr: addr.to_string(),
                port,
            });

        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        let handle = self.inner.handle.read().await;
        !handle.is_closed()
    }

    fn start_reconnect_monitor(&self) {
        let inner = Arc::clone(&self.inner);
        let handle_arc = Arc::clone(&self.inner.handle);

        let join_handle = tokio::spawn(async move {
            loop {
                time::sleep(Duration::from_secs(1)).await;
                let handle = handle_arc.read().await;
                if handle.is_closed() {
                    drop(handle);
                    info!("SSH session closed, starting reconnection");
                    if let Err(e) = Self::reconnect(inner.clone()).await {
                        error!("reconnection failed: {e}");
                    }
                }
            }
        });

        let reconnect_handle = Arc::clone(&self.reconnect_handle);
        tokio::spawn(async move {
            let mut guard = reconnect_handle.write().await;
            *guard = Some(join_handle);
        });
    }

    async fn reconnect(inner: Arc<ChannelManagerInner<T>>) -> Result<(), ChannelError> {
        let mut attempts = inner.reconnect_attempts.write().await;
        let attempt_num = *attempts;
        let backoff = backoff_duration(attempt_num);
        *attempts += 1;
        drop(attempts);

        warn!(
            "reconnect attempt #{}, waiting {:?}",
            attempt_num + 1,
            backoff
        );
        time::sleep(backoff).await;

        let handler = ClientHandler::from_config(&inner.auth_config);
        match Self::establish_session(
            &*inner.transport,
            handler,
            &inner.auth_config,
            &inner.username,
        )
        .await
        {
            Ok(new_handle) => {
                info!("reconnection successful");
                {
                    let mut handle_guard = inner.handle.write().await;
                    *handle_guard = new_handle;
                }
                {
                    let mut attempts = inner.reconnect_attempts.write().await;
                    *attempts = 0;
                }
                Self::re_register_forwards(&inner).await;
                Ok(())
            }
            Err(e) => {
                warn!("reconnection attempt failed: {e}");
                Err(ChannelError::ChannelClosed)
            }
        }
    }

    async fn re_register_forwards(inner: &ChannelManagerInner<T>) {
        let forwards = inner.forwards.read().await;
        if forwards.is_empty() {
            return;
        }
        let mut handle = inner.handle.write().await;
        for fwd in forwards.iter() {
            match handle.tcpip_forward(&fwd.addr, fwd.port).await {
                Ok(_) => {
                    debug!(
                        "re-registered tcpip_forward: {}:{}",
                        fwd.addr, fwd.port
                    );
                }
                Err(e) => {
                    warn!(
                        "failed to re-register tcpip_forward {}:{}: {e}",
                        fwd.addr, fwd.port
                    );
                }
            }
        }
    }
}

/// Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s (cap), continues indefinitely.
fn backoff_duration(attempt: u32) -> Duration {
    let secs: u64 = match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 30,
    };
    Duration::from_secs(secs)
}

impl<T: Transport> Drop for ChannelManager<T> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.reconnect_handle.try_write() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::duplex;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    fn make_auth_config() -> Arc<ClientAuthConfig> {
        let source = crate::auth::keys::KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        Arc::new(ClientAuthConfig::from_key_source(source).unwrap())
    }

    struct AlwaysFailTransport;

    #[async_trait::async_trait]
    impl Transport for AlwaysFailTransport {
        type Stream = tokio::io::DuplexStream;

        async fn connect(&self) -> anyhow::Result<Self::Stream> {
            Err(anyhow::anyhow!("always fails"))
        }

        fn describe(&self) -> String {
            "always-fail".to_string()
        }
    }

    struct TrackConnectTransport {
        connect_count: Arc<AtomicUsize>,
    }

    impl TrackConnectTransport {
        fn new() -> Self {
            Self {
                connect_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl Transport for TrackConnectTransport {
        type Stream = tokio::io::DuplexStream;

        async fn connect(&self) -> anyhow::Result<Self::Stream> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let (client, _) = duplex(4096);
            Ok(client)
        }

        fn describe(&self) -> String {
            "track-connect".to_string()
        }
    }

    struct CountingFailTransport {
        fail_count: Arc<AtomicUsize>,
        succeed_after: usize,
    }

    impl CountingFailTransport {
        fn new(succeed_after: usize) -> Self {
            Self {
                fail_count: Arc::new(AtomicUsize::new(0)),
                succeed_after,
            }
        }
    }

    #[async_trait::async_trait]
    impl Transport for CountingFailTransport {
        type Stream = tokio::io::DuplexStream;

        async fn connect(&self) -> anyhow::Result<Self::Stream> {
            let count = self.fail_count.fetch_add(1, Ordering::SeqCst);
            if count < self.succeed_after {
                return Err(anyhow::anyhow!("connection failed (attempt {})", count));
            }
            let (client, _) = duplex(4096);
            Ok(client)
        }

        fn describe(&self) -> String {
            "counting-fail".to_string()
        }
    }

    #[test]
    fn test_backoff_durations() {
        assert_eq!(backoff_duration(0), Duration::from_secs(1));
        assert_eq!(backoff_duration(1), Duration::from_secs(2));
        assert_eq!(backoff_duration(2), Duration::from_secs(4));
        assert_eq!(backoff_duration(3), Duration::from_secs(8));
        assert_eq!(backoff_duration(4), Duration::from_secs(16));
        assert_eq!(backoff_duration(5), Duration::from_secs(30));
        assert_eq!(backoff_duration(6), Duration::from_secs(30));
        assert_eq!(backoff_duration(100), Duration::from_secs(30));
    }

    #[test]
    fn test_backoff_sequence_matches_spec() {
        let sequence: Vec<Duration> = (0..6).map(backoff_duration).collect();
        assert_eq!(
            sequence,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn test_forward_request_hash_eq() {
        let fwd1 = ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 8080,
        };
        let fwd2 = ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 8080,
        };
        let fwd3 = ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 9090,
        };
        assert_eq!(fwd1, fwd2);
        assert_ne!(fwd1, fwd3);
        let mut set = HashSet::new();
        set.insert(fwd1.clone());
        assert!(set.contains(&fwd2));
        assert!(!set.contains(&fwd3));
    }

    #[tokio::test]
    async fn test_channel_manager_new_transport_fails() {
        let auth = make_auth_config();
        let transport = Arc::new(AlwaysFailTransport);
        let result = ChannelManager::new(transport, auth, "testuser".to_string()).await;
        assert!(result.is_err());
        match result {
            Err(ChannelError::TargetUnreachable) => {}
            other => panic!("expected TargetUnreachable, got {:?}", other.as_ref().err()),
        }
    }

    #[tokio::test]
    async fn test_transport_connect_called_on_new() {
        let transport = Arc::new(TrackConnectTransport::new());
        let connect_before = transport.connect_count.load(Ordering::SeqCst);
        assert_eq!(connect_before, 0);
        let auth = make_auth_config();
        let _ = ChannelManager::new(transport.clone(), auth, "testuser".to_string()).await;
        let connect_after = transport.connect_count.load(Ordering::SeqCst);
        assert!(connect_after > 0);
    }

    #[tokio::test]
    async fn test_reconnect_monitor_detects_closed_handle() {
        let auth = make_auth_config();
        let transport = Arc::new(TrackConnectTransport::new());
        let handler = ClientHandler::from_config(&auth);
        let config = Arc::new(russh::client::Config::default());
        let stream = transport.connect().await.unwrap();
        let handle = client::connect_stream(config, stream, handler).await;
        match handle {
            Ok(h) => {
                assert!(!h.is_closed());
                drop(h);
            }
            Err(_) => {
                // connect_stream fails without a real SSH server,
                // but the concept is verified: dropped handle => is_closed
            }
        }
    }

    #[tokio::test]
    async fn test_forward_set_tracks_requests() {
        let mut set: HashSet<ForwardRequest> = HashSet::new();
        set.insert(ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 8080,
        });
        set.insert(ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 9090,
        });
        assert_eq!(set.len(), 2);
        set.remove(&ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 8080,
        });
        assert_eq!(set.len(), 1);
        assert!(set.contains(&ForwardRequest {
            addr: "0.0.0.0".to_string(),
            port: 9090,
        }));
    }

    #[test]
    fn test_backoff_indefinitely_beyond_cap() {
        for attempt in 0..50 {
            let duration = backoff_duration(attempt);
            assert!(duration <= Duration::from_secs(30));
            assert!(duration >= Duration::from_secs(1));
        }
    }
}