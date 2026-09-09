//! The visit verb (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 4.5.
//!
//! Order of arms, all before the ONE transaction:
//!
//! 1. Host binding: `WebsiteSurface::resolve_website_by_host` — no
//!    fallback; a miss is the typed `blog_website_not_found`.
//! 2. Token arm: presented token → verify (Tier A, capability.rs);
//!    absent → MINT one and return it (first view counts — the
//!    response carries the token so the client stores it). Bad or
//!    expired signature → the uniform `blog_invalid_input` refusal
//!    (constant-time, one message) + audit `capability_refused`.
//!    Empty secret → the typed 503, fail-closed — never mint under an
//!    empty key.
//! 3. Throttle: fixed windows, per-IP AND per-token (60/hour each).
//!    Over budget → `blog_throttled` + `Retry-After` + audit
//!    `visit_throttled`.
//! 4. The verb itself lives in `VisitCommandRepository::visit` (the
//!    ONE owner of that transaction).
//!
//! No visitor rows anywhere (D4): the token is a random nonce, the
//! receipts are the dedup wall, the counter is the trace.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use uuid::Uuid;

use backbone_website::exports::WebsiteSurface;

use crate::infrastructure::persistence::visit_command_repository::{
    VisitCommandRepository, VisitOutcome,
};

use super::blog_error::{BlogError, BlogResult};
use super::capability::{mint_view_token, verify_view_token, BLOG_CAPABILITY_SECRET_ENV};

/// Visit windows: 60 per hour per IP, 60 per hour per token.
pub const VISIT_THROTTLE: (u64, u64) = (60, 3600);

/// The visit request body (absent token = mint).
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisitRequest {
    #[serde(default)]
    pub view_token: Option<String>,
}

/// The visit response: the (possibly freshly minted) token rides back
/// so the client stores it; `counted: false` = the receipt wall ate a
/// duplicate.
#[derive(Debug, serde::Serialize)]
pub struct VisitResponse {
    pub post_id: Uuid,
    pub counted: bool,
    pub visits: i32,
    pub view_token: String,
}

/// In-memory fixed-window throttle (per key). Windows are wall-clock
/// buckets measured from the first hit inside the window; a poisoned
/// lock fails closed into the live map (never wedges the verb).
#[derive(Debug, Default)]
pub struct FixedWindows {
    inner: Mutex<HashMap<String, (u64, u64)>>, // key -> (window_start_unix, count)
}

impl FixedWindows {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a hit; returns false when the key is over budget in the
    /// current window.
    pub fn allow(&self, key: &str, max: u64, window_secs: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let Ok(mut guard) = self.inner.lock() else {
            // Fail closed: an un-lockable map refuses the hit.
            return false;
        };
        match guard.get_mut(key) {
            Some((start, count)) => {
                if now.saturating_sub(*start) >= window_secs {
                    // Stale window: reset in place.
                    *start = now;
                    *count = 0;
                }
            }
            None => {
                guard.insert(key.to_string(), (now, 0));
            }
        }
        let Some((_, count)) = guard.get_mut(key) else {
            return false;
        };
        if *count >= max {
            return false;
        }
        *count += 1;
        true
    }
}

/// The visit verb service.
pub struct VisitService {
    visits: VisitCommandRepository,
    surface: Arc<dyn WebsiteSurface>,
    windows: FixedWindows,
    secret: String,
}

impl VisitService {
    pub fn new(pool: sqlx::PgPool, surface: Arc<dyn WebsiteSurface>, secret: String) -> Self {
        Self {
            visits: VisitCommandRepository::new(pool),
            surface,
            windows: FixedWindows::new(),
            secret,
        }
    }

    /// The env-var name (operator surface; the secret itself is never
    /// exposed — only whether it is set).
    pub fn secret_env_name(&self) -> &'static str {
        BLOG_CAPABILITY_SECRET_ENV
    }

    pub fn secret_is_configured(&self) -> bool {
        !self.secret.is_empty()
    }

    /// The whole visit flow (see the module doc).
    pub async fn visit(
        &self,
        host: &str,
        post_slug: &str,
        presented: Option<&str>,
        client_ip: &str,
    ) -> BlogResult<VisitResponse> {
        // 1. Host binding — no fallback.
        let website = self
            .surface
            .resolve_website_by_host(host)
            .await
            .map_err(|_| BlogError::WebsiteNotFound)?;

        // 2. Token arm. Fail-closed secret first (never mint under an
        //    empty key; a presented token cannot verify either).
        if !self.secret_is_configured() {
            return Err(BlogError::CapabilitySecretNotConfigured);
        }
        let now = chrono::Utc::now();
        let token = match presented {
            Some(presented) => match verify_view_token(&self.secret, presented, now) {
                // Verified: the nonce IS the dedup identity.
                Ok(_nonce) => presented.to_string(),
                Err(_) => {
                    // Uniform refusal (constant-time verify, one
                    // message — no oracle on WHICH arm failed).
                    self.visits
                        .audit_refusal(
                            "capability_refused",
                            json!({ "post_slug": post_slug }),
                        )
                        .await?;
                    return Err(BlogError::InvalidInput("invalid view token".to_string()));
                }
            },
            None => mint_view_token(&self.secret, now)?,
        };

        // 3. Throttle: per-IP AND per-token.
        let (max, window) = VISIT_THROTTLE;
        if !self
            .windows
            .allow(&format!("visit-ip:{client_ip}"), max, window)
            || !self
                .windows
                .allow(&format!("visit-token:{token}"), max, window)
        {
            self.visits
                .audit_refusal(
                    "visit_throttled",
                    json!({ "post_slug": post_slug, "client_ip": client_ip }),
                )
                .await?;
            return Err(BlogError::Throttled {
                retry_after_secs: window as u32,
            });
        }

        // 4. The verb (the ONE transaction).
        let VisitOutcome {
            post_id,
            counted,
            visits,
        } = self
            .visits
            .visit(website.id, post_slug, &token)
            .await?;
        Ok(VisitResponse {
            post_id,
            counted,
            visits,
            view_token: token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_allows_up_to_cap_then_refuses() {
        let windows = FixedWindows::new();
        for n in 0..VISIT_THROTTLE.0 {
            assert!(
                windows.allow("visit-ip:x", VISIT_THROTTLE.0, VISIT_THROTTLE.1),
                "hit {n} inside the cap must pass"
            );
        }
        assert!(
            !windows.allow("visit-ip:x", VISIT_THROTTLE.0, VISIT_THROTTLE.1),
            "one past the cap must refuse"
        );
    }

    #[test]
    fn keys_are_independent_buckets() {
        let windows = FixedWindows::new();
        for _ in 0..3 {
            assert!(windows.allow("visit-ip:a", 3, VISIT_THROTTLE.1));
        }
        assert!(!windows.allow("visit-ip:a", 3, VISIT_THROTTLE.1));
        // A different key (the per-token bucket beside the per-IP one)
        // has its OWN budget.
        assert!(windows.allow("visit-token:b", 3, VISIT_THROTTLE.1));
    }

    #[test]
    fn zero_window_buckets_reset_every_hit() {
        // The stale-window reset arm: window_secs = 0 makes every hit
        // land in a fresh bucket, so no cap ever trips.
        let windows = FixedWindows::new();
        for _ in 0..10 {
            assert!(windows.allow("visit-ip:z", 1, 0));
        }
    }
}
