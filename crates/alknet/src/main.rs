//! # alknet
//!
//! CLI binary for [Alknet](https://git.alk.dev/alkdev/alknet), a self-hostable SSH-based tunnel
//! tool. Provides `alknet connect` (client) and `alknet serve` (server) subcommands with
//! pluggable transports (TCP, TLS, iroh).
//!
//! > **Alpha software.** See `alknet-core` for library usage.

use std::net::SocketAddr;
use std::process;
use std::sync::Arc;

use alknet_core::auth::keys::KeySource;
use alknet_core::client::{ConnectOptions, TransportMode};
use alknet_core::server::{ServeOptions, ServeTransportMode, Server};
#[cfg(feature = "iroh")]
use alknet_core::transport::IrohTransport;
use alknet_core::transport::TcpTransport;
#[cfg(feature = "tls")]
use alknet_core::transport::TlsTransport;
use alknet_core::transport::Transport;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "alknet", version, about = "Alknet SSH tunnel tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Connect to an alknet server and start a SOCKS5 proxy / port forwarding session"
    )]
    Connect {
        #[arg(
            long,
            help = "TCP/TLS server address (required for tcp/tls transport)",
            env = "ALKNET_SERVER"
        )]
        server: Option<String>,

        #[arg(
            long,
            help = "iroh endpoint ID, base58-encoded (required for iroh transport)"
        )]
        peer: Option<String>,

        #[arg(long, value_enum, default_value = "tcp", help = "Transport mode")]
        transport: TransportModeArg,

        #[arg(long, help = "SSH private key path", env = "ALKNET_IDENTITY")]
        identity: Option<String>,

        #[arg(long, default_value = "127.0.0.1:1080", help = "SOCKS5 listen address")]
        socks5: String,

        #[arg(long, action = clap::ArgAction::Append, help = "Port forward spec (repeatable, e.g. 5432:db:5432)")]
        forward: Vec<String>,

        #[arg(long, action = clap::ArgAction::Append, help = "Remote port forward spec (repeatable)")]
        remote_forward: Vec<String>,

        #[arg(long, help = "Upstream proxy URL (socks5:// or http://)")]
        proxy: Option<String>,

        #[arg(long, help = "iroh relay URL")]
        iroh_relay: Option<String>,

        #[arg(long, help = "SNI hostname for TLS")]
        tls_server_name: Option<String>,

        #[arg(long, help = "Accept self-signed TLS certs")]
        insecure: bool,
    },

    #[command(about = "Start the alknet server (accept SSH connections)")]
    Serve {
        #[arg(long, help = "SSH host key path (required)")]
        key: String,

        #[arg(long, help = "Authorized keys file path")]
        authorized_keys: Option<String>,

        #[arg(long, help = "CA public key for certificate authority auth")]
        cert_authority: Option<String>,

        #[arg(
            long,
            value_enum,
            default_value = "tcp",
            help = "Transport mode (tcp, tls, iroh)"
        )]
        transport: ServeTransportModeArg,

        #[arg(
            long,
            default_value = "0.0.0.0:22",
            help = "Listen address for TCP/TLS"
        )]
        listen: String,

        #[arg(long, help = "TLS certificate path (manual)")]
        tls_cert: Option<String>,

        #[arg(long, help = "TLS private key path (manual)")]
        tls_key: Option<String>,

        #[arg(long, help = "ACME auto-cert domain")]
        acme_domain: Option<String>,

        #[arg(
            long,
            help = "Serve fake nginx 404 to non-SSH connections (requires --transport tls)"
        )]
        stealth: bool,

        #[arg(long, help = "Outbound proxy URL (socks5:// or http://)")]
        proxy: Option<String>,

        #[arg(long, help = "iroh relay server URL")]
        iroh_relay: Option<String>,

        #[arg(
            long,
            default_value_t = 0,
            help = "Max concurrent connections per IP (0 = unlimited)"
        )]
        max_connections_per_ip: usize,

        #[arg(
            long,
            default_value_t = 10,
            help = "Max auth failures before disconnect"
        )]
        max_auth_attempts: usize,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum TransportModeArg {
    Tcp,
    Tls,
    Iroh,
}

impl From<TransportModeArg> for TransportMode {
    fn from(val: TransportModeArg) -> Self {
        match val {
            TransportModeArg::Tcp => TransportMode::Tcp,
            TransportModeArg::Tls => TransportMode::Tls,
            TransportModeArg::Iroh => TransportMode::Iroh,
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
enum ServeTransportModeArg {
    Tcp,
    Tls,
    Iroh,
}

impl From<ServeTransportModeArg> for ServeTransportMode {
    fn from(val: ServeTransportModeArg) -> Self {
        match val {
            ServeTransportModeArg::Tcp => ServeTransportMode::Tcp,
            ServeTransportModeArg::Tls => ServeTransportMode::Tls,
            ServeTransportModeArg::Iroh => ServeTransportMode::Iroh,
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Connect {
            server,
            peer,
            transport,
            identity,
            socks5,
            forward,
            remote_forward,
            proxy,
            iroh_relay,
            tls_server_name,
            insecure,
        } => {
            run_connect(
                server,
                peer,
                transport,
                identity,
                socks5,
                forward,
                remote_forward,
                proxy,
                iroh_relay,
                tls_server_name,
                insecure,
            )
            .await
        }
        Commands::Serve {
            key,
            authorized_keys,
            cert_authority,
            transport,
            listen,
            tls_cert,
            tls_key,
            acme_domain,
            stealth,
            proxy,
            iroh_relay,
            max_connections_per_ip,
            max_auth_attempts,
        } => {
            run_serve(
                key,
                authorized_keys,
                cert_authority,
                transport,
                listen,
                tls_cert,
                tls_key,
                acme_domain,
                stealth,
                proxy,
                iroh_relay,
                max_connections_per_ip,
                max_auth_attempts,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_connect(
    server: Option<String>,
    peer: Option<String>,
    transport: TransportModeArg,
    identity: Option<String>,
    socks5: String,
    forward: Vec<String>,
    remote_forward: Vec<String>,
    proxy: Option<String>,
    iroh_relay: Option<String>,
    tls_server_name: Option<String>,
    insecure: bool,
) -> Result<()> {
    let identity_val = identity
        .ok_or_else(|| anyhow!("--identity is required (or set ALKNET_IDENTITY env var)"))?;
    let key_source = KeySource::File(identity_val.into());

    let transport_mode: TransportMode = transport.into();

    if proxy.is_some() && matches!(transport_mode, TransportMode::Tcp) {
        eprintln!("warning: --proxy with --transport tcp is effectively a no-op (TCP transport is already a direct connection); use the SOCKS5 server instead");
    }

    let mut opts = ConnectOptions::new(key_source)
        .transport_mode(transport_mode.clone())
        .socks5_addr(&socks5);

    if let Some(ref s) = server {
        opts = opts.server(s);
    }
    if let Some(ref p) = peer {
        opts = opts.peer(p);
    }
    for fwd in &forward {
        opts = opts.forward(fwd);
    }
    for rfwd in &remote_forward {
        opts = opts.remote_forward(rfwd);
    }
    if let Some(ref p) = proxy {
        opts = opts.proxy(p);
    }
    if let Some(ref r) = iroh_relay {
        opts = opts.iroh_relay(r);
    }
    if let Some(ref n) = tls_server_name {
        opts = opts.tls_server_name(n);
    }
    if insecure {
        opts = opts.insecure(true);
    }

    opts.validate().map_err(|e| anyhow!("{e}"))?;

    match transport_mode {
        TransportMode::Tcp => {
            let addr: SocketAddr = server
                .as_deref()
                .ok_or_else(|| anyhow!("--server is required for tcp transport"))?
                .parse()
                .map_err(|e| anyhow!("invalid server address: {e}"))?;
            let t = Arc::new(TcpTransport::new(addr));
            connect_and_run(opts, t).await
        }
        TransportMode::Tls => {
            #[cfg(not(feature = "tls"))]
            {
                Err(anyhow!(
                    "TLS transport is not available (alknet-core built without 'tls' feature)"
                ))
            }
            #[cfg(feature = "tls")]
            {
                let addr: SocketAddr = server
                    .as_deref()
                    .ok_or_else(|| anyhow!("--server is required for tls transport"))?
                    .parse()
                    .map_err(|e| anyhow!("invalid server address: {e}"))?;
                let mut t = TlsTransport::new(addr);
                if let Some(ref n) = tls_server_name {
                    t = t.with_server_name(n);
                }
                t = t.with_insecure(insecure);
                let t = Arc::new(t);
                connect_and_run(opts, t).await
            }
        }
        TransportMode::Iroh => {
            #[cfg(not(feature = "iroh"))]
            {
                Err(anyhow!(
                    "iroh transport is not available (alknet-core built without 'iroh' feature)"
                ))
            }
            #[cfg(feature = "iroh")]
            {
                use iroh::{NodeId, RelayUrl};
                let node_id_str = peer
                    .as_deref()
                    .ok_or_else(|| anyhow!("--peer is required for iroh transport"))?;
                let node_id: NodeId = node_id_str
                    .parse()
                    .map_err(|e| anyhow!("invalid iroh peer endpoint ID: {e}"))?;
                let relay_url: Option<RelayUrl> = match iroh_relay.as_deref() {
                    Some(u) => Some(
                        u.parse()
                            .map_err(|e| anyhow!("invalid iroh relay URL: {e}"))?,
                    ),
                    None => None,
                };
                let proxy_url: Option<url::Url> = match proxy.as_deref() {
                    Some(u) => Some(u.parse().map_err(|e| anyhow!("invalid proxy URL: {e}"))?),
                    None => None,
                };
                let t = Arc::new(
                    IrohTransport::new(node_id, relay_url, proxy_url)
                        .await
                        .map_err(|e| anyhow!("failed to create iroh transport: {e}"))?,
                );
                connect_and_run(opts, t).await
            }
        }
    }
}

async fn connect_and_run<T: Transport>(opts: ConnectOptions, transport: Arc<T>) -> Result<()> {
    alknet_core::client::ClientSession::new(opts, transport)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .run()
        .await
        .map_err(|e| anyhow!("{e}"))
}

#[allow(clippy::too_many_arguments)]
async fn run_serve(
    key: String,
    authorized_keys: Option<String>,
    cert_authority: Option<String>,
    transport: ServeTransportModeArg,
    listen: String,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    acme_domain: Option<String>,
    stealth: bool,
    proxy: Option<String>,
    iroh_relay: Option<String>,
    max_connections_per_ip: usize,
    max_auth_attempts: usize,
) -> Result<()> {
    let transport_mode: ServeTransportMode = transport.into();

    if acme_domain.is_some() {
        #[cfg(not(feature = "acme"))]
        {
            return Err(anyhow!(
                "ACME support is not available (alknet built without 'acme' feature)"
            ));
        }
    }

    if stealth && transport_mode != ServeTransportMode::Tls {
        return Err(anyhow!(
            "stealth mode requires TLS transport (--transport tls)"
        ));
    }

    let mut opts = ServeOptions::new(KeySource::File(key.into()))
        .transport_mode(transport_mode.clone())
        .listen_addr(&listen)
        .stealth(stealth)
        .max_connections_per_ip(max_connections_per_ip)
        .max_auth_attempts(max_auth_attempts);

    if let Some(ref path) = authorized_keys {
        opts = opts.authorized_keys(KeySource::File(path.into()));
    }
    if let Some(ref path) = cert_authority {
        opts = opts.cert_authority(KeySource::File(path.into()));
    }
    if let Some(ref path) = tls_cert {
        opts = opts.tls_cert(path);
    }
    if let Some(ref path) = tls_key {
        opts = opts.tls_key(path);
    }
    if let Some(ref domain) = acme_domain {
        opts = opts.acme_domain(domain);
    }
    if let Some(ref url) = proxy {
        opts = opts.proxy(url);
    }
    if let Some(ref url) = iroh_relay {
        opts = opts.iroh_relay(url);
    }

    opts.validate().map_err(|e| anyhow!("{e}"))?;

    let server = Server::new(opts).map_err(|e| anyhow!("{e}"))?;

    match transport_mode {
        ServeTransportMode::Tcp => {
            let addr: SocketAddr = listen
                .parse()
                .map_err(|e| anyhow!("invalid listen address: {e}"))?;
            let acceptor = alknet_core::transport::TcpAcceptor::bind(addr)
                .await
                .map_err(|e| anyhow!("bind failed: {e}"))?;
            server.run(acceptor, None).await.map_err(|e| anyhow!("{e}"))
        }
        ServeTransportMode::Tls => {
            #[cfg(not(feature = "tls"))]
            {
                Err(anyhow!(
                    "TLS transport is not available (alknet-core built without 'tls' feature)"
                ))
            }
            #[cfg(feature = "acme")]
            {
                if let Some(ref domain) = acme_domain {
                    let addr: SocketAddr = listen
                        .parse()
                        .map_err(|e| anyhow!("invalid listen address: {e}"))?;
                    let provider = Arc::new(
                        alknet_core::transport::AcmeCertProvider::domain(domain)
                            .with_production_directory(),
                    );
                    let acceptor =
                        alknet_core::transport::AcmeTlsAcceptor::bind_acme(addr, provider)
                            .await
                            .map_err(|e| anyhow!("ACME bind failed: {e}"))?;
                    return server.run(acceptor, None).await.map_err(|e| anyhow!("{e}"));
                }
            }
            #[cfg(feature = "tls")]
            {
                use rustls_pki_types::{CertificateDer, PrivateKeyDer};
                let addr: SocketAddr = listen
                    .parse()
                    .map_err(|e| anyhow!("invalid listen address: {e}"))?;
                let cert_path = tls_cert.ok_or_else(|| {
                    anyhow!("--tls-cert is required for TLS transport (or use --acme-domain)")
                })?;
                let key_path = tls_key.ok_or_else(|| {
                    anyhow!("--tls-key is required for TLS transport (or use --acme-domain)")
                })?;
                let cert_data = std::fs::read(&cert_path)
                    .map_err(|e| anyhow!("failed to read TLS cert '{}': {e}", cert_path))?;
                let key_data = std::fs::read(&key_path)
                    .map_err(|e| anyhow!("failed to read TLS key '{}': {e}", key_path))?;
                let certs: Vec<CertificateDer<'static>> =
                    rustls_pemfile::certs(&mut &cert_data[..])
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| anyhow!("failed to parse TLS certificates: {e}"))?;
                let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_data[..])
                    .map_err(|e| anyhow!("failed to parse TLS private key: {e}"))?
                    .ok_or_else(|| anyhow!("no private key found in {}", key_path))?;
                let acceptor = alknet_core::transport::TlsAcceptor::bind(addr, certs, key, None)
                    .await
                    .map_err(|e| anyhow!("TLS bind failed: {e}"))?;
                server.run(acceptor, None).await.map_err(|e| anyhow!("{e}"))
            }
        }
        ServeTransportMode::Iroh => {
            #[cfg(not(feature = "iroh"))]
            {
                Err(anyhow!(
                    "iroh transport is not available (alknet-core built without 'iroh' feature)"
                ))
            }
            #[cfg(feature = "iroh")]
            {
                use iroh::RelayUrl;
                let relay_url: Option<RelayUrl> = match iroh_relay.as_deref() {
                    Some(u) => Some(
                        u.parse()
                            .map_err(|e| anyhow!("invalid iroh relay URL: {e}"))?,
                    ),
                    None => None,
                };
                let proxy_url: Option<url::Url> = match proxy.as_deref() {
                    Some(u) => Some(u.parse().map_err(|e| anyhow!("invalid proxy URL: {e}"))?),
                    None => None,
                };
                let acceptor = alknet_core::transport::IrohAcceptor::bind(relay_url, proxy_url)
                    .await
                    .map_err(|e| anyhow!("iroh bind failed: {e}"))?;
                let endpoint_id = acceptor.endpoint_id();
                eprintln!("iroh endpoint ID: {endpoint_id}");
                server
                    .run(acceptor, Some(&endpoint_id))
                    .await
                    .map_err(|e| anyhow!("{e}"))
            }
        }
    }
}
