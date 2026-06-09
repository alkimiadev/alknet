use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::auth::{AuthToken, Identity, IdentityProvider};
use crate::call::frame::{decode_with_remainder, encode};
use crate::call::EventEnvelope;
use crate::interface::session::{InterfaceEvent, InterfaceSession};
use crate::interface::{StreamInterface, StreamInterfaceConfig, TransportStream};

const READ_BUF_SIZE: usize = 8192;

pub struct RawFramingInterface;

#[async_trait]
impl StreamInterface for RawFramingInterface {
    type Session = RawFramingSession;

    async fn accept(
        &self,
        stream: Box<dyn TransportStream>,
        config: &StreamInterfaceConfig,
    ) -> Result<Self::Session> {
        let raw_config = match config {
            StreamInterfaceConfig::RawFraming(c) => c,
            StreamInterfaceConfig::Ssh(_) => {
                return Err(anyhow::anyhow!(
                    "RawFramingInterface received SshInterfaceConfig"
                ));
            }
        };

        Ok(RawFramingSession::new(stream, Arc::clone(&raw_config.auth)))
    }
}

enum AuthState {
    Pending,
    Authenticated(Identity),
    Failed,
}

pub struct RawFramingSession {
    reader: BufReader<tokio::io::ReadHalf<Box<dyn TransportStream>>>,
    writer: BufWriter<tokio::io::WriteHalf<Box<dyn TransportStream>>>,
    auth_state: AuthState,
    identity_provider: Arc<dyn IdentityProvider>,
    read_buf: Vec<u8>,
}

impl RawFramingSession {
    pub fn new(
        stream: Box<dyn TransportStream>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(read_half),
            writer: BufWriter::new(write_half),
            auth_state: AuthState::Pending,
            identity_provider,
            read_buf: Vec::new(),
        }
    }

    async fn read_frame(&mut self) -> Result<EventEnvelope> {
        loop {
            match decode_with_remainder(&self.read_buf) {
                Ok((envelope, consumed)) => {
                    self.read_buf.drain(..consumed);
                    return Ok(envelope);
                }
                Err(crate::call::frame::FrameDecodeError::TooShort { .. })
                | Err(crate::call::frame::FrameDecodeError::Incomplete { .. }) => {
                    let mut tmp = [0u8; READ_BUF_SIZE];
                    let n = self.reader.read(&mut tmp).await?;
                    if n == 0 {
                        return Err(anyhow::anyhow!("stream closed while reading frame"));
                    }
                    self.read_buf.extend_from_slice(&tmp[..n]);
                }
                Err(crate::call::frame::FrameDecodeError::Json(e)) => {
                    return Err(anyhow::anyhow!("frame JSON decode error: {e}"));
                }
            }
        }
    }

    async fn write_frame(&mut self, envelope: &EventEnvelope) -> Result<()> {
        let frame = encode(envelope);
        self.writer.write_all(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[async_trait]
impl InterfaceSession for RawFramingSession {
    async fn recv(&mut self) -> Option<InterfaceEvent> {
        match &self.auth_state {
            AuthState::Failed => return None,
            AuthState::Authenticated(_) => {
                let identity = match &self.auth_state {
                    AuthState::Authenticated(id) => id.clone(),
                    _ => unreachable!(),
                };
                let envelope = match self.read_frame().await {
                    Ok(e) => e,
                    Err(_) => return None,
                };
                return Some(InterfaceEvent::with_identity(envelope, identity));
            }
            AuthState::Pending => {}
        }

        let envelope = match self.read_frame().await {
            Ok(e) => e,
            Err(_) => {
                self.auth_state = AuthState::Failed;
                return None;
            }
        };

        let token_raw = envelope.payload.as_str().unwrap_or("").as_bytes().to_vec();
        let token = AuthToken { raw: token_raw };

        match self.identity_provider.resolve_from_token(&token) {
            Some(identity) => {
                self.auth_state = AuthState::Authenticated(identity.clone());
                Some(InterfaceEvent::with_identity(envelope, identity))
            }
            None => {
                self.auth_state = AuthState::Failed;
                None
            }
        }
    }

    async fn send(&mut self, envelope: EventEnvelope) -> Result<()> {
        match self.auth_state {
            AuthState::Failed => Err(anyhow::anyhow!("session authentication failed")),
            _ => self.write_frame(&envelope).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ConfigIdentityProvider;
    use crate::config::DynamicConfig;
    use crate::interface::RawFramingConfig;
    use arc_swap::ArcSwap;
    use std::collections::HashMap;

    fn make_provider() -> Arc<dyn IdentityProvider> {
        Arc::new(ConfigIdentityProvider::new(Arc::new(ArcSwap::new(
            Arc::new(DynamicConfig::default()),
        ))))
    }

    fn make_provider_with_identity(
        identity: Identity,
        valid_token: &str,
    ) -> (Arc<dyn IdentityProvider>, String) {
        struct MockProvider {
            identity: Identity,
            valid_token: String,
        }
        impl IdentityProvider for MockProvider {
            fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
                if token.raw == self.valid_token.as_bytes() {
                    Some(self.identity.clone())
                } else {
                    None
                }
            }
        }
        let provider = Arc::new(MockProvider {
            identity,
            valid_token: valid_token.to_string(),
        });
        (provider, valid_token.to_string())
    }

    #[tokio::test]
    async fn raw_framing_interface_accept_succeeds() {
        let iface = RawFramingInterface;
        let (_client, server) = tokio::io::duplex(1024);
        let stream: Box<dyn TransportStream> = Box::new(server);
        let config = StreamInterfaceConfig::RawFraming(RawFramingConfig {
            auth: make_provider(),
        });
        let result = iface.accept(stream, &config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn raw_framing_interface_rejects_ssh_config() {
        let iface = RawFramingInterface;
        let (_client, server) = tokio::io::duplex(1024);
        let stream: Box<dyn TransportStream> = Box::new(server);
        let config = StreamInterfaceConfig::Ssh(crate::interface::SshInterfaceConfig {
            auth: make_provider(),
            forwarding: Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default()))),
            host_key: Arc::new(
                russh::keys::PrivateKey::random(
                    &mut rand_core::OsRng,
                    russh::keys::Algorithm::Ed25519,
                )
                .unwrap(),
            ),
        });
        let result = iface.accept(stream, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn raw_framing_session_round_trip() {
        let identity = Identity {
            id: "test-id".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        let (provider, token_str) =
            make_provider_with_identity(identity.clone(), "valid-test-token");

        let (client, server) = tokio::io::duplex(4096);
        let server_stream: Box<dyn TransportStream> = Box::new(server);
        let client_stream: Box<dyn TransportStream> = Box::new(client);

        let mut server_session = RawFramingSession::new(server_stream, provider);

        let auth_envelope = EventEnvelope::new("auth", "auth-1", serde_json::json!(token_str));
        let auth_frame = encode(&auth_envelope);

        let mut client_writer = tokio::io::BufWriter::new(client_stream);
        client_writer.write_all(&auth_frame).await.unwrap();
        client_writer.flush().await.unwrap();

        let event = server_session.recv().await;
        assert!(event.is_some());
        let event = event.unwrap();
        assert!(event.identity.is_some());
        assert_eq!(event.identity.as_ref().unwrap().id, "test-id");

        let data_envelope =
            EventEnvelope::call_requested("req-2", serde_json::json!({"op": "test"}));
        let data_frame = encode(&data_envelope);
        client_writer.write_all(&data_frame).await.unwrap();
        client_writer.flush().await.unwrap();

        let event = server_session.recv().await;
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.envelope.r#type, "call.requested");
        assert_eq!(event.envelope.id, "req-2");
        assert!(event.identity.is_some());
    }

    #[tokio::test]
    async fn first_frame_auth_valid_token() {
        let identity = Identity {
            id: "auth-user".to_string(),
            scopes: vec!["admin".to_string()],
            resources: HashMap::new(),
        };
        let (provider, token_str) = make_provider_with_identity(identity, "my-valid-token");

        let (client, server) = tokio::io::duplex(4096);
        let server_stream: Box<dyn TransportStream> = Box::new(server);
        let client_stream: Box<dyn TransportStream> = Box::new(client);

        let mut session = RawFramingSession::new(server_stream, provider);

        let auth_envelope = EventEnvelope::new("auth", "auth-1", serde_json::json!(token_str));
        let frame = encode(&auth_envelope);
        let mut writer = tokio::io::BufWriter::new(client_stream);
        writer.write_all(&frame).await.unwrap();
        writer.flush().await.unwrap();

        let event = session.recv().await;
        assert!(event.is_some());
        let event = event.unwrap();
        assert!(event.identity.is_some());
        assert_eq!(event.identity.as_ref().unwrap().id, "auth-user");
        assert_eq!(event.identity.as_ref().unwrap().scopes, vec!["admin"]);
    }

    #[tokio::test]
    async fn first_frame_auth_invalid_token() {
        let identity = Identity {
            id: "auth-user".to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        };
        let (provider, _) = make_provider_with_identity(identity, "correct-token");

        let (client, server) = tokio::io::duplex(4096);
        let server_stream: Box<dyn TransportStream> = Box::new(server);
        let client_stream: Box<dyn TransportStream> = Box::new(client);

        let mut session = RawFramingSession::new(server_stream, provider);

        let bad_envelope =
            EventEnvelope::new("auth", "auth-1", serde_json::json!("bad-token-value"));
        let frame = encode(&bad_envelope);
        let mut writer = tokio::io::BufWriter::new(client_stream);
        writer.write_all(&frame).await.unwrap();
        writer.flush().await.unwrap();

        let event = session.recv().await;
        assert!(event.is_none());

        let data_envelope = EventEnvelope::call_requested("req-2", serde_json::json!({}));
        let result = session.send(data_envelope).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn raw_framing_session_send() {
        let identity = Identity {
            id: "send-user".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        let (provider, token_str) = make_provider_with_identity(identity, "send-token");

        let (client, server) = tokio::io::duplex(4096);
        let server_stream: Box<dyn TransportStream> = Box::new(server);
        let client_stream: Box<dyn TransportStream> = Box::new(client);

        let mut server_session = RawFramingSession::new(server_stream, provider);

        let auth_envelope = EventEnvelope::new("auth", "auth-1", serde_json::json!(token_str));
        let auth_frame = encode(&auth_envelope);
        let mut client_writer = tokio::io::BufWriter::new(client_stream);
        client_writer.write_all(&auth_frame).await.unwrap();
        client_writer.flush().await.unwrap();

        let _ = server_session.recv().await;

        let response = EventEnvelope::call_responded("req-1", serde_json::json!({"result": "ok"}));
        let send_result = server_session.send(response).await;
        assert!(send_result.is_ok());
    }

    #[tokio::test]
    async fn raw_framing_multiple_frames_over_duplex() {
        let identity = Identity {
            id: "multi-user".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        let (provider, token_str) = make_provider_with_identity(identity, "multi-token");

        let (client, server) = tokio::io::duplex(8192);
        let server_stream: Box<dyn TransportStream> = Box::new(server);
        let client_stream: Box<dyn TransportStream> = Box::new(client);

        let mut session = RawFramingSession::new(server_stream, provider);
        let mut client_writer = tokio::io::BufWriter::new(client_stream);

        let auth_envelope = EventEnvelope::new("auth", "auth-0", serde_json::json!(token_str));
        client_writer
            .write_all(&encode(&auth_envelope))
            .await
            .unwrap();

        for i in 1..=5 {
            let envelope =
                EventEnvelope::call_requested(format!("req-{i}"), serde_json::json!({"seq": i}));
            client_writer.write_all(&encode(&envelope)).await.unwrap();
        }
        client_writer.flush().await.unwrap();

        let auth_event = session.recv().await;
        assert!(auth_event.is_some());
        assert!(auth_event.unwrap().identity.is_some());

        for i in 1..=5 {
            let event = session.recv().await;
            assert!(event.is_some());
            let event = event.unwrap();
            assert_eq!(event.envelope.id, format!("req-{i}"));
            assert!(event.identity.is_some());
        }
    }

    #[test]
    fn raw_framing_interface_type_exists() {
        let _iface = RawFramingInterface;
    }
}
