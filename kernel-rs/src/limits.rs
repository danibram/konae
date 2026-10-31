//! Per-client request limits for unauthenticated traffic.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::http::HeaderMap;

/// Anonymous `actions_invoke` calls allowed per client address and Deployment.
pub(crate) const ANONYMOUS_ACTIONS: u32 = 30;
pub(crate) const ANONYMOUS_ACTIONS_PERIOD: Duration = Duration::from_secs(10);
/// `actions_invoke` calls allowed per signed-in app user.
pub(crate) const SIGNED_IN_ACTIONS: u32 = 120;
pub(crate) const SIGNED_IN_ACTIONS_PERIOD: Duration = Duration::from_secs(10);
/// OAuth client registrations allowed per client address.
pub(crate) const REGISTRATIONS: u32 = 10;
pub(crate) const REGISTRATIONS_PERIOD: Duration = Duration::from_secs(60 * 60);
/// Login and signup attempts allowed per client address, for each app and
/// for owner sign-in. Each attempt runs a 600,000-iteration password hash.
pub(crate) const AUTH_CLIENT_ATTEMPTS: u32 = 10;
/// Login and signup attempts allowed per email address from one client
/// address, for each app and for owner sign-in.
pub(crate) const AUTH_EMAIL_ATTEMPTS: u32 = 5;
pub(crate) const AUTH_ATTEMPTS_PERIOD: Duration = Duration::from_secs(60);
/// Failed logins allowed per email address across all clients. Only wrong
/// passwords count, so a correct login never uses this up, but once failures
/// exhaust it every login for that email waits, including correct ones.
pub(crate) const AUTH_EMAIL_FAILURES: u32 = 100;
/// Failed logins one client address may add to an email's shared count, so
/// exhausting it takes failures from at least five addresses.
pub(crate) const AUTH_CLIENT_EMAIL_FAILURES: u32 = 20;
pub(crate) const AUTH_FAILURES_PERIOD: Duration = Duration::from_secs(60 * 60);
/// Unknown or revoked API keys one client address may present to a
/// Deployment per minute. Each runs a lookup in the Deployment's identity
/// store; once they are used up the Kernel refuses keys from that address
/// without looking, a valid one included, until the bucket refills.
pub(crate) const API_KEY_FAILURES: u32 = 10;
pub(crate) const API_KEY_FAILURES_PERIOD: Duration = Duration::from_secs(60);
/// Each limiter remembers at most this many clients.
const MAX_TRACKED_CLIENTS: usize = 10_000;

/// A token bucket per key. Memory stays bounded. Once the map reaches its
/// size limit, buckets that have refilled are dropped, since a fresh bucket
/// behaves the same. If every bucket is still draining, the oldest quarter goes.
pub(crate) struct RateLimiter {
    capacity: f64,
    per_second: f64,
    max_keys: usize,
    buckets: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl RateLimiter {
    pub(crate) fn new(capacity: u32, period: Duration, max_keys: usize) -> Self {
        Self {
            capacity: capacity as f64,
            per_second: capacity as f64 / period.as_secs_f64(),
            max_keys,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes one token for `key`. On refusal returns how long until one is free.
    pub(crate) fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        self.take_at(key, now, true)
    }

    /// Like `check`, but leaves the token in place.
    fn peek_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        self.take_at(key, now, false)
    }

    fn take_at(&self, key: &str, now: Instant, take: bool) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !take {
            let Some(bucket) = buckets.get(key) else {
                return Ok(());
            };
            let tokens = self.refilled(bucket, now);
            return if tokens >= 1.0 {
                Ok(())
            } else {
                Err(Duration::from_secs_f64((1.0 - tokens) / self.per_second))
            };
        }
        if !buckets.contains_key(key) && buckets.len() >= self.max_keys {
            self.evict(&mut buckets, now);
        }
        let bucket = buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: self.capacity,
            updated: now,
        });
        bucket.tokens = self.refilled(bucket, now);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - bucket.tokens) / self.per_second))
        }
    }

    fn refilled(&self, bucket: &Bucket, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        (bucket.tokens + elapsed * self.per_second).min(self.capacity)
    }

    fn evict(&self, buckets: &mut HashMap<String, Bucket>, now: Instant) {
        buckets.retain(|_, bucket| self.refilled(bucket, now) < self.capacity);
        if buckets.len() < self.max_keys {
            return;
        }
        let mut ages = buckets
            .iter()
            .map(|(key, bucket)| (bucket.updated, key.clone()))
            .collect::<Vec<_>>();
        ages.sort();
        for (_, key) in ages.into_iter().take(self.max_keys.div_ceil(4)) {
            buckets.remove(&key);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.lock().unwrap().len()
    }
}

pub(crate) struct Guards {
    pub(crate) anonymous_actions: RateLimiter,
    pub(crate) signed_in_actions: RateLimiter,
    pub(crate) registrations: RateLimiter,
    auth_clients: RateLimiter,
    auth_client_emails: RateLimiter,
    auth_email_failures: RateLimiter,
    auth_client_email_failures: RateLimiter,
    api_key_failures: RateLimiter,
    trusted_proxies: Vec<IpAddr>,
}

impl Guards {
    pub(crate) fn new(trusted_proxies: Vec<IpAddr>) -> Self {
        Self {
            anonymous_actions: RateLimiter::new(
                ANONYMOUS_ACTIONS,
                ANONYMOUS_ACTIONS_PERIOD,
                MAX_TRACKED_CLIENTS,
            ),
            signed_in_actions: RateLimiter::new(
                SIGNED_IN_ACTIONS,
                SIGNED_IN_ACTIONS_PERIOD,
                MAX_TRACKED_CLIENTS,
            ),
            registrations: RateLimiter::new(REGISTRATIONS, REGISTRATIONS_PERIOD, MAX_TRACKED_CLIENTS),
            auth_clients: RateLimiter::new(AUTH_CLIENT_ATTEMPTS, AUTH_ATTEMPTS_PERIOD, MAX_TRACKED_CLIENTS),
            auth_client_emails: RateLimiter::new(AUTH_EMAIL_ATTEMPTS, AUTH_ATTEMPTS_PERIOD, MAX_TRACKED_CLIENTS),
            auth_email_failures: RateLimiter::new(AUTH_EMAIL_FAILURES, AUTH_FAILURES_PERIOD, MAX_TRACKED_CLIENTS),
            auth_client_email_failures: RateLimiter::new(
                AUTH_CLIENT_EMAIL_FAILURES,
                AUTH_FAILURES_PERIOD,
                MAX_TRACKED_CLIENTS,
            ),
            api_key_failures: RateLimiter::new(API_KEY_FAILURES, API_KEY_FAILURES_PERIOD, MAX_TRACKED_CLIENTS),
            trusted_proxies,
        }
    }

    /// Takes one login or signup attempt for a client address and an email
    /// within `scope`, which is a Deployment ID or "owner". On refusal returns
    /// how long until the next attempt may run.
    ///
    /// Attempts count per client address and per email from that address, so
    /// one client cannot lock others out of an account. The email's shared
    /// failure count is only read here: `auth_failed` adds to it.
    pub(crate) fn auth_attempt(&self, scope: &str, client: IpAddr, email: &str) -> Result<(), Duration> {
        self.auth_attempt_at(scope, client, email, Instant::now())
    }

    fn auth_attempt_at(&self, scope: &str, client: IpAddr, email: &str, now: Instant) -> Result<(), Duration> {
        let client = client_key(client);
        let email = email_key(email);
        self.auth_clients.check_at(&format!("{scope}|{client}"), now)?;
        self.auth_client_emails.check_at(&format!("{scope}|{email}|{client}"), now)?;
        self.auth_email_failures.peek_at(&format!("{scope}|{email}"), now)
    }

    /// Records a wrong password for `email`. It counts toward the email's
    /// shared failure limit only while this client has not used up its share.
    pub(crate) fn auth_failed(&self, scope: &str, client: IpAddr, email: &str) {
        self.auth_failed_at(scope, client, email, Instant::now());
    }

    fn auth_failed_at(&self, scope: &str, client: IpAddr, email: &str, now: Instant) {
        let email = email_key(email);
        let share = format!("{scope}|{email}|{}", client_key(client));
        if self.auth_client_email_failures.check_at(&share, now).is_ok() {
            let _ = self.auth_email_failures.check_at(&format!("{scope}|{email}"), now);
        }
    }

    /// Whether a client may present an API key to the Deployment `scope`.
    /// Only failures use the bucket up, as wrong passwords do: `key_failed`
    /// takes from it, so a client presenting a valid key never waits here.
    pub(crate) fn key_attempt(&self, scope: &str, client: IpAddr) -> Result<(), Duration> {
        self.api_key_failures.peek_at(&format!("{scope}|{}", client_key(client)), Instant::now())
    }

    /// Records an unknown or revoked key from this client.
    pub(crate) fn key_failed(&self, scope: &str, client: IpAddr) {
        let _ = self.api_key_failures.check(&format!("{scope}|{}", client_key(client)));
    }

    /// The address a rate limit should count. X-Forwarded-For is read only
    /// when the socket peer is a configured proxy; then the rightmost entry
    /// that is not itself a configured proxy wins, since entries to its left
    /// come from the client and can be forged.
    pub(crate) fn client_ip(&self, peer: Option<SocketAddr>, headers: &HeaderMap) -> IpAddr {
        let peer = peer
            .map(|address| address.ip().to_canonical())
            .unwrap_or(IpAddr::V6(Ipv6Addr::UNSPECIFIED));
        if !self.trusted_proxies.contains(&peer) {
            return peer;
        }
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect::<Vec<_>>();
        let mut client = peer;
        for entry in forwarded.into_iter().rev() {
            let Ok(address) = entry.parse::<IpAddr>().map(|address| address.to_canonical()) else {
                break;
            };
            client = address;
            if !self.trusted_proxies.contains(&address) {
                break;
            }
        }
        client
    }
}

/// The bucket key for an email address: trimmed, lowercased and capped at
/// 254 bytes.
fn email_key(email: &str) -> String {
    let email = email.trim().to_lowercase();
    let mut end = email.len().min(254);
    while !email.is_char_boundary(end) {
        end -= 1;
    }
    email[..end].to_owned()
}

/// The bucket key for an address. IPv6 clients usually control a whole /64,
/// so they share one bucket per /64.
pub(crate) fn client_key(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// The bucket key for OAuth client registration. Registration is cheaper to
/// repeat than an action call, and many providers hand one customer a whole
/// /48, so IPv6 clients share one bucket per /48 here.
pub(crate) fn registration_key(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
        }
    }
}

/// Parses KONAE_TRUSTED_PROXIES, a comma-separated list of IP addresses.
pub(crate) fn trusted_proxies(value: &str) -> anyhow::Result<Vec<IpAddr>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<IpAddr>()
                .map(|address| address.to_canonical())
                .map_err(|_| anyhow::anyhow!("KONAE_TRUSTED_PROXIES entry {entry} is not an IP address."))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn buckets_refuse_after_capacity_and_refill_over_time() {
        let limiter = RateLimiter::new(30, Duration::from_secs(10), 100);
        let start = Instant::now();
        for _ in 0..30 {
            assert!(limiter.check_at("a", start).is_ok());
        }
        let wait = limiter.check_at("a", start).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(1));
        assert!(limiter.check_at("b", start).is_ok(), "other keys keep their own bucket");
        assert!(limiter.check_at("a", start + Duration::from_millis(350)).is_ok());
        assert!(limiter.check_at("a", start + Duration::from_millis(350)).is_err());
    }

    #[test]
    fn the_limiter_never_tracks_more_than_its_key_limit() {
        let limiter = RateLimiter::new(2, Duration::from_secs(10), 8);
        let start = Instant::now();
        for i in 0..100 {
            let _ = limiter.check_at(&format!("busy-{i}"), start);
            assert!(limiter.len() <= 8);
        }
        // Refilled buckets are dropped before draining ones.
        let later = start + Duration::from_secs(60);
        limiter.check_at("fresh", later).unwrap();
        assert_eq!(limiter.len(), 1);
    }

    #[test]
    fn forwarded_addresses_count_only_behind_a_configured_proxy() {
        let proxy: SocketAddr = "10.0.0.2:5000".parse().unwrap();
        let direct: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 198.51.100.7"));

        let untrusting = Guards::new(Vec::new());
        assert_eq!(untrusting.client_ip(Some(proxy), &headers), proxy.ip());

        let guards = Guards::new(trusted_proxies("10.0.0.2, 10.0.0.3").unwrap());
        assert_eq!(guards.client_ip(Some(direct), &headers), direct.ip());
        assert_eq!(guards.client_ip(Some(proxy), &headers), "198.51.100.7".parse::<IpAddr>().unwrap());
        headers.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 198.51.100.7, 10.0.0.3"));
        assert_eq!(guards.client_ip(Some(proxy), &headers), "198.51.100.7".parse::<IpAddr>().unwrap());
        headers.insert("x-forwarded-for", HeaderValue::from_static("garbage"));
        assert_eq!(guards.client_ip(Some(proxy), &headers), proxy.ip());
        assert_eq!(guards.client_ip(Some(proxy), &HeaderMap::new()), proxy.ip());
        assert!(trusted_proxies("10.0.0.0/8").is_err());
        assert_eq!(client_key("2001:db8:1:2:aaaa::1".parse().unwrap()), client_key("2001:db8:1:2:bbbb::9".parse().unwrap()));
    }

    #[test]
    fn registrations_group_ipv6_clients_by_48() {
        let key = |text: &str| registration_key(text.parse().unwrap());
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:ffff::9"));
        assert_ne!(key("2001:db8:1::1"), key("2001:db8:2::1"));
        assert_eq!(key("192.0.2.4"), "192.0.2.4");
    }

    #[test]
    fn auth_attempts_are_limited_per_client_and_per_email_from_that_client() {
        let guards = Guards::new(Vec::new());
        let client: IpAddr = "192.0.2.1".parse().unwrap();
        for i in 0..AUTH_CLIENT_ATTEMPTS {
            assert!(guards.auth_attempt("app", client, &format!("user{i}@example.test")).is_ok());
        }
        let wait = guards.auth_attempt("app", client, "other@example.test").unwrap_err();
        assert!(wait > Duration::ZERO && wait <= AUTH_ATTEMPTS_PERIOD);
        assert!(guards.auth_attempt("other-app", client, "user0@example.test").is_ok(), "each app has its own buckets");

        let other: IpAddr = "198.51.100.1".parse().unwrap();
        for _ in 0..AUTH_EMAIL_ATTEMPTS {
            assert!(guards.auth_attempt("app", other, "Victim@Example.test").is_ok());
        }
        assert!(guards.auth_attempt("app", other, " victim@example.test").is_err(), "one email from one client shares a bucket");
        let fresh: IpAddr = "203.0.113.1".parse().unwrap();
        assert!(guards.auth_attempt("app", fresh, "victim@example.test").is_ok(), "another client keeps its own bucket");
    }

    #[test]
    fn one_client_failing_cannot_lock_the_owner_out_from_another_address() {
        let guards = Guards::new(Vec::new());
        let attacker: IpAddr = "198.51.100.66".parse().unwrap();
        let victim: IpAddr = "203.0.113.7".parse().unwrap();
        let email = "victim@example.test";
        let start = Instant::now();
        // The attacker fails at its full allowed rate for two hours.
        let mut refused = 0;
        for second in 0..2 * 60 * 60 {
            let now = start + Duration::from_secs(second);
            match guards.auth_attempt_at("owner", attacker, email, now) {
                Ok(()) => guards.auth_failed_at("owner", attacker, email, now),
                Err(_) => refused += 1,
            }
        }
        assert!(refused > 0, "the attacker is throttled");
        let later = start + Duration::from_secs(2 * 60 * 60);
        for _ in 0..AUTH_EMAIL_ATTEMPTS {
            assert!(guards.auth_attempt_at("owner", victim, email, later).is_ok(), "the victim signs in from its own address");
        }
    }

    #[test]
    fn failures_from_many_addresses_pause_logins_for_that_email() {
        let guards = Guards::new(Vec::new());
        let email = "victim@example.test";
        let now = Instant::now();
        let mut addresses = (1..=255).map(|i| format!("198.51.100.{i}").parse::<IpAddr>().unwrap());
        let mut failures = 0;
        while failures < AUTH_EMAIL_FAILURES {
            let address = addresses.next().unwrap();
            for _ in 0..AUTH_EMAIL_ATTEMPTS {
                guards.auth_attempt_at("app", address, email, now).unwrap();
                guards.auth_failed_at("app", address, email, now);
                failures += 1;
            }
        }
        let fresh: IpAddr = "203.0.113.9".parse().unwrap();
        let wait = guards.auth_attempt_at("app", fresh, email, now).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= AUTH_FAILURES_PERIOD);
        assert!(guards.auth_attempt_at("app", fresh, "someone@example.test", now).is_ok(), "other emails are unaffected");
        assert!(guards.auth_attempt_at("owner", fresh, email, now).is_ok(), "each scope counts separately");
        let later = now + AUTH_FAILURES_PERIOD / AUTH_EMAIL_FAILURES + Duration::from_millis(10);
        assert!(guards.auth_attempt_at("app", fresh, email, later).is_ok(), "the count refills over time");
    }

    #[test]
    fn successful_logins_never_use_up_the_shared_failure_count() {
        let guards = Guards::new(Vec::new());
        let start = Instant::now();
        for i in 0..1000 {
            let address: IpAddr = format!("10.0.{}.{}", i / 250, i % 250 + 1).parse().unwrap();
            guards.auth_attempt_at("app", address, "busy@example.test", start).unwrap();
        }
    }

    #[test]
    fn failed_api_keys_use_up_the_client_bucket_before_any_lookup() {
        let guards = Guards::new(Vec::new());
        let client: IpAddr = "198.51.100.7".parse().unwrap();
        for _ in 0..API_KEY_FAILURES {
            assert!(guards.key_attempt("app", client).is_ok());
            guards.key_failed("app", client);
        }
        let wait = guards.key_attempt("app", client).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= API_KEY_FAILURES_PERIOD);
        assert!(guards.key_attempt("app", "198.51.100.8".parse().unwrap()).is_ok(), "another client keeps its own bucket");
        assert!(guards.key_attempt("other-app", client).is_ok(), "each Deployment counts separately");
        let busy: IpAddr = "198.51.100.9".parse().unwrap();
        for _ in 0..1000 {
            assert!(guards.key_attempt("app", busy).is_ok(), "presenting a valid key takes nothing");
        }
    }
}
