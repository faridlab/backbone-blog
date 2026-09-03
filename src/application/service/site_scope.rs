//! The public-tier scope binder (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 2.3.
//!
//! The public path binds BOTH GUCs transaction-locally before any
//! statement:
//!
//!  1. `bind_company_on(tx, website.company_id)` — the company fence
//!     (the public path has no request scope; the company arrives
//!     from the resolved website, never from the request);
//!  2. `set_config('app.blog_tier', 'public', true)` — arms the
//!     RESTRICTIVE visibility policies (hardening H2).
//!
//! Everything else (admin verbs, the host request scope) NEVER sets
//! `app.blog_tier`, so `COALESCE(current_setting(...), '') <> 'public'`
//! is true and the restrictive policy is inert — officers of the
//! scoped company read all rows, including unpublished, future-dated,
//! and archived (the declared "employees see everything" posture).
//!
//! Both binds are transaction-local (`true` flag): neither GUC can
//! leak past commit on a pooled connection — the fenced-runtime probe
//! claims this (claim 9).

use uuid::Uuid;

/// The tier value the restrictive policies key on.
pub const PUBLIC_TIER: &str = "public";

/// Bind the public scope on an open transaction: company fence +
/// public tier. Call as the FIRST statements after `begin()` on every
/// public read/write path (the RLS LAW's public arm).
pub async fn bind_public_scope(
    tx: &mut sqlx::PgConnection,
    company: Uuid,
) -> Result<(), sqlx::Error> {
    backbone_orm::company_scope::bind_company_on(tx, company).await?;
    sqlx::query("SELECT set_config('app.blog_tier', $1, true)")
        .bind(PUBLIC_TIER)
        .execute(tx)
        .await?;
    Ok(())
}
