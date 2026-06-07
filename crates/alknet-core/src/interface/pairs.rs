use crate::transport::TransportKind;

use super::config::InterfaceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKindBase {
    Tcp,
    Tls,
    Iroh,
    Dns,
    WebTransport,
}

fn transport_base(kind: &TransportKind) -> TransportKindBase {
    match kind {
        TransportKind::Tcp => TransportKindBase::Tcp,
        TransportKind::Tls { .. } => TransportKindBase::Tls,
        TransportKind::Iroh { .. } => TransportKindBase::Iroh,
        TransportKind::Dns { .. } => TransportKindBase::Dns,
        TransportKind::WebTransport { .. } => TransportKindBase::WebTransport,
    }
}

pub fn is_valid_pair(transport: &TransportKind, interface: InterfaceKind) -> bool {
    let base = transport_base(transport);
    matches!(
        (base, interface),
        (TransportKindBase::Tcp, InterfaceKind::Ssh)
            | (TransportKindBase::Tls, InterfaceKind::Ssh)
            | (TransportKindBase::Iroh, InterfaceKind::Ssh)
            | (TransportKindBase::Dns, InterfaceKind::RawFraming)
            | (TransportKindBase::WebTransport, InterfaceKind::Ssh)
            | (TransportKindBase::WebTransport, InterfaceKind::RawFraming)
            | (TransportKindBase::Tcp, InterfaceKind::RawFraming)
    )
}

pub const VALID_TRANSPORT_INTERFACE_PAIRS: &[(TransportKindBase, InterfaceKind)] = &[
    (TransportKindBase::Tcp, InterfaceKind::Ssh),
    (TransportKindBase::Tls, InterfaceKind::Ssh),
    (TransportKindBase::Iroh, InterfaceKind::Ssh),
    (TransportKindBase::Dns, InterfaceKind::RawFraming),
    (TransportKindBase::WebTransport, InterfaceKind::Ssh),
    (TransportKindBase::WebTransport, InterfaceKind::RawFraming),
    (TransportKindBase::Tcp, InterfaceKind::RawFraming),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ssh_pairs() {
        assert!(is_valid_pair(&TransportKind::Tcp, InterfaceKind::Ssh));
        assert!(is_valid_pair(
            &TransportKind::Tls { server_name: None },
            InterfaceKind::Ssh
        ));
        assert!(is_valid_pair(
            &TransportKind::Iroh {
                endpoint_id: String::new()
            },
            InterfaceKind::Ssh
        ));
        assert!(is_valid_pair(
            &TransportKind::WebTransport {
                host: String::new()
            },
            InterfaceKind::Ssh
        ));
    }

    #[test]
    fn valid_raw_framing_pairs() {
        assert!(is_valid_pair(
            &TransportKind::Tcp,
            InterfaceKind::RawFraming
        ));
        assert!(is_valid_pair(
            &TransportKind::Dns {
                domain: String::new()
            },
            InterfaceKind::RawFraming
        ));
        assert!(is_valid_pair(
            &TransportKind::WebTransport {
                host: String::new()
            },
            InterfaceKind::RawFraming
        ));
    }

    #[test]
    fn invalid_pairs() {
        assert!(!is_valid_pair(
            &TransportKind::Dns {
                domain: String::new()
            },
            InterfaceKind::Ssh
        ));
        assert!(!is_valid_pair(
            &TransportKind::Iroh {
                endpoint_id: String::new()
            },
            InterfaceKind::RawFraming
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
            transport_base(&TransportKind::Dns {
                domain: "example.com".to_string()
            }),
            TransportKindBase::Dns
        );
        assert_eq!(
            transport_base(&TransportKind::WebTransport {
                host: "example.com".to_string()
            }),
            TransportKindBase::WebTransport
        );
    }

    #[test]
    fn valid_pairs_table_complete() {
        assert_eq!(VALID_TRANSPORT_INTERFACE_PAIRS.len(), 7);
    }
}
