//! The visit verb's transaction (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — the ONE owner of the visit flow's SQL
//! (SPEC section 4.5).
//!
//! ONE transaction, opened at the PUBLIC tier (the ambient org scope
//! relayed onto the transaction + the tier mark — the fence is the
//! visibility guard), in order:
//!
//!  1. Resolve the post by slug + website under the public fence
//!     (a miss is the uniform 404 — and because the whole flow rolls
//!     back, NO receipt row survives the miss: no oracle).
//!  2. `INSERT .. ON CONFLICT DO NOTHING` the receipt: the unique
//!     `(post_id, view_token, window_start)` IS the dedup wall (H5).
//!     Zero rows inserted → duplicate view → `{counted: false}`,
//!     commit, done.
//!  3. Single-statement atomic increment
//!     (`visits = visits + 1` with the visibility guard re-posed in
//!     the WHERE): a self-incrementing row UPDATE serializes at the
//!     row and cannot lose counts — strictly stronger than a SKIP
//!     LOCKED increment, which would undercount under contention.
//!  4. Amortized prune: a bounded DELETE gated to ~1% of visits by a
//!     random predicate — receipts are retained 30 days WITHOUT any
//!     scheduler (D2 holds module-wide).
//!
//! Per-visit audit rows are deliberately absent (D4): the counter +
//! the receipts ARE the trace. Only refusals are audited, and those
//! audit rows are written by the service (throttle/capability), not
//! here.

use sqlx::PgPool;
use uuid::Uuid;
use super::scoped_read;

use crate::application::service::blog_error::{BlogError, BlogResult};
use crate::application::service::site_scope::bind_public_scope;

/// Receipt retention (days) — the prune horizon.
pub const RECEIPT_RETENTION_DAYS: i64 = 30;

/// The prune's fire probability (~1% of visits carry the prune).
pub const PRUNE_GATE: f64 = 0.01;

/// What the visit verb did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VisitOutcome {
    pub post_id: Uuid,
    /// false = the receipt wall said "already counted this window".
    pub counted: bool,
    pub visits: i32,
}

#[derive(Clone)]
pub struct VisitCommandRepository {
    pool: PgPool,
}

impl VisitCommandRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The whole visit flow (see the module doc). `view_token` is the
    /// VERIFIED nonce arm (service-side; capability.rs) — never a
    /// visitor profile.
    pub async fn visit(
        &self,
        website_id: Uuid,
        post_slug: &str,
        view_token: &str,
    ) -> BlogResult<VisitOutcome> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx).await?;

        // 1. Resolve under the public fence (scoped lookup, D8; the
        //    uniform miss rolls the whole flow back — no receipt
        //    oracle).
        let post: Option<(Uuid, i32)> = sqlx::query_as(
            "SELECT id, visits FROM blog.posts
              WHERE slug = $1
                AND website_id = $2
                AND is_published
                AND post_date <= now()
                AND archived_at IS NULL
                AND metadata->>'deleted_at' IS NULL",
        )
        .bind(post_slug)
        .bind(website_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (post_id, _) = post.ok_or(BlogError::NotFound)?;

        // 2. The dedup wall.
        let inserted = sqlx::query(
            "INSERT INTO blog.post_view_receipts
                 (id, post_id, view_token, window_start, occurred_at)
             VALUES (gen_random_uuid(), $1, $2, date_trunc('day', now()), now())
             ON CONFLICT (post_id, view_token, window_start) DO NOTHING",
        )
        .bind(post_id)
        .bind(view_token)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if inserted == 0 {
            // Duplicate within the window: commit the read, answer
            // counted=false. No increment, no audit.
            let visits =
                sqlx::query_scalar::<_, i32>("SELECT visits FROM blog.posts WHERE id = $1")
                    .bind(post_id)
                    .fetch_one(&mut *tx)
                    .await?;
            tx.commit().await?;
            return Ok(VisitOutcome {
                post_id,
                counted: false,
                visits,
            });
        }

        // 3. Atomic self-increment, visibility guard re-posed. Zero
        //    rows = the post left the visible set between resolve and
        //    increment (or the fence refused) → uniform 404, full
        //    rollback (the receipt dies with the miss).
        let incremented = sqlx::query(
            "UPDATE blog.posts SET visits = visits + 1
              WHERE id = $1
                AND is_published
                AND post_date <= now()
                AND archived_at IS NULL
                AND metadata->>'deleted_at' IS NULL",
        )
        .bind(post_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if incremented == 0 {
            tx.rollback().await?;
            return Err(BlogError::NotFound);
        }
        let visits = sqlx::query_scalar::<_, i32>("SELECT visits FROM blog.posts WHERE id = $1")
            .bind(post_id)
            .fetch_one(&mut *tx)
            .await?;

        // 4. Amortized prune (~1% of counted visits carry it).
        if rand::random::<f64>() < PRUNE_GATE {
            sqlx::query(
                "DELETE FROM blog.post_view_receipts
                  WHERE window_start < now() - ($1 || ' days')::interval",
            )
            .bind(RECEIPT_RETENTION_DAYS.to_string())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(VisitOutcome {
            post_id,
            counted: true,
            visits,
        })
    }

    /// The receipts-per-day census (probe support; not a route).
    pub async fn receipt_count(&self, post_id: Uuid) -> BlogResult<i64> {
        let count = scoped_read::fetch_optional_scalar(
            &self.pool,
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM blog.post_view_receipts WHERE post_id = $1",
            )
            .bind(post_id),
        )
        .await?
        .unwrap_or(0);
        Ok(count)
    }

    /// Record a visit-flow refusal (`visit_throttled` /
    /// `capability_refused`) on its own transaction at the public
    /// tier. Refusals are the ONLY audited visit facts (D4); the
    /// service calls this, the pool stays here.
    pub async fn audit_refusal(
        &self,
        event: &str,
        detail: serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx).await?;
        super::blog_command_repository::record_audit(&mut tx, event, None, "post", None, detail)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
