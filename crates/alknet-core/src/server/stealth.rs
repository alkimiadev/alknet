//! Stealth mode: protocol detection on TLS connections.
//!
//! When stealth mode is enabled with TLS transport, the server peeks at the first
//! bytes after the TLS handshake to determine whether the client is speaking SSH
//! or HTTP. Non-SSH connections receive a fake nginx 404 response, making the
//! server appear as an ordinary web server to port scanners and DPI systems.
//! See ADR-017.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

const SSH_BANNER_PREFIX: &[u8] = b"SSH-2.0-";
const FAKE_NGINX_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nServer: nginx\r\n\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolDetection {
    Ssh,
    Http,
}

pub async fn detect_protocol<S>(stream: S) -> (ProtocolDetection, BufReader<S>)
where
    S: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);

    let detection = match reader.fill_buf().await {
        Ok(buf) if buf.len() >= SSH_BANNER_PREFIX.len() => {
            if &buf[..SSH_BANNER_PREFIX.len()] == SSH_BANNER_PREFIX {
                ProtocolDetection::Ssh
            } else {
                ProtocolDetection::Http
            }
        }
        Ok(buf) if !buf.is_empty() => {
            if buf.starts_with(SSH_BANNER_PREFIX) {
                ProtocolDetection::Ssh
            } else {
                ProtocolDetection::Http
            }
        }
        _ => ProtocolDetection::Http,
    };

    (detection, reader)
}

pub async fn send_fake_nginx_404<S>(reader: &mut BufReader<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = reader.get_mut().write_all(FAKE_NGINX_404).await;
    let _ = reader.get_mut().shutdown().await;
}

pub fn validate_stealth_config(stealth: bool, transport_is_tls: bool) -> Result<(), &'static str> {
    if stealth && !transport_is_tls {
        return Err("stealth mode requires TLS transport (--transport tls)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    async fn write_and_detect(data: &[u8]) -> ProtocolDetection {
        let (client, server) = duplex(1024);
        let mut client = client;

        client.write_all(data).await.unwrap();
        drop(client);

        let (detection, _) = detect_protocol(server).await;
        detection
    }

    #[tokio::test]
    async fn ssh_banner_detected() {
        let detection = write_and_detect(b"SSH-2.0-OpenSSH_9.0\r\n").await;
        assert_eq!(detection, ProtocolDetection::Ssh);
    }

    #[tokio::test]
    async fn ssh_banner_other_implementation() {
        let detection = write_and_detect(b"SSH-2.0-russh_0.49\r\n").await;
        assert_eq!(detection, ProtocolDetection::Ssh);
    }

    #[tokio::test]
    async fn ssh_banner_minimal() {
        let detection = write_and_detect(b"SSH-2.0-X\n").await;
        assert_eq!(detection, ProtocolDetection::Ssh);
    }

    #[tokio::test]
    async fn http_get_detected_as_http() {
        let detection = write_and_detect(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn http_post_detected_as_http() {
        let detection = write_and_detect(b"POST /api HTTP/1.1\r\nHost: example.com\r\n\r\n").await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn random_data_detected_as_http() {
        let detection = write_and_detect(b"\x01\x02\x03\x04\x05\x06\x07\x08").await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn empty_stream_detected_as_http() {
        let (client, server) = duplex(1024);
        drop(client);
        let (detection, _) = detect_protocol(server).await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn ssh_banner_bytes_preserved_by_bufreader() {
        let (client, server) = duplex(1024);
        let mut client = client;

        let banner = b"SSH-2.0-OpenSSH_9.0\r\n";
        client.write_all(banner).await.unwrap();
        client.write_all(b"subsequent data").await.unwrap();
        drop(client);

        let (detection, mut reader) = detect_protocol(server).await;
        assert_eq!(detection, ProtocolDetection::Ssh);

        let mut all_data = Vec::new();
        reader.read_to_end(&mut all_data).await.unwrap();
        assert!(all_data.starts_with(banner), "banner bytes must be preserved after detection");
    }

    #[tokio::test]
    async fn fake_nginx_404_response() {
        let (client, server) = duplex(1024);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        client_write.write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();
        drop(client_write);

        let (detection, mut reader) = detect_protocol(server).await;
        assert_eq!(detection, ProtocolDetection::Http);

        send_fake_nginx_404(&mut reader).await;

        let mut buf = [0u8; 256];
        let n = client_read.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("HTTP/1.1 404 Not Found"));
        assert!(response.contains("Server: nginx"));
    }

    #[tokio::test]
    async fn protocol_detection_enum_equality() {
        assert_eq!(ProtocolDetection::Ssh, ProtocolDetection::Ssh);
        assert_eq!(ProtocolDetection::Http, ProtocolDetection::Http);
        assert_ne!(ProtocolDetection::Ssh, ProtocolDetection::Http);
    }

    #[test]
    fn validate_stealth_without_tls_rejected() {
        let result = validate_stealth_config(true, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("TLS transport"));
    }

    #[test]
    fn validate_stealth_with_tls_accepted() {
        let result = validate_stealth_config(true, true);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_no_stealth_with_tcp_accepted() {
        let result = validate_stealth_config(false, false);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_no_stealth_with_tls_accepted() {
        let result = validate_stealth_config(false, true);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn short_data_detected_as_http() {
        let detection = write_and_detect(b"GE").await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn partial_ssh_prefix_detected_as_http() {
        let detection = write_and_detect(b"SSH-1.").await;
        assert_eq!(detection, ProtocolDetection::Http);
    }

    #[tokio::test]
    async fn http_request_gets_404_then_closed() {
        let (client, server) = duplex(1024);
        let mut client = client;

        client.write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();

        let (detection, mut reader) = detect_protocol(server).await;
        assert_eq!(detection, ProtocolDetection::Http);

        send_fake_nginx_404(&mut reader).await;

        let mut buf = [0u8; 256];
        let n = client.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.starts_with("HTTP/1.1 404 Not Found"));
        assert!(response.contains("Server: nginx"));

        let mut extra = [0u8; 16];
        let result = client.read(&mut extra).await;
        assert!(result.is_err() || result.unwrap() == 0);
    }
}