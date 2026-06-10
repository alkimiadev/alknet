# Russh: Usage Patterns & Examples

This document provides practical usage patterns for both client and server sides of russh.

## Minimal Client

The simplest client connects, authenticates, opens a session channel, and runs a command:

```rust
use std::sync::Arc;
use russh::*;
use russh::keys::*;

struct Client;

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        // In production: verify against known_hosts
        Ok(true)
    }
}

async fn run_command(host: &str, command: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let config = Arc::new(client::Config::default());
    let mut session = client::connect(config, (host, 22), Client).await?;
    
    let auth = session.authenticate_publickey(
        "user",
        PrivateKeyWithHashAlg::new(
            Arc::new(keys::load_secret_key("/path/to/key", None)?),
            session.best_supported_rsa_hash().await?.flatten(),
        ),
    ).await?;
    
    if !auth.success() {
        return Err("Auth failed".into());
    }
    
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, command).await?;
    
    let mut exit_code = 0;
    let mut stdout = tokio::io::stdout();
    
    loop {
        let Some(msg) = channel.wait().await else { break };
        match msg {
            ChannelMsg::Data { data } => { stdout.write_all(&data).await?; }
            ChannelMsg::ExtendedData { data, ext } => {
                // ext == 1 is stderr
                eprint!("{}", String::from_utf8_lossy(&data));
            }
            ChannelMsg::ExitStatus { exit_status } => { exit_code = exit_status; }
            _ => {}
        }
    }
    
    session.disconnect(Disconnect::ByApplication, "", "en").await?;
    Ok(exit_code)
}
```

## Interactive PTY Client

For interactive sessions (shell access), request a PTY and handle window changes:

```rust
async fn interactive_session(session: &client::Handle<Client>) -> Result<Channel<client::Msg>, russh::Error> {
    let mut channel = session.channel_open_session().await?;
    
    // Request PTY
    channel.request_pty(
        false,           // want_reply
        "xterm-256bit",  // term type
        80, 24,          // cols, rows
        0, 0,            // pixel width/height (0 = not specified)
        &[],             // terminal modes
    ).await?;
    
    // Request shell
    channel.request_shell(true).await?;
    
    Ok(channel)
}
```

## Server Implementation

A basic server that accepts all public keys and echoes input:

```rust
use std::sync::Arc;
use russh::server::{self, Server as _, Session};
use russh::*;

#[derive(Clone)]
struct App;

impl server::Server for App {
    type Handler = ClientHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> ClientHandler {
        ClientHandler
    }
}

struct ClientHandler;

impl server::Handler for ClientHandler {
    type Error = russh::Error;
    
    async fn auth_publickey(&mut self, _: &str, _: &ssh_key::PublicKey) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }
    
    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
    
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Echo back
        session.data(channel, data.to_vec().into())?;
        Ok(())
    }
    
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Handle exec request
        session.channel_success(channel);
        // Process the command...
        session.channel_failure(channel);
        Ok(())
    }
}

async fn run_server() {
    let config = Arc::new(server::Config {
        keys: vec![russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap()],
        ..Default::default()
    });
    
    let mut app = App;
    let socket = tokio::net::TcpListener::bind(("0.0.0.0", 22)).await.unwrap();
    app.run_on_socket(config, &socket).await.unwrap();
}
```

## Authentication with SSH Agent

```rust
use russh::keys::agent::client::AgentClient;

async fn auth_with_agent(
    session: &mut client::Handle<Client>,
    user: &str,
) -> Result<AuthResult, Box<dyn std::error::Error>> {
    // Connect to SSH agent (Unix socket or Windows Pageant)
    let stream = tokio::net::UnixStream::connect(std::env::var("SSH_AUTH_SOCK")?).await?;
    let mut agent = AgentClient::connect(stream);
    
    // List available identities
    let identities = agent.request_identities().await?;
    
    // Try each identity
    for identity in &identities {
        let result = session.authenticate_publickey_with(
            user,
            identity.public_key(),
            None,  // hash_alg (None for Ed25519)
            &mut agent,
        ).await?;
        
        if result.success() {
            return Ok(result);
        }
    }
    
    Err("No matching key found in agent".into())
}
```

## Port Forwarding

### Local TCP Forwarding

```rust
async fn local_forward(
    session: &client::Handle<Client>,
    target_host: &str,
    target_port: u16,
) -> Result<Channel<client::Msg>, russh::Error> {
    let channel = session.channel_open_direct_tcpip(
        target_host,
        target_port as u32,
        "127.0.0.1",  // originator
        0,             // originator port
    ).await?;
    Ok(channel)
}
```

### Remote TCP Forwarding

```rust
async fn remote_forward(
    session: &client::Handle<Client>,
    listen_addr: &str,
    listen_port: u32,
) -> Result<u32, russh::Error> {
    // Request server to listen
    let assigned_port = session.tcpip_forward(listen_addr, listen_port).await?;
    Ok(assigned_port)
}
```

## Channel as AsyncRead/AsyncWrite

Channels can be converted to `AsyncRead` + `AsyncWrite` streams:

```rust
async fn use_channel_as_stream(channel: Channel<client::Msg>) {
    // Convert to bidirectional stream
    let stream = channel.into_stream();
    
    // Or split into read/write halves
    let (read_half, write_half) = channel.split();
    let mut reader = read_half.make_reader();
    let writer = write_half.make_writer();
    
    // Use with any tokio IO utility
    let mut buf = vec![0u8; 1024];
    use tokio::io::AsyncReadExt;
    let n = reader.read(&mut buf).await.unwrap();
    
    // Read stderr separately
    let stderr_reader = read_half.make_reader_ext(Some(1)); // ext code 1 = stderr
}
```

## Custom Algorithm Preferences

```rust
use std::borrow::Cow;

let config = client::Config {
    preferred: Preferred {
        kex: Cow::Owned(vec![
            russh::kex::CURVE25519,
            russh::kex::EXTENSION_SUPPORT_AS_CLIENT,
        ]),
        cipher: Cow::Owned(vec![
            russh::cipher::CHACHA20_POLY1305,
            russh::cipher::AES_256_GCM,
        ]),
        mac: Cow::Owned(vec![
            russh::mac::HMAC_SHA256_ETM,
        ]),
        ..Default::default()
    },
    ..Default::default()
};
```

## Rekeying

```rust
// Automatic rekeying happens based on Limits (default 1GB / 1 hour)
// Explicit rekeying:
session.rekey_soon().await?;

// Access shared secret after kex (for custom key derivation):
struct MyHandler;
impl client::Handler for MyHandler {
    type Error = russh::Error;
    
    async fn kex_done(
        &mut self,
        shared_secret: Option<&[u8]>,
        names: &Names,
        session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        // shared_secret is the raw DH shared secret
        // names contains all negotiated algorithms
        Ok(())
    }
    
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}
```

## Keyboard-Interactive Authentication

```rust
async fn keyboard_interactive_auth(
    session: &mut client::Handle<Client>,
    user: &str,
) -> Result<AuthResult, russh::Error> {
    let response = session.authenticate_keyboard_interactive_start(user, None).await?;
    
    match response {
        KeyboardInteractiveAuthResponse::Success => Ok(AuthResult::Success),
        KeyboardInteractiveAuthResponse::Failure { .. } => Ok(AuthResult::Failure { remaining_methods: MethodSet::empty(), partial_success: false }),
        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
            // Respond to each prompt
            let responses: Vec<String> = prompts.iter()
                .map(|p| /* get user input for p.prompt */ String::new())
                .collect();
            
            let response = session.authenticate_keyboard_interactive_respond(responses).await?;
            // May need to loop if server sends more challenges
            // ...
            todo!()
        }
    }
}
```

## Server: Handling Channel Requests

Server-side channel requests (shell, exec, pty, etc.) must be explicitly accepted or rejected:

```rust
impl server::Handler for MyHandler {
    type Error = russh::Error;
    
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Must call success or failure!
        session.channel_success(channel);
        Ok(())
    }
    
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data);
        if is_allowed(&command) {
            session.channel_success(channel);
            // Process command, send data back...
            session.data(channel, output.into())?;
            session.eof(channel)?;
        } else {
            session.channel_failure(channel);
        }
        Ok(())
    }
    
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel);
        Ok(())
    }
}
```

## Known Hosts Verification

```rust
use russh::keys::check_known_hosts;

impl client::Handler for SecureClient {
    type Error = russh::Error;
    
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        match check_known_hosts("example.com", 22, key) {
            Ok(()) => Ok(true),
            Err(e) => {
                if matches!(e, russh::keys::Error::KeyChanged { .. }) {
                    // Possible MITM attack!
                    eprintln!("WARNING: Host key changed! Possible MITM attack.");
                    Ok(false)
                } else {
                    // Unknown host - prompt user to verify fingerprint
                    eprintln!("Unknown host key. Fingerprint: {}", key.fingerprint(ssh_key::HashAlg::Sha256));
                    // In a real app, ask the user
                    Ok(false)
                }
            }
        }
    }
}
```