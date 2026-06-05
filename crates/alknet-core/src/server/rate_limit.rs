//! Connection rate limiting and auth attempt limiting.
//!
//! `ConnectionRateLimiter` tracks per-IP active connections (thread-safe).
//! `AuthAttemptLimiter` caps failed auth attempts per connection.
//! These complement fail2ban on Linux and provide abuse protection on all platforms.
//! See ADR-013.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

pub struct ConnectionRateLimiter {
    max_per_ip: usize,
    active: Mutex<HashMap<IpAddr, usize>>,
}

impl ConnectionRateLimiter {
    pub fn new(max_per_ip: usize) -> Self {
        Self {
            max_per_ip,
            active: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, ip: IpAddr) -> bool {
        if self.max_per_ip == 0 {
            return true;
        }
        let active = self.active.lock().unwrap();
        let count = active.get(&ip).copied().unwrap_or(0);
        count < self.max_per_ip
    }

    pub fn on_connect(&self, ip: IpAddr) {
        let mut active = self.active.lock().unwrap();
        *active.entry(ip).or_insert(0) += 1;
    }

    pub fn on_disconnect(&self, ip: IpAddr) {
        let mut active = self.active.lock().unwrap();
        if let Some(count) = active.get_mut(&ip) {
            if *count > 1 {
                *count -= 1;
            } else {
                active.remove(&ip);
            }
        }
    }
}

pub struct AuthAttemptLimiter {
    max_attempts: usize,
    failures: usize,
}

impl AuthAttemptLimiter {
    pub fn new(max_attempts: usize) -> Self {
        Self {
            max_attempts,
            failures: 0,
        }
    }

    pub fn check(&self) -> bool {
        if self.max_attempts == 0 {
            return true;
        }
        self.failures < self.max_attempts
    }

    pub fn on_failure(&mut self) {
        self.failures += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, n))
    }

    #[test]
    fn connection_limiter_allows_when_under_limit() {
        let limiter = ConnectionRateLimiter::new(3);
        assert!(limiter.check(ip(1)));
    }

    #[test]
    fn connection_limiter_blocks_when_at_limit() {
        let limiter = ConnectionRateLimiter::new(2);
        limiter.on_connect(ip(1));
        limiter.on_connect(ip(1));
        assert!(!limiter.check(ip(1)));
    }

    #[test]
    fn connection_limiter_allows_after_disconnect() {
        let limiter = ConnectionRateLimiter::new(2);
        limiter.on_connect(ip(1));
        limiter.on_connect(ip(1));
        assert!(!limiter.check(ip(1)));
        limiter.on_disconnect(ip(1));
        assert!(limiter.check(ip(1)));
    }

    #[test]
    fn connection_limiter_unlimited_when_zero() {
        let limiter = ConnectionRateLimiter::new(0);
        for _ in 0..100 {
            limiter.on_connect(ip(1));
        }
        assert!(limiter.check(ip(1)));
    }

    #[test]
    fn connection_limiter_tracks_per_ip_independently() {
        let limiter = ConnectionRateLimiter::new(1);
        limiter.on_connect(ip(1));
        assert!(!limiter.check(ip(1)));
        assert!(limiter.check(ip(2)));
    }

    #[test]
    fn connection_limiter_ipv6() {
        let limiter = ConnectionRateLimiter::new(1);
        let ip6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        limiter.on_connect(ip6);
        assert!(!limiter.check(ip6));
    }

    #[test]
    fn connection_limiter_disconnect_removes_zero_entry() {
        let limiter = ConnectionRateLimiter::new(3);
        limiter.on_connect(ip(1));
        limiter.on_disconnect(ip(1));
        {
            let active = limiter.active.lock().unwrap();
            assert!(!active.contains_key(&ip(1)));
        }
    }

    #[test]
    fn auth_limiter_allows_when_under_limit() {
        let limiter = AuthAttemptLimiter::new(3);
        assert!(limiter.check());
    }

    #[test]
    fn auth_limiter_blocks_after_max_failures() {
        let mut limiter = AuthAttemptLimiter::new(2);
        limiter.on_failure();
        limiter.on_failure();
        assert!(!limiter.check());
    }

    #[test]
    fn auth_limiter_unlimited_when_zero() {
        let mut limiter = AuthAttemptLimiter::new(0);
        for _ in 0..100 {
            limiter.on_failure();
        }
        assert!(limiter.check());
    }

    #[test]
    fn auth_limiter_still_allows_at_one_below_limit() {
        let mut limiter = AuthAttemptLimiter::new(3);
        limiter.on_failure();
        limiter.on_failure();
        assert!(limiter.check());
        limiter.on_failure();
        assert!(!limiter.check());
    }

    #[test]
    fn connection_limiter_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let limiter = Arc::new(ConnectionRateLimiter::new(100));
        let mut handles = vec![];

        for i in 0..10 {
            let lim = Arc::clone(&limiter);
            handles.push(thread::spawn(move || {
                let ip_addr = ip((i % 3) as u8 + 1);
                lim.on_connect(ip_addr);
                assert!(lim.check(ip_addr));
                lim.on_disconnect(ip_addr);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }
}