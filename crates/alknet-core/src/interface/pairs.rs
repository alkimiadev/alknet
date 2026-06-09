use crate::transport::TransportKind;

use super::config::StreamInterfaceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKindBase {
    Tcp,
    Tls,
    Iroh,
    WebTransport,
}

fn transport_base(kind: &TransportKind) -> TransportKindBase {
    match kind {
        TransportKind::Tcp => TransportKindBase::Tcp,
        TransportKind::Tls { .. } => TransportKindBase::Tls,
        TransportKind::Iroh { .. } => TransportKindBase::Iroh,
        TransportKind::WebTransport { .. } => TransportKindBase::WebTransport,
    }
}

pub fn is_valid_pair(transport: &TransportKind, interface: StreamInterfaceKind) -> bool {
    let base = transport_base(transport);
    matches!(
        (base, interface),
        (TransportKindBase::Tcp, StreamInterfaceKind::Ssh)
            | (TransportKindBase::Tls, StreamInterfaceKind::Ssh)
            | (TransportKindBase::Iroh, StreamInterfaceKind::Ssh)
            | (TransportKindBase::WebTransport, StreamInterfaceKind::Ssh)
            | (
                TransportKindBase::WebTransport,
                StreamInterfaceKind::RawFraming
            )
            | (TransportKindBase::Tcp, StreamInterfaceKind::RawFraming)
    )
}

pub const VALID_TRANSPORT_INTERFACE_PAIRS: &[(TransportKindBase, StreamInterfaceKind)] = &[
    (TransportKindBase::Tcp, StreamInterfaceKind::Ssh),
    (TransportKindBase::Tls, StreamInterfaceKind::Ssh),
    (TransportKindBase::Iroh, StreamInterfaceKind::Ssh),
    (TransportKindBase::WebTransport, StreamInterfaceKind::Ssh),
    (
        TransportKindBase::WebTransport,
        StreamInterfaceKind::RawFraming,
    ),
    (TransportKindBase::Tcp, StreamInterfaceKind::RawFraming),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ssh_pairs() {
        assert!(is_valid_pair(&TransportKind::Tcp, StreamInterfaceKind::Ssh));
        assert!(is_valid_pair(
            &TransportKind::Tls { server_name: None },
            StreamInterfaceKind::Ssh
        ));
        assert!(is_valid_pair(
            &TransportKind::Iroh {
                endpoint_id: String::new()
            },
            StreamInterfaceKind::Ssh
        ));
        assert!(is_valid_pair(
            &TransportKind::WebTransport { server_name: None },
            StreamInterfaceKind::Ssh
        ));
    }

    #[test]
    fn valid_raw_framing_pairs() {
        assert!(is_valid_pair(
            &TransportKind::Tcp,
            StreamInterfaceKind::RawFraming
        ));
        assert!(is_valid_pair(
            &TransportKind::WebTransport { server_name: None },
            StreamInterfaceKind::RawFraming
        ));
    }

    #[test]
    fn invalid_pairs() {
        assert!(!is_valid_pair(
            &TransportKind::Iroh {
                endpoint_id: String::new()
            },
            StreamInterfaceKind::RawFraming
        ));
    }

    #[test]
    fn transport_kind_base_classification() {
        assert_eq!(transport_base(&TransportKind::Tcp), TransportKindBase::Tcp);
        assert_eq!(
            transport_base(&TransportKind::Tls {
                server_name: Some("example.com".to_string())
            }),
            TransportKindBase::Tls
        );
        assert_eq!(
            transport_base(&TransportKind::Iroh {
                endpoint_id: "abc".to_string()
            }),
            TransportKindBase::Iroh
        );
        assert_eq!(
            transport_base(&TransportKind::WebTransport {
                server_name: Some("example.com".to_string())
            }),
            TransportKindBase::WebTransport
        );
    }

    #[test]
    fn valid_pairs_table_complete() {
        assert_eq!(VALID_TRANSPORT_INTERFACE_PAIRS.len(), 6);
    }
}
