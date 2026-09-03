//! The publish-notify port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 7.1.
//!
//! `BlogPublishNotifier` is the parent-blog notification the ONE
//! publish coupling event fires (on the `false -> true` transition
//! only — D6: Odoo's re-notify on redundant writes is not ported, and
//! the BL-R5 reply-downgrade is consciously dropped: it protects a
//! follower inbox that does not exist in backbone).
//!
//! NON-BLOCKING POSTURE (the website intake-notify family, chosen
//! explicitly — contrast with events' template port, which blocks):
//! the publish verb COMMITS regardless; a refused notify WARNs and
//! audits `notify_parked`, and is never auto-retried (the audit trail
//! is the retry surface for the officer). A down mail transport must
//! not unpublish a post.
//!
//! The host installs the real adapter (over backbone-mail, host-side
//! — the module carries no mail crate edge). An unwired host gets
//! [`RefusingPublishNotifier`] — loud, never a silent skip.

use uuid::Uuid;

use super::blog_error::{BlogError, BlogResult};

/// The parent-blog publish notification port.
#[async_trait::async_trait]
pub trait BlogPublishNotifier: Send + Sync {
    /// Fired exactly once per `false -> true` publish transition,
    /// inside the publish transaction (the verb treats a refusal as a
    /// park, never a rollback).
    async fn post_published(
        &self,
        blog_id: Uuid,
        post_id: Uuid,
        title: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<()>;
}

/// The refusing default: every call answers a typed Internal refusal
/// the verb maps onto the `notify_parked` audit row (publish still
/// commits).
pub struct RefusingPublishNotifier;

impl RefusingPublishNotifier {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RefusingPublishNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BlogPublishNotifier for RefusingPublishNotifier {
    async fn post_published(
        &self,
        _blog_id: Uuid,
        _post_id: Uuid,
        _title: &str,
        _actor: Option<Uuid>,
    ) -> BlogResult<()> {
        Err(BlogError::Internal(
            "publish notifier not installed (host adapter missing)".to_string(),
        ))
    }
}

/// A recording notifier for tests/probes: captures every call.
pub struct RecordingNotifier(std::sync::Mutex<Vec<(Uuid, Uuid, String, Option<Uuid>)>>);

impl RecordingNotifier {
    pub fn new() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }

    /// The recorded calls, in order.
    pub fn calls(&self) -> Vec<(Uuid, Uuid, String, Option<Uuid>)> {
        self.0.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl Default for RecordingNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BlogPublishNotifier for RecordingNotifier {
    async fn post_published(
        &self,
        blog_id: Uuid,
        post_id: Uuid,
        title: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<()> {
        if let Ok(mut calls) = self.0.lock() {
            calls.push((blog_id, post_id, title.to_string(), actor));
        }
        Ok(())
    }
}
