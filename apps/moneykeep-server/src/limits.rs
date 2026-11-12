//! Rate limits (mobile-capture.md): per token 60 requests a minute and 200 MB of
//! uploads a day; per source address 10 failed authentications a minute, then a
//! 15-minute pause. Kept in memory: a restart forgets them, which is harmless.

use std::collections::HashMap;

use mk_capture_api::{
    AUTH_BACKOFF_SECS, AUTH_FAILURES_PER_MINUTE, REQUESTS_PER_MINUTE, UPLOAD_BYTES_PER_DAY,
};

#[derive(Debug, Default)]
struct Window {
    start: i64,
    count: u32,
}

#[derive(Debug, Default)]
struct Upload {
    day: i64,
    bytes: u64,
}

#[derive(Debug, Default)]
struct Failures {
    window: Window,
    blocked_until: i64,
}

/// What the limits say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Limited,
}

#[derive(Debug, Default)]
pub struct Limits {
    requests: HashMap<String, Window>,
    uploads: HashMap<String, Upload>,
    failures: HashMap<String, Failures>,
}

impl Limits {
    /// Count a request by `token_id`.
    pub fn request(&mut self, token_id: &str, now: i64) -> Verdict {
        let w = self.requests.entry(token_id.to_owned()).or_default();
        if now - w.start >= 60 {
            w.start = now;
            w.count = 0;
        }
        w.count += 1;
        if w.count > REQUESTS_PER_MINUTE {
            Verdict::Limited
        } else {
            Verdict::Ok
        }
    }

    /// Count `bytes` uploaded by `token_id` today.
    pub fn upload(&mut self, token_id: &str, bytes: u64, now: i64) -> Verdict {
        let day = now.div_euclid(86_400);
        let u = self.uploads.entry(token_id.to_owned()).or_default();
        if u.day != day {
            u.day = day;
            u.bytes = 0;
        }
        if u.bytes.saturating_add(bytes) > UPLOAD_BYTES_PER_DAY {
            return Verdict::Limited;
        }
        u.bytes = u.bytes.saturating_add(bytes);
        Verdict::Ok
    }

    /// Whether `source` is paused after too many failures.
    pub fn blocked(&self, source: &str, now: i64) -> bool {
        self.failures
            .get(source)
            .is_some_and(|f| f.blocked_until > now)
    }

    /// Count a failed authentication from `source`.
    pub fn failure(&mut self, source: &str, now: i64) {
        let f = self.failures.entry(source.to_owned()).or_default();
        if now - f.window.start >= 60 {
            f.window.start = now;
            f.window.count = 0;
        }
        f.window.count += 1;
        if f.window.count >= AUTH_FAILURES_PER_MINUTE {
            f.blocked_until = now + AUTH_BACKOFF_SECS;
            f.window.count = 0;
        }
    }

    /// Forget entries that no longer matter (bounded memory).
    pub fn tidy(&mut self, now: i64) {
        self.requests.retain(|_, w| now - w.start < 120);
        self.failures
            .retain(|_, f| f.blocked_until > now || now - f.window.start < 120);
        let day = now.div_euclid(86_400);
        self.uploads.retain(|_, u| u.day == day);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits() {
        let mut l = Limits::default();
        for _ in 0..60 {
            assert_eq!(l.request("t", 100), Verdict::Ok);
        }
        assert_eq!(l.request("t", 100), Verdict::Limited);
        assert_eq!(l.request("t", 160), Verdict::Ok);
        assert_eq!(l.upload("t", 150 << 20, 0), Verdict::Ok);
        assert_eq!(l.upload("t", 60 << 20, 10), Verdict::Limited);
        assert_eq!(l.upload("t", 60 << 20, 86_400), Verdict::Ok);
        for _ in 0..9 {
            l.failure("a", 0);
        }
        assert!(!l.blocked("a", 1));
        l.failure("a", 1);
        assert!(l.blocked("a", 2));
        assert!(l.blocked("a", 900));
        assert!(!l.blocked("a", 902));
    }
}
