use alknet_core::testutil::{MockTransport, MockTransportAcceptor, Transport, TransportAcceptor, mock_pair};

#[tokio::test]
async fn mock_transport_connect() {
    let transport = MockTransport::new(1024);
    let stream = transport.connect().await.unwrap();
    drop(stream);
}

#[tokio::test]
async fn mock_transport_acceptor_accept() {
    let acceptor = MockTransportAcceptor::new(1024);
    let (stream, info) = acceptor.accept().await.unwrap();
    drop(stream);
    drop(info);
}

#[tokio::test]
async fn mock_pair_communicates() {
    let (mut client, mut server) = mock_pair(1024);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    client.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    server.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");
}