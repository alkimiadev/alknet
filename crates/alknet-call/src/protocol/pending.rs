use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::protocol::wire::CallError;

const SUBSCRIBE_CHANNEL_CAPACITY: usize = 32;

pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

enum PendingEntry {
    Call {
        tx: oneshot::Sender<Result<Value, CallError>>,
        timeout: Instant,
    },
    Subscribe {
        tx: mpsc::Sender<Result<Value, CallError>>,
        timeout: Option<Instant>,
    },
}

impl PendingRequestMap {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    pub fn register_call(
        &mut self,
        request_id: String,
        timeout: Instant,
    ) -> oneshot::Receiver<Result<Value, CallError>> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .insert(request_id, PendingEntry::Call { tx, timeout });
        rx
    }

    pub fn register_subscribe(
        &mut self,
        request_id: String,
        timeout: Option<Instant>,
    ) -> mpsc::Receiver<Result<Value, CallError>> {
        let (tx, rx) = mpsc::channel(SUBSCRIBE_CHANNEL_CAPACITY);
        self.pending
            .insert(request_id, PendingEntry::Subscribe { tx, timeout });
        rx
    }

    pub fn handle_responded(&mut self, request_id: &str, output: Value) -> bool {
        let Some(entry) = self.pending.remove(request_id) else {
            return false;
        };
        match entry {
            PendingEntry::Call { tx, .. } => {
                let _ = tx.send(Ok(output));
                true
            }
            PendingEntry::Subscribe { tx, timeout } => {
                let send_result = tx.try_send(Ok(output));
                match send_result {
                    Ok(()) => {
                        self.pending.insert(
                            request_id.to_string(),
                            PendingEntry::Subscribe { tx, timeout },
                        );
                        true
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(
                            request_id,
                            "subscribe channel full; dropping entry and closing subscription"
                        );
                        true
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => true,
                }
            }
        }
    }

    pub fn handle_completed(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id).is_some()
    }

    pub fn handle_aborted(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id).is_some()
    }

    pub fn handle_error(&mut self, request_id: &str, error: CallError) -> bool {
        let Some(entry) = self.pending.remove(request_id) else {
            return false;
        };
        match entry {
            PendingEntry::Call { tx, .. } => {
                let _ = tx.send(Err(error));
                true
            }
            PendingEntry::Subscribe { tx, .. } => {
                let _ = tx.try_send(Err(error));
                true
            }
        }
    }

    pub fn evict_expired(&mut self) -> Vec<String> {
        let now = Instant::now();
        let mut evicted = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        for (id, entry) in self.pending.iter() {
            let expired = match entry {
                PendingEntry::Call { timeout, .. } => *timeout <= now,
                PendingEntry::Subscribe {
                    timeout: Some(t), ..
                } => *t <= now,
                PendingEntry::Subscribe { timeout: None, .. } => false,
            };
            if expired {
                to_remove.push(id.clone());
            }
        }
        for id in to_remove {
            let Some(entry) = self.pending.remove(&id) else {
                continue;
            };
            let timeout_err = CallError::timeout("request timed out");
            match entry {
                PendingEntry::Call { tx, .. } => {
                    let _ = tx.send(Err(timeout_err));
                }
                PendingEntry::Subscribe { tx, .. } => {
                    let _ = tx.try_send(Err(timeout_err));
                }
            }
            evicted.push(id);
        }
        evicted
    }

    pub fn fail_all(&mut self, error: CallError) -> Vec<String> {
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in &ids {
            if let Some(entry) = self.pending.remove(id) {
                match entry {
                    PendingEntry::Call { tx, .. } => {
                        let _ = tx.send(Err(error.clone()));
                    }
                    PendingEntry::Subscribe { tx, .. } => {
                        let _ = tx.try_send(Err(error.clone()));
                    }
                }
            }
        }
        ids
    }

    pub fn contains(&self, request_id: &str) -> bool {
        self.pending.contains_key(request_id)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

impl Default for PendingRequestMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::timeout;

    fn timeout_error() -> CallError {
        CallError::timeout("request timed out")
    }

    fn internal_error(message: &str) -> CallError {
        CallError::internal(message)
    }

    #[tokio::test]
    async fn register_call_then_handle_responded_resolves_oneshot() {
        let mut map = PendingRequestMap::new();
        let rx = map.register_call(
            "req-1".to_string(),
            Instant::now() + Duration::from_secs(30),
        );

        assert!(map.contains("req-1"));
        assert_eq!(map.len(), 1);

        assert!(map.handle_responded("req-1", json!(42)));

        let result = timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Ok(value))) => assert_eq!(value, json!(42)),
            other => panic!("expected Ok(42), got {other:?}"),
        }
        assert!(!map.contains("req-1"));
        assert_eq!(map.len(), 0);
    }

    #[tokio::test]
    async fn register_subscribe_then_handle_responded_pushes_to_channel() {
        let mut map = PendingRequestMap::new();
        let mut rx = map.register_subscribe("sub-1".to_string(), None);

        assert!(map.handle_responded("sub-1", json!("first")));
        assert!(map.handle_responded("sub-1", json!("second")));
        assert!(map.contains("sub-1"));

        let first = timeout(Duration::from_millis(100), rx.recv()).await;
        let second = timeout(Duration::from_millis(100), rx.recv()).await;
        match (first, second) {
            (Ok(Some(Ok(a))), Ok(Some(Ok(b)))) => {
                assert_eq!(a, json!("first"));
                assert_eq!(b, json!("second"));
            }
            other => panic!("expected two Ok values, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_handle_completed_closes_channel_and_deletes_entry() {
        let mut map = PendingRequestMap::new();
        let mut rx = map.register_subscribe("sub-2".to_string(), None);

        assert!(map.handle_responded("sub-2", json!("a")));
        assert!(map.handle_completed("sub-2"));
        assert!(!map.contains("sub-2"));

        let _ = timeout(Duration::from_millis(100), rx.recv()).await;
        let after_close = timeout(Duration::from_millis(100), rx.recv()).await;
        match after_close {
            Ok(None) => {}
            other => panic!("expected channel closed (None), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn expired_call_is_evicted_with_timeout_error() {
        let mut map = PendingRequestMap::new();
        let rx = map.register_call(
            "req-2".to_string(),
            Instant::now() - Duration::from_millis(1),
        );

        let evicted = map.evict_expired();
        assert_eq!(evicted, vec!["req-2".to_string()]);
        assert!(!map.contains("req-2"));

        let result = timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Err(e))) => {
                assert_eq!(e.code, "TIMEOUT");
                assert!(e.retryable);
            }
            other => panic!("expected Err(TIMEOUT), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn expired_subscribe_is_evicted_with_timeout_error() {
        let mut map = PendingRequestMap::new();
        let mut rx = map.register_subscribe(
            "sub-3".to_string(),
            Some(Instant::now() - Duration::from_millis(1)),
        );

        let evicted = map.evict_expired();
        assert_eq!(evicted, vec!["sub-3".to_string()]);

        let result = timeout(Duration::from_millis(100), rx.recv()).await;
        match result {
            Ok(Some(Err(e))) => {
                assert_eq!(e.code, "TIMEOUT");
                assert!(e.retryable);
            }
            other => panic!("expected Err(TIMEOUT), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unbounded_subscribe_is_not_evicted() {
        let mut map = PendingRequestMap::new();
        let _rx = map.register_subscribe("sub-4".to_string(), None);

        let evicted = map.evict_expired();
        assert!(evicted.is_empty());
        assert!(map.contains("sub-4"));
    }

    #[tokio::test]
    async fn fail_all_resolves_all_pending_with_internal_error() {
        let mut map = PendingRequestMap::new();
        let rx_call =
            map.register_call("c-1".to_string(), Instant::now() + Duration::from_secs(30));
        let mut rx_sub = map.register_subscribe(
            "s-1".to_string(),
            Some(Instant::now() + Duration::from_secs(30)),
        );

        let failed = map.fail_all(internal_error("connection closed"));
        assert_eq!(failed.len(), 2);
        assert!(failed.contains(&"c-1".to_string()));
        assert!(failed.contains(&"s-1".to_string()));
        assert!(map.is_empty());

        let call_result = timeout(Duration::from_millis(100), rx_call).await;
        match call_result {
            Ok(Ok(Err(e))) => {
                assert_eq!(e.code, "INTERNAL");
                assert_eq!(e.message, "connection closed");
            }
            other => panic!("expected Err(INTERNAL), got {other:?}"),
        }

        let sub_result = timeout(Duration::from_millis(100), rx_sub.recv()).await;
        match sub_result {
            Ok(Some(Err(e))) => {
                assert_eq!(e.code, "INTERNAL");
                assert_eq!(e.message, "connection closed");
            }
            other => panic!("expected Err(INTERNAL), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_responded_unknown_request_id_returns_false() {
        let mut map = PendingRequestMap::new();
        assert!(!map.handle_responded("nonexistent", json!(1)));
        assert_eq!(map.len(), 0);
    }

    #[tokio::test]
    async fn handle_completed_unknown_request_id_returns_false() {
        let mut map = PendingRequestMap::new();
        assert!(!map.handle_completed("nonexistent"));
    }

    #[tokio::test]
    async fn handle_aborted_unknown_request_id_returns_false() {
        let mut map = PendingRequestMap::new();
        assert!(!map.handle_aborted("nonexistent"));
    }

    #[tokio::test]
    async fn handle_error_unknown_request_id_returns_false() {
        let mut map = PendingRequestMap::new();
        assert!(!map.handle_error("nonexistent", internal_error("x")));
    }

    #[tokio::test]
    async fn handle_aborted_cancels_pending_call() {
        let mut map = PendingRequestMap::new();
        let rx = map.register_call(
            "req-3".to_string(),
            Instant::now() + Duration::from_secs(30),
        );

        assert!(map.handle_aborted("req-3"));
        assert!(!map.contains("req-3"));

        let result = timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Err(_)) => {}
            other => panic!("expected sender dropped (Err), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_error_resolves_call_with_error() {
        let mut map = PendingRequestMap::new();
        let rx = map.register_call(
            "req-4".to_string(),
            Instant::now() + Duration::from_secs(30),
        );

        let err = CallError::new("FILE_NOT_FOUND", "missing", false);
        assert!(map.handle_error("req-4", err.clone()));
        assert!(!map.contains("req-4"));

        let result = timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Err(e))) => {
                assert_eq!(e.code, "FILE_NOT_FOUND");
                assert!(!e.retryable);
            }
            other => panic!("expected Err(FILE_NOT_FOUND), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_error_pushes_to_subscribe_channel() {
        let mut map = PendingRequestMap::new();
        let mut rx = map.register_subscribe("sub-5".to_string(), None);

        let err = CallError::new("RATE_LIMITED", "too fast", true);
        assert!(map.handle_error("sub-5", err.clone()));
        assert!(!map.contains("sub-5"));

        let result = timeout(Duration::from_millis(100), rx.recv()).await;
        match result {
            Ok(Some(Err(e))) => {
                assert_eq!(e.code, "RATE_LIMITED");
                assert!(e.retryable);
            }
            other => panic!("expected Err(RATE_LIMITED), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn correlation_by_id_not_by_stream() {
        let mut map = PendingRequestMap::new();
        let rx = map.register_call(
            "req-stream-3".to_string(),
            Instant::now() + Duration::from_secs(30),
        );

        assert!(map.handle_responded("req-stream-3", json!("response-from-stream-7")));
        let result = timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Ok(value))) => assert_eq!(value, json!("response-from-stream-7")),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_call_overwrites_existing_entry() {
        let mut map = PendingRequestMap::new();
        let _rx_old = map.register_call(
            "req-5".to_string(),
            Instant::now() + Duration::from_secs(30),
        );
        let rx_new = map.register_call(
            "req-5".to_string(),
            Instant::now() + Duration::from_secs(30),
        );
        assert_eq!(map.len(), 1);

        assert!(map.handle_responded("req-5", json!("new")));
        let result = timeout(Duration::from_millis(100), rx_new).await;
        match result {
            Ok(Ok(Ok(value))) => assert_eq!(value, json!("new")),
            other => panic!("expected Ok from new receiver, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn evict_expired_skips_non_expired_entries() {
        let mut map = PendingRequestMap::new();
        let _rx_expired = map.register_call(
            "expired".to_string(),
            Instant::now() - Duration::from_millis(1),
        );
        let _rx_alive = map.register_call(
            "alive".to_string(),
            Instant::now() + Duration::from_secs(60),
        );

        let evicted = map.evict_expired();
        assert_eq!(evicted, vec!["expired".to_string()]);
        assert!(map.contains("alive"));
        assert!(!map.contains("expired"));
    }

    #[tokio::test]
    async fn default_is_empty_map() {
        let map = PendingRequestMap::default();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[tokio::test]
    async fn timeout_error_helper() {
        let err = timeout_error();
        assert_eq!(err.code, "TIMEOUT");
        assert!(err.retryable);
    }
}
