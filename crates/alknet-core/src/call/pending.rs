use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::call::response::CallError;

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

pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

impl PendingRequestMap {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    pub fn insert_call(
        &mut self,
        request_id: impl Into<String>,
        tx: oneshot::Sender<Result<Value, CallError>>,
        timeout: Instant,
    ) {
        self.pending
            .insert(request_id.into(), PendingEntry::Call { tx, timeout });
    }

    pub fn insert_subscribe(
        &mut self,
        request_id: impl Into<String>,
        tx: mpsc::Sender<Result<Value, CallError>>,
        timeout: Option<Instant>,
    ) {
        self.pending
            .insert(request_id.into(), PendingEntry::Subscribe { tx, timeout });
    }

    pub fn resolve_call(&mut self, request_id: &str, value: Result<Value, CallError>) -> bool {
        if let Some(PendingEntry::Call { tx, .. }) = self.pending.remove(request_id) {
            let _ = tx.send(value);
            true
        } else {
            false
        }
    }

    pub fn push_subscribe(&mut self, request_id: &str, value: Result<Value, CallError>) -> bool {
        match self.pending.get_mut(request_id) {
            Some(PendingEntry::Subscribe { tx, .. }) => tx.try_send(value).is_ok(),
            _ => false,
        }
    }

    pub fn complete_subscribe(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id).is_some()
    }

    pub fn abort(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id).is_some()
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

    pub fn sweep_expired(&mut self, now: Instant) -> usize {
        let expired: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, entry)| match entry {
                PendingEntry::Call { timeout, .. } => *timeout <= now,
                PendingEntry::Subscribe { timeout, .. } => timeout.is_some_and(|t| t <= now),
            })
            .map(|(id, _)| id.clone())
            .collect();
        let count = expired.len();
        for id in &expired {
            self.pending.remove(id);
        }
        count
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
    use std::time::Duration;

    #[tokio::test]
    async fn pending_request_map_insert_and_resolve_call() {
        let mut map = PendingRequestMap::new();
        let (tx, rx) = oneshot::channel();
        let timeout = Instant::now() + Duration::from_secs(30);
        map.insert_call("req-1", tx, timeout);
        assert!(map.contains("req-1"));
        assert_eq!(map.len(), 1);

        let result = map.resolve_call("req-1", Ok(serde_json::json!({"status": "ok"})));
        assert!(result);
        assert!(map.is_empty());

        let response = rx.await.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap(), serde_json::json!({"status": "ok"}));
    }

    #[tokio::test]
    async fn pending_request_map_resolve_unknown_call() {
        let mut map = PendingRequestMap::new();
        let result = map.resolve_call("unknown", Ok(serde_json::json!(null)));
        assert!(!result);
    }

    #[tokio::test]
    async fn pending_request_map_insert_and_push_subscribe() {
        let mut map = PendingRequestMap::new();
        let (tx, mut rx) = mpsc::channel(16);
        map.insert_subscribe("sub-1", tx, None);
        assert!(map.contains("sub-1"));

        let pushed = map.push_subscribe("sub-1", Ok(serde_json::json!({"item": 1})));
        assert!(pushed);

        let response = rx.recv().await.unwrap();
        assert!(response.is_ok());
        assert_eq!(response.unwrap(), serde_json::json!({"item": 1}));
    }

    #[tokio::test]
    async fn pending_request_map_complete_subscribe() {
        let mut map = PendingRequestMap::new();
        let (tx, mut rx) = mpsc::channel(16);
        map.insert_subscribe("sub-1", tx, None);

        map.push_subscribe("sub-1", Ok(serde_json::json!({"item": 1})));
        let completed = map.complete_subscribe("sub-1");
        assert!(completed);
        assert!(map.is_empty());

        let _ = rx.recv().await;
    }

    #[tokio::test]
    async fn pending_request_map_abort_call() {
        let mut map = PendingRequestMap::new();
        let (tx, _rx) = oneshot::channel();
        let timeout = Instant::now() + Duration::from_secs(30);
        map.insert_call("req-1", tx, timeout);

        let aborted = map.abort("req-1");
        assert!(aborted);
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn pending_request_map_abort_unknown() {
        let mut map = PendingRequestMap::new();
        let aborted = map.abort("unknown");
        assert!(!aborted);
    }

    #[tokio::test]
    async fn pending_request_map_sweep_expired() {
        let mut map = PendingRequestMap::new();
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();
        let past = Instant::now() - Duration::from_secs(1);
        let future = Instant::now() + Duration::from_secs(30);

        map.insert_call("expired-1", tx1, past);
        map.insert_call("active-1", tx2, future);

        let swept = map.sweep_expired(Instant::now());
        assert_eq!(swept, 1);
        assert!(!map.contains("expired-1"));
        assert!(map.contains("active-1"));
    }

    #[tokio::test]
    async fn pending_request_map_sweep_subscribe_with_timeout() {
        let mut map = PendingRequestMap::new();
        let (tx1, _rx1) = mpsc::channel(16);
        let (tx2, _rx2) = mpsc::channel(16);
        let past = Some(Instant::now() - Duration::from_secs(1));
        let future = Some(Instant::now() + Duration::from_secs(30));

        map.insert_subscribe("expired-sub", tx1, past);
        map.insert_subscribe("active-sub", tx2, future);

        let swept = map.sweep_expired(Instant::now());
        assert_eq!(swept, 1);
        assert!(!map.contains("expired-sub"));
        assert!(map.contains("active-sub"));
    }

    #[tokio::test]
    async fn pending_request_map_subscribe_no_timeout_not_swept() {
        let mut map = PendingRequestMap::new();
        let (tx, _rx) = mpsc::channel(16);
        map.insert_subscribe("sub-no-timeout", tx, None);

        let swept = map.sweep_expired(Instant::now());
        assert_eq!(swept, 0);
        assert!(map.contains("sub-no-timeout"));
    }

    #[tokio::test]
    async fn pending_request_map_push_unknown_subscribe() {
        let mut map = PendingRequestMap::new();
        let pushed = map.push_subscribe("unknown", Ok(serde_json::json!(null)));
        assert!(!pushed);
    }

    #[tokio::test]
    async fn pending_request_map_call_error_response() {
        let mut map = PendingRequestMap::new();
        let (tx, rx) = oneshot::channel();
        let timeout = Instant::now() + Duration::from_secs(30);
        map.insert_call("req-err", tx, timeout);

        let result = map.resolve_call(
            "req-err",
            Err(CallError {
                code: "TIMEOUT".to_string(),
                message: "request timed out".to_string(),
                retryable: true,
            }),
        );
        assert!(result);
        assert!(map.is_empty());

        let response = rx.await.unwrap();
        assert!(response.is_err());
        let err = response.unwrap_err();
        assert_eq!(err.code, "TIMEOUT");
        assert!(err.retryable);
    }
}
