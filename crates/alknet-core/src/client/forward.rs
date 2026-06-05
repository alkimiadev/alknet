//! Local and remote port forwarding.
//!
//! `LocalForwarder` binds a local TCP listener and forwards each connection through
//! an SSH `direct-tcpip` channel. `RemoteForwarder` requests `tcpip-forward` from
//! the server and handles `forwarded-tcpip` channels. Specs follow the
//! `bind_addr:bind_port:target_host:target_port` format.

use std::net::SocketAddr;
use std::sync::Arc;

use russh::client;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::error::ForwardError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortForwardSpecKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardSpec {
    pub kind: PortForwardSpecKind,
    pub bind_addr: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

impl PortForwardSpec {
    pub fn local(spec: &str) -> Result<Self, ForwardError> {
        let (bind_addr, bind_port, target_host, target_port) = parse_spec(spec)?;
        Ok(Self {
            kind: PortForwardSpecKind::Local,
            bind_addr,
            bind_port,
            target_host,
            target_port,
        })
    }

    pub fn remote(spec: &str) -> Result<Self, ForwardError> {
        let (bind_addr, bind_port, target_host, target_port) = parse_spec(spec)?;
        Ok(Self {
            kind: PortForwardSpecKind::Remote,
            bind_addr,
            bind_port,
            target_host,
            target_port,
        })
    }

    pub fn listen_addr(&self) -> Result<SocketAddr, ForwardError> {
        format!("{}:{}", self.bind_addr, self.bind_port)
            .parse()
            .map_err(|_| ForwardError::InvalidSpec {
                spec: format!("{}:{}", self.bind_addr, self.bind_port),
            })
    }

    pub fn target_addr(&self) -> Result<SocketAddr, ForwardError> {
        format!("{}:{}", self.target_host, self.target_port)
            .parse()
            .map_err(|_| ForwardError::InvalidSpec {
                spec: format!("{}:{}", self.target_host, self.target_port),
            })
    }
}

impl std::fmt::Display for PortForwardSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.kind {
            PortForwardSpecKind::Local => "-L",
            PortForwardSpecKind::Remote => "-R",
        };
        write!(
            f,
            "{} {}:{}:{}:{}",
            prefix, self.bind_addr, self.bind_port, self.target_host, self.target_port
        )
    }
}

fn parse_spec(spec: &str) -> Result<(String, u16, String, u16), ForwardError> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() != 4 {
        return Err(ForwardError::InvalidSpec {
            spec: spec.to_string(),
        });
    }

    let bind_addr = parts[0].to_string();
    let bind_port: u16 = parts[1].parse().map_err(|_| ForwardError::InvalidSpec {
        spec: spec.to_string(),
    })?;
    let target_host = parts[2].to_string();
    let target_port: u16 = parts[3].parse().map_err(|_| ForwardError::InvalidSpec {
        spec: spec.to_string(),
    })?;

    Ok((bind_addr, bind_port, target_host, target_port))
}

pub struct LocalForwarder {
    spec: PortForwardSpec,
    listener: Option<TcpListener>,
}

impl LocalForwarder {
    pub fn new(spec: PortForwardSpec) -> Result<Self, ForwardError> {
        if spec.kind != PortForwardSpecKind::Local {
            return Err(ForwardError::InvalidSpec {
                spec: format!("expected local spec, got {:?}", spec.kind),
            });
        }
        Ok(Self {
            spec,
            listener: None,
        })
    }

    pub fn spec(&self) -> &PortForwardSpec {
        &self.spec
    }

    pub async fn run<H: client::Handler + Send + 'static>(
        &mut self,
        handle: Arc<Mutex<client::Handle<H>>>,
    ) -> Result<(), ForwardError> {
        let listen_addr = self.spec.listen_addr()?;
        let listener: TcpListener = TcpListener::bind(listen_addr)
            .await
            .map_err(|e| ForwardError::BindFailed { source: e })?;
        self.listener = Some(listener);
        let remote_host = self.spec.target_host.clone();
        let remote_port = self.spec.target_port;

        info!(
            "local forward listening on {} -> {}:{}",
            listen_addr, remote_host, remote_port
        );

        loop {
            let listener = match &self.listener {
                Some(l) => l,
                None => return Ok(()),
            };
            let accept_result = listener.accept().await;
            let (local_stream, local_addr) = match accept_result {
                Ok(conn) => conn,
                Err(e) => {
                    let handle = handle.lock().await;
                    if handle.is_closed() {
                        debug!("local forward accept loop ending: ssh session closed");
                        return Ok(());
                    }
                    drop(handle);
                    error!("local forward accept error: {}", e);
                    continue;
                }
            };

            debug!(
                "local forward connection from {} -> {}:{}",
                local_addr, remote_host, remote_port
            );

            let handle = handle.clone();
            let remote_host = remote_host.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    proxy_local_to_remote(local_stream, handle, &remote_host, remote_port).await
                {
                    debug!("local forward proxy error: {}", e);
                }
            });
        }
    }

    pub async fn stop(&mut self) {
        if let Some(listener) = self.listener.take() {
            drop(listener);
        }
    }

    pub fn local_port(&self) -> u16 {
        self.spec.bind_port
    }
}

async fn proxy_local_to_remote<H: client::Handler + Send + 'static>(
    local_stream: TcpStream,
    handle: Arc<Mutex<client::Handle<H>>>,
    remote_host: &str,
    remote_port: u16,
) -> Result<(), ForwardError> {
    let local_addr = local_stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();

    let handle_guard = handle.lock().await;
    let channel = handle_guard
        .channel_open_direct_tcpip(
            remote_host,
            remote_port as u32,
            &local_addr,
            0,
        )
        .await
        .map_err(|e| ForwardError::ChannelOpenFailed {
            source: Box::new(e) as _,
        })?;
    drop(handle_guard);

    let ssh_stream = channel.into_stream();
    let (mut ssh_read, mut ssh_write) = tokio::io::split(ssh_stream);
    let (mut local_read, mut local_write) = tokio::io::split(local_stream);

    let client_to_server = io::copy(&mut local_read, &mut ssh_write);
    let server_to_client = io::copy(&mut ssh_read, &mut local_write);

    match tokio::join!(client_to_server, server_to_client) {
        (Err(e), _) | (_, Err(e)) => {
            debug!("local forward bidirectional copy error: {}", e);
        }
        _ => {}
    }

    Ok(())
}

pub struct RemoteForwarder {
    spec: PortForwardSpec,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RemoteForwarder {
    pub fn new(spec: PortForwardSpec) -> Result<Self, ForwardError> {
        if spec.kind != PortForwardSpecKind::Remote {
            return Err(ForwardError::InvalidSpec {
                spec: format!("expected remote spec, got {:?}", spec.kind),
            });
        }
        Ok(Self { spec, cancel: None })
    }

    pub fn spec(&self) -> &PortForwardSpec {
        &self.spec
    }

    pub async fn register<H: client::Handler + Send + 'static>(
        &self,
        handle: &mut client::Handle<H>,
    ) -> Result<u32, ForwardError> {
        let port = handle
            .tcpip_forward(&self.spec.bind_addr, self.spec.bind_port as u32)
            .await
            .map_err(|e| ForwardError::ChannelOpenFailed {
                source: Box::new(e) as _,
            })?;
        Ok(port)
    }

    pub async fn handle_forwarded_channel(
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        local_host: &str,
        local_port: u16,
    ) {
        debug!(
            "remote forward: server opened forwarded-tcpip channel to {}:{} -> local {}:{}",
            connected_address, connected_port, local_host, local_port
        );

        let local_target = format!("{}:{}", local_host, local_port);
        let local_stream = match TcpStream::connect(&local_target).await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "remote forward: failed to connect to local target {}: {}",
                    local_target, e
                );
                return;
            }
        };

        let ssh_stream = channel.into_stream();
        let (mut ssh_read, mut ssh_write) = tokio::io::split(ssh_stream);
        let (mut local_read, mut local_write) = tokio::io::split(local_stream);

        let client_to_server = io::copy(&mut local_read, &mut ssh_write);
        let server_to_client = io::copy(&mut ssh_read, &mut local_write);

        match tokio::join!(client_to_server, server_to_client) {
            (Err(e), _) | (_, Err(e)) => {
                debug!("remote forward bidirectional copy error: {}", e);
            }
            _ => {}
        }
    }

    pub async fn unregister<H: client::Handler + Send + 'static>(
        &self,
        handle: &client::Handle<H>,
    ) -> Result<(), ForwardError> {
        handle
            .cancel_tcpip_forward(&self.spec.bind_addr, self.spec.bind_port as u32)
            .await
            .map_err(|e| ForwardError::ChannelOpenFailed {
                source: Box::new(e) as _,
            })?;
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

pub async fn run_local_forwarders<H: client::Handler + Send + 'static>(
    forwarders: Vec<LocalForwarder>,
    handle: Arc<Mutex<client::Handle<H>>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Vec<LocalForwarder> {
    let mut forwarders = forwarders;
    let mut tasks = Vec::new();

    for forwarder in forwarders.drain(..) {
        let handle = handle.clone();
        let spec = forwarder.spec().clone();
        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        tasks.push(tokio::spawn(async move {
            let mut fwd = forwarder;
            tokio::select! {
                result = fwd.run(handle) => {
                    if let Err(e) = result {
                        error!("local forward {} failed: {}", spec, e);
                    }
                }
                _ = cancel_rx => {
                    fwd.stop().await;
                }
            }
            fwd
        }));
    }

    let _ = shutdown.changed().await;

    for task in &tasks {
        task.abort();
    }

    let mut results = Vec::new();
    for task in tasks {
        match task.await {
            Ok(fwd) => results.push(fwd),
            Err(e) => {
                if !e.is_cancelled() {
                    error!("local forwarder task panicked: {}", e);
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_local_spec() {
        let spec = PortForwardSpec::local("127.0.0.1:5432:db.internal:5432").unwrap();
        assert_eq!(spec.kind, PortForwardSpecKind::Local);
        assert_eq!(spec.bind_addr, "127.0.0.1");
        assert_eq!(spec.bind_port, 5432);
        assert_eq!(spec.target_host, "db.internal");
        assert_eq!(spec.target_port, 5432);
    }

    #[test]
    fn parse_remote_spec() {
        let spec = PortForwardSpec::remote("0.0.0.0:8080:127.0.0.1:3000").unwrap();
        assert_eq!(spec.kind, PortForwardSpecKind::Remote);
        assert_eq!(spec.bind_addr, "0.0.0.0");
        assert_eq!(spec.bind_port, 8080);
        assert_eq!(spec.target_host, "127.0.0.1");
        assert_eq!(spec.target_port, 3000);
    }

    #[test]
    fn parse_spec_invalid_few_parts() {
        assert!(PortForwardSpec::local("127.0.0.1:5432:db").is_err());
    }

    #[test]
    fn parse_spec_invalid_many_parts() {
        assert!(PortForwardSpec::local("a:b:c:d:e").is_err());
    }

    #[test]
    fn parse_spec_invalid_port() {
        assert!(PortForwardSpec::local("127.0.0.1:abc:db:5432").is_err());
    }

    #[test]
    fn parse_spec_invalid_target_port() {
        assert!(PortForwardSpec::local("127.0.0.1:5432:db:abc").is_err());
    }

    #[test]
    fn spec_display() {
        let spec = PortForwardSpec::local("127.0.0.1:5432:db.internal:5432").unwrap();
        assert_eq!(spec.to_string(), "-L 127.0.0.1:5432:db.internal:5432");
    }

    #[test]
    fn spec_display_remote() {
        let spec = PortForwardSpec::remote("0.0.0.0:8080:127.0.0.1:3000").unwrap();
        assert_eq!(spec.to_string(), "-R 0.0.0.0:8080:127.0.0.1:3000");
    }

    #[test]
    fn local_forwarder_rejects_remote_spec() {
        let spec = PortForwardSpec::remote("0.0.0.0:8080:127.0.0.1:3000").unwrap();
        assert!(LocalForwarder::new(spec).is_err());
    }

    #[test]
    fn remote_forwarder_rejects_local_spec() {
        let spec = PortForwardSpec::local("127.0.0.1:5432:db.internal:5432").unwrap();
        assert!(RemoteForwarder::new(spec).is_err());
    }

    #[test]
    fn listen_addr_valid() {
        let spec = PortForwardSpec::local("127.0.0.1:5432:db.internal:5432").unwrap();
        let addr = spec.listen_addr().unwrap();
        assert_eq!(addr.port(), 5432);
    }

    #[test]
    fn listen_addr_invalid_host() {
        let spec = PortForwardSpec {
            kind: PortForwardSpecKind::Local,
            bind_addr: "!!!invalid".to_string(),
            bind_port: 5432,
            target_host: "db".to_string(),
            target_port: 5432,
        };
        assert!(spec.listen_addr().is_err());
    }

    #[tokio::test]
    async fn local_forward_bind_and_accept() {
        let spec = PortForwardSpec::local(&format!("127.0.0.1:0:remote:5432")).unwrap();
        let forwarder = LocalForwarder::new(spec).unwrap();

        let listen_addr = forwarder.spec.listen_addr().unwrap();
        let listener = TcpListener::bind(listen_addr).await.unwrap();
        let bound_addr = listener.local_addr().unwrap();
        drop(listener);

        let spec = PortForwardSpec::local(&format!(
            "127.0.0.1:{}:remote:5432",
            bound_addr.port()
        ))
        .unwrap();
        let forwarder = LocalForwarder::new(spec).unwrap();
        assert_eq!(forwarder.local_port(), bound_addr.port());
    }

    #[tokio::test]
    async fn remote_forward_proxy_bidirectional() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let _echo_addr = echo_listener.local_addr().unwrap();

        let echo_server = tokio::spawn(async move {
            let (mut stream, _) = echo_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            loop {
                let n = match stream.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                if stream.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });

        let local_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local_listener.local_addr().unwrap();

        let proxy_task = tokio::spawn(async move {
            let (stream, _) = local_listener.accept().await.unwrap();
            let (mut read, mut write) = tokio::io::split(stream);
            let _ = io::copy(&mut read, &mut write).await;
        });

        let mut local_conn = TcpStream::connect(local_addr).await.unwrap();
        local_conn.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 64];
        let n = local_conn.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");

        echo_server.abort();
        proxy_task.abort();
    }

    #[test]
    fn forwarder_spec_access() {
        let spec = PortForwardSpec::local("127.0.0.1:5432:db.internal:5432").unwrap();
        let forwarder = LocalForwarder::new(spec.clone()).unwrap();
        assert_eq!(forwarder.spec(), &spec);
        assert_eq!(forwarder.local_port(), 5432);
    }

    #[test]
    fn remote_forwarder_spec_access() {
        let spec = PortForwardSpec::remote("0.0.0.0:8080:127.0.0.1:3000").unwrap();
        let forwarder = RemoteForwarder::new(spec.clone()).unwrap();
        assert_eq!(forwarder.spec(), &spec);
    }
}