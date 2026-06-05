use std::net::{Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq)]
pub enum Socks5Address {
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
    Domain(String),
}

#[derive(Debug)]
pub struct Socks5VersionMethod {
    pub version: u8,
    pub methods: Vec<u8>,
}

impl Socks5VersionMethod {
    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Self> {
        let version = reader.read_u8().await?;
        let nmethods = reader.read_u8().await?;
        let mut methods = vec![0u8; nmethods as usize];
        reader.read_exact(&mut methods).await?;
        Ok(Self { version, methods })
    }
}

#[derive(Debug)]
pub struct Socks5Request {
    pub version: u8,
    pub command: u8,
    pub address: Socks5Address,
    pub port: u16,
}

impl Socks5Request {
    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Self> {
        let version = reader.read_u8().await?;
        let command = reader.read_u8().await?;
        let _rsv = reader.read_u8().await?;
        let atyp = reader.read_u8().await?;

        let address = match atyp {
            0x01 => {
                let mut octets = [0u8; 4];
                reader.read_exact(&mut octets).await?;
                Socks5Address::Ipv4(Ipv4Addr::from(octets))
            }
            0x04 => {
                let mut octets = [0u8; 16];
                reader.read_exact(&mut octets).await?;
                Socks5Address::Ipv6(Ipv6Addr::from(octets))
            }
            0x03 => {
                let len = reader.read_u8().await?;
                let mut domain = vec![0u8; len as usize];
                reader.read_exact(&mut domain).await?;
                Socks5Address::Domain(String::from_utf8_lossy(&domain).into_owned())
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported address type: {atyp}"),
                ))
            }
        };

        let port = reader.read_u16().await?;

        Ok(Self {
            version,
            command,
            address,
            port,
        })
    }
}

#[derive(Debug)]
pub struct Socks5Reply {
    pub version: u8,
    pub reply: u8,
    pub address: Socks5Address,
    pub port: u16,
}

impl Socks5Reply {
    pub fn success(address: Socks5Address, port: u16) -> Self {
        Self {
            version: 0x05,
            reply: 0x00,
            address,
            port,
        }
    }

    pub fn connection_refused() -> Self {
        Self {
            version: 0x05,
            reply: 0x05,
            address: Socks5Address::Ipv4(Ipv4Addr::UNSPECIFIED),
            port: 0,
        }
    }

    pub fn command_not_supported() -> Self {
        Self {
            version: 0x05,
            reply: 0x07,
            address: Socks5Address::Ipv4(Ipv4Addr::UNSPECIFIED),
            port: 0,
        }
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_u8(self.version).await?;
        writer.write_u8(self.reply).await?;
        writer.write_u8(0x00).await?;
        match &self.address {
            Socks5Address::Ipv4(addr) => {
                writer.write_u8(0x01).await?;
                writer.write_all(&addr.octets()).await?;
            }
            Socks5Address::Ipv6(addr) => {
                writer.write_u8(0x04).await?;
                writer.write_all(&addr.octets()).await?;
            }
            Socks5Address::Domain(name) => {
                writer.write_u8(0x03).await?;
                writer.write_u8(name.len() as u8).await?;
                writer.write_all(name.as_bytes()).await?;
            }
        }
        writer.write_u16(self.port).await?;
        writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn parse_version_method_no_auth() {
        let data = [0x05, 0x01, 0x00];
        let mut cursor = Cursor::new(&data[..]);
        let vm = Socks5VersionMethod::read_from(&mut cursor).await.unwrap();
        assert_eq!(vm.version, 0x05);
        assert_eq!(vm.methods, vec![0x00]);
    }

    #[tokio::test]
    async fn parse_version_method_multiple() {
        let data = [0x05, 0x02, 0x00, 0x02];
        let mut cursor = Cursor::new(&data[..]);
        let vm = Socks5VersionMethod::read_from(&mut cursor).await.unwrap();
        assert_eq!(vm.version, 0x05);
        assert_eq!(vm.methods, vec![0x00, 0x02]);
    }

    #[tokio::test]
    async fn parse_request_ipv4() {
        let mut data = vec![0x05, 0x01, 0x00, 0x01];
        data.extend_from_slice(&[10, 0, 0, 1]);
        data.extend_from_slice(&443u16.to_be_bytes());
        let mut cursor = Cursor::new(&data[..]);
        let req = Socks5Request::read_from(&mut cursor).await.unwrap();
        assert_eq!(req.version, 0x05);
        assert_eq!(req.command, 0x01);
        assert_eq!(
            req.address,
            Socks5Address::Ipv4(Ipv4Addr::new(10, 0, 0, 1))
        );
        assert_eq!(req.port, 443);
    }

    #[tokio::test]
    async fn parse_request_ipv6() {
        let mut data = vec![0x05, 0x01, 0x00, 0x04];
        let octets: [u8; 16] = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        data.extend_from_slice(&octets);
        data.extend_from_slice(&443u16.to_be_bytes());
        let mut cursor = Cursor::new(&data[..]);
        let req = Socks5Request::read_from(&mut cursor).await.unwrap();
        assert_eq!(req.version, 0x05);
        assert_eq!(req.command, 0x01);
        assert!(matches!(req.address, Socks5Address::Ipv6(_)));
        assert_eq!(req.port, 443);
    }

    #[tokio::test]
    async fn parse_request_domain() {
        let domain = "example.com";
        let mut data = vec![0x05, 0x01, 0x00, 0x03];
        data.push(domain.len() as u8);
        data.extend_from_slice(domain.as_bytes());
        data.extend_from_slice(&443u16.to_be_bytes());
        let mut cursor = Cursor::new(&data[..]);
        let req = Socks5Request::read_from(&mut cursor).await.unwrap();
        assert_eq!(req.version, 0x05);
        assert_eq!(req.command, 0x01);
        assert_eq!(req.address, Socks5Address::Domain("example.com".to_string()));
        assert_eq!(req.port, 443);
    }

    #[tokio::test]
    async fn parse_request_unsupported_address_type() {
        let data = [0x05, 0x01, 0x00, 0x05];
        let mut cursor = Cursor::new(&data[..]);
        let result = Socks5Request::read_from(&mut cursor).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reply_success_ipv4() {
        let reply = Socks5Reply::success(Socks5Address::Ipv4(Ipv4Addr::UNSPECIFIED), 0);
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();
        assert_eq!(buf[0], 0x05);
        assert_eq!(buf[1], 0x00);
        assert_eq!(buf[2], 0x00);
        assert_eq!(buf[3], 0x01);
    }

    #[tokio::test]
    async fn reply_connection_refused() {
        let reply = Socks5Reply::connection_refused();
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();
        assert_eq!(buf[0], 0x05);
        assert_eq!(buf[1], 0x05);
    }

    #[tokio::test]
    async fn reply_command_not_supported() {
        let reply = Socks5Reply::command_not_supported();
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();
        assert_eq!(buf[0], 0x05);
        assert_eq!(buf[1], 0x07);
    }

    #[tokio::test]
    async fn roundtrip_ipv4_reply() {
        let reply = Socks5Reply::success(Socks5Address::Ipv4(Ipv4Addr::new(127, 0, 0, 1)), 1080);
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();

        let mut cursor = Cursor::new(&buf[..]);
        let version = cursor.read_u8().await.unwrap();
        let _reply_code = cursor.read_u8().await.unwrap();
        let _rsv = cursor.read_u8().await.unwrap();
        let atyp = cursor.read_u8().await.unwrap();
        assert_eq!(version, 0x05);
        assert_eq!(atyp, 0x01);
        let mut octets = [0u8; 4];
        cursor.read_exact(&mut octets).await.unwrap();
        assert_eq!(Ipv4Addr::from(octets), Ipv4Addr::new(127, 0, 0, 1));
        let port = cursor.read_u16().await.unwrap();
        assert_eq!(port, 1080);
    }

    #[tokio::test]
    async fn roundtrip_ipv6_reply() {
        let addr = Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1);
        let reply = Socks5Reply::success(Socks5Address::Ipv6(addr), 443);
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();

        let mut cursor = Cursor::new(&buf[..]);
        let _version = cursor.read_u8().await.unwrap();
        let _reply_code = cursor.read_u8().await.unwrap();
        let _rsv = cursor.read_u8().await.unwrap();
        let atyp = cursor.read_u8().await.unwrap();
        assert_eq!(atyp, 0x04);
        let mut octets = [0u8; 16];
        cursor.read_exact(&mut octets).await.unwrap();
        assert_eq!(Ipv6Addr::from(octets), addr);
        let port = cursor.read_u16().await.unwrap();
        assert_eq!(port, 443);
    }

    #[tokio::test]
    async fn roundtrip_domain_reply() {
        let reply = Socks5Reply::success(Socks5Address::Domain("example.com".to_string()), 8080);
        let mut buf = Vec::new();
        reply.write_to(&mut buf).await.unwrap();

        let mut cursor = Cursor::new(&buf[..]);
        let _version = cursor.read_u8().await.unwrap();
        let _reply_code = cursor.read_u8().await.unwrap();
        let _rsv = cursor.read_u8().await.unwrap();
        let atyp = cursor.read_u8().await.unwrap();
        assert_eq!(atyp, 0x03);
        let len = cursor.read_u8().await.unwrap();
        let mut domain = vec![0u8; len as usize];
        cursor.read_exact(&mut domain).await.unwrap();
        assert_eq!(String::from_utf8(domain).unwrap(), "example.com");
        let port = cursor.read_u16().await.unwrap();
        assert_eq!(port, 8080);
    }
}