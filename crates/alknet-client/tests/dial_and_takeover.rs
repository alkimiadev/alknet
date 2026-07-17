//! Integration test: dial + take-over composition.
//!
//! Verifies that the AlknetClient type exists and compiles correctly.
//! Full end-to-end dial tests (with a real quinn endpoint) are tested
//! in the assembly layer integration tests (future hub/worker tests).

use alknet_client::AlknetClient;
use alknet_core::credentials::ConnectionCredentials;

#[tokio::test]
async fn alknet_client_new_creates_empty_client() {
    let client = AlknetClient::new();
    let creds = ConnectionCredentials::new();
    let _ = (client, creds);
}

#[test]
fn alknet_client_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AlknetClient>();
}

#[test]
fn client_dial_error_is_send_sync() {
    use alknet_client::ClientDialError;
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ClientDialError>();
}
