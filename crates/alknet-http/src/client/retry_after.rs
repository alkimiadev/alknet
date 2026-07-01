//! Inlined `RetryAfterMiddleware`: parses the `Retry-After` header on
//! 429/503 and sleeps before the next request to that URL.
//!
//! Inlined (MIT, from `melotic/reqwest-retry-after`) so the upstream's
//! unbounded `HashMap<Url, SystemTime>` storage can be bounded for a
//! long-running process. The bound is enforced via LRU eviction: when
//! the map is at capacity, the entry with the earliest deadline is
//! evicted first (those are the most likely to have already elapsed
//! and are the cheapest to drop).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use http::Extensions;
use reqwest::{Request, Response, StatusCode};
use reqwest_middleware::{Middleware, Next, Result};
use url::Url;

const RETRY_AFTER_HEADER: &str = "retry-after";
const THROTTLED_STATUS: &[u16] = &[StatusCode::TOO_MANY_REQUESTS.as_u16(), 503];

fn is_throttled(status: u16) -> bool {
    THROTTLED_STATUS.contains(&status)
}

fn parse_retry_after(value: &str) -> Option<SystemTime> {
    let trimmed = value.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return SystemTime::now()
            .checked_add(Duration::from_secs(secs))
            .filter(|deadline| *deadline > SystemTime::now());
    }
    httpdate::parse_http_date(trimmed)
        .ok()
        .filter(|deadline| *deadline > SystemTime::now())
}

pub struct RetryAfterMiddleware {
    deadlines: Mutex<HashMap<Url, SystemTime>>,
    capacity: usize,
}

impl RetryAfterMiddleware {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            deadlines: Mutex::new(HashMap::with_capacity(capacity.min(128))),
            capacity,
        }
    }

    fn record(&self, url: Url, deadline: SystemTime) {
        let mut deadlines = self.deadlines.lock().expect("deadlines mutex poisoned");
        if !deadlines.contains_key(&url) && deadlines.len() >= self.capacity {
            self.evict(&mut deadlines);
        }
        deadlines.insert(url, deadline);
    }

    fn evict(&self, deadlines: &mut HashMap<Url, SystemTime>) {
        if let Some(evict_url) = deadlines
            .iter()
            .min_by_key(|(_, deadline)| *deadline)
            .map(|(url, _)| url.clone())
        {
            deadlines.remove(&evict_url);
        }
    }

    fn deadline_for(&self, url: &Url) -> Option<SystemTime> {
        let deadlines = self.deadlines.lock().expect("deadlines mutex poisoned");
        deadlines.get(url).copied()
    }

    async fn maybe_sleep_for(&self, url: &Url) {
        if let Some(deadline) = self.deadline_for(url) {
            if let Ok(remaining) = deadline.duration_since(SystemTime::now()) {
                if !remaining.is_zero() {
                    tokio::time::sleep(remaining).await;
                }
            }
        }
    }

    fn record_if_throttled(&self, url: Url, response: &Response) {
        let status = response.status();
        if is_throttled(status.as_u16()) {
            if let Some(retry_after) = response
                .headers()
                .get(RETRY_AFTER_HEADER)
                .and_then(|value| value.to_str().ok())
            {
                if let Some(deadline) = parse_retry_after(retry_after) {
                    self.record(url, deadline);
                }
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.deadlines.lock().expect("deadlines mutex poisoned").len()
    }

    #[cfg(test)]
    fn deadline_for_test(&self, url: &Url) -> Option<SystemTime> {
        self.deadline_for(url)
    }

    #[cfg(test)]
    fn record_test(&self, url: Url, deadline: SystemTime) {
        self.record(url, deadline);
    }
}

#[async_trait::async_trait]
impl Middleware for RetryAfterMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let req_url = req.url().clone();
        self.maybe_sleep_for(&req_url).await;
        let response = next.run(req, extensions).await?;
        self.record_if_throttled(req_url, &response);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn synthetic_response(status: StatusCode, retry_after: Option<&str>) -> Response {
        let mut builder = http::Response::builder().status(status);
        if let Some(value) = retry_after {
            builder = builder.header(RETRY_AFTER_HEADER, value);
        }
        builder.body("").unwrap().into()
    }

    #[test]
    fn parse_retry_after_seconds() {
        let deadline = parse_retry_after("5").expect("seconds value parses");
        let now = SystemTime::now();
        let lower = now.checked_add(Duration::from_secs(4)).unwrap();
        let upper = now.checked_add(Duration::from_secs(6)).unwrap();
        assert!(deadline > lower && deadline < upper);
    }

    #[test]
    fn parse_retry_after_http_date() {
        let deadline = parse_retry_after("Wed, 21 Oct 2099 07:28:00 GMT")
            .expect("HTTP-date value parses");
        assert!(deadline > SystemTime::now());
    }

    #[test]
    fn parse_retry_after_past_http_date_yields_none() {
        let deadline = parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT");
        assert!(
            deadline.is_none(),
            "a deadline already in the past must not be recorded"
        );
    }

    #[test]
    fn parse_retry_after_invalid_yields_none() {
        assert!(parse_retry_after("not-a-date").is_none());
        assert!(parse_retry_after("").is_none());
    }

    #[test]
    fn record_stores_deadline_for_url() {
        let mw = RetryAfterMiddleware::with_capacity(8);
        let u = url("https://api.example.com/v1/chat");
        let deadline = SystemTime::now() + Duration::from_secs(10);
        mw.record_test(u.clone(), deadline);
        assert_eq!(mw.deadline_for_test(&u), Some(deadline));
        assert_eq!(mw.len(), 1);
    }

    #[test]
    fn record_evicts_oldest_when_at_capacity() {
        let mw = RetryAfterMiddleware::with_capacity(2);
        let u1 = url("https://a.example.com");
        let u2 = url("https://b.example.com");
        let u3 = url("https://c.example.com");
        mw.record_test(u1.clone(), SystemTime::now() + Duration::from_secs(100));
        mw.record_test(u2.clone(), SystemTime::now() + Duration::from_secs(1));
        assert_eq!(mw.len(), 2);
        mw.record_test(u3.clone(), SystemTime::now() + Duration::from_secs(50));
        assert_eq!(mw.len(), 2, "capacity must be enforced");
        assert!(
            mw.deadline_for_test(&u2).is_none(),
            "entry with the earliest deadline must be evicted"
        );
        assert!(mw.deadline_for_test(&u1).is_some());
        assert!(mw.deadline_for_test(&u3).is_some());
    }

    #[test]
    fn record_overwrites_existing_url_deadline_without_evicting() {
        let mw = RetryAfterMiddleware::with_capacity(2);
        let u = url("https://api.example.com/v1/chat");
        let far = url("https://far.example.com");
        mw.record_test(u.clone(), SystemTime::now() + Duration::from_secs(10));
        mw.record_test(far.clone(), SystemTime::now() + Duration::from_secs(20));
        mw.record_test(u.clone(), SystemTime::now() + Duration::from_secs(30));
        assert_eq!(
            mw.len(),
            2,
            "overwriting an existing URL must not evict another entry"
        );
        assert!(mw.deadline_for_test(&far).is_some());
    }

    #[tokio::test]
    async fn middleware_records_deadline_from_seconds_header() {
        let mw = std::sync::Arc::new(RetryAfterMiddleware::with_capacity(8));
        let target = url("https://api.example.com/v1/chat");
        let response = synthetic_response(StatusCode::TOO_MANY_REQUESTS, Some("5"));
        mw.record_if_throttled(target.clone(), &response);
        let deadline = mw
            .deadline_for_test(&target)
            .expect("429 with Retry-After records a deadline");
        let now = SystemTime::now();
        assert!(deadline > now, "deadline must be in the future");
        assert!(deadline < now + Duration::from_secs(6));
    }

    #[tokio::test]
    async fn middleware_records_deadline_from_http_date_header() {
        let mw = std::sync::Arc::new(RetryAfterMiddleware::with_capacity(8));
        let target = url("https://api.example.com/v1/chat");
        let response = synthetic_response(
            StatusCode::SERVICE_UNAVAILABLE,
            Some("Wed, 21 Oct 2099 07:28:00 GMT"),
        );
        mw.record_if_throttled(target.clone(), &response);
        let deadline = mw
            .deadline_for_test(&target)
            .expect("503 with Retry-After HTTP-date records a deadline");
        assert!(deadline > SystemTime::now());
    }

    #[tokio::test]
    async fn middleware_does_not_record_on_non_throttled_status() {
        let mw = std::sync::Arc::new(RetryAfterMiddleware::with_capacity(8));
        let target = url("https://api.example.com/v1/chat");
        let response = synthetic_response(StatusCode::OK, Some("5"));
        mw.record_if_throttled(target.clone(), &response);
        assert!(mw.deadline_for_test(&target).is_none());
    }

    #[tokio::test]
    async fn middleware_does_not_record_when_header_absent() {
        let mw = std::sync::Arc::new(RetryAfterMiddleware::with_capacity(8));
        let target = url("https://api.example.com/v1/chat");
        let response = synthetic_response(StatusCode::TOO_MANY_REQUESTS, None);
        mw.record_if_throttled(target.clone(), &response);
        assert!(mw.deadline_for_test(&target).is_none());
    }

    #[tokio::test]
    async fn middleware_sleeps_before_request_with_active_deadline() {
        let mw = std::sync::Arc::new(RetryAfterMiddleware::with_capacity(8));
        let target = url("https://api.example.com/v1/chat");
        mw.record_test(target.clone(), SystemTime::now() + Duration::from_millis(50));
        let started = SystemTime::now();
        mw.maybe_sleep_for(&target).await;
        let elapsed = SystemTime::now().duration_since(started).unwrap();
        assert!(
            elapsed >= Duration::from_millis(40),
            "middleware must sleep until the deadline elapses"
        );
    }
}