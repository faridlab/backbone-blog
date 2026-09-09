//! The public-tier scope binder (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 2.3.
//!
//! The public path binds the tier mark transaction-locally before any
//! statement:
//!
//! ```sql
//! set_config('app.blog_tier', 'public', true)
//! ```
//!
//! which arms the RESTRICTIVE visibility policies (hardening H2).
//! Everything else (admin verbs, the host request scope) NEVER sets
//! `app.blog_tier`, so `COALESCE(current_setting(...), '') <> 'public'`
//! is true and the restrictive policy is inert — officers read all
//! rows, including unpublished, future-dated, and archived (the
//! declared "employees see everything" posture).
//!
//! Tenancy (ADR-0029): the module ships no scoping column and binds no
//! tenant key of its own. When the composing service resolved an org
//! request scope, it is relayed onto the transaction (`bind_org_scope_on`)
//! so the decorator-installed fence applies to the statements that
//! follow; with no scope bound the statements run plainly. The bind is
//! transaction-local (`true` flag): no variable can leak past commit on
//! a pooled connection — the fenced-runtime posture the probes claim.

/// The tier value the restrictive policies key on.
pub const PUBLIC_TIER: &str = "public";

/// Bind the public scope on an open transaction: the ambient org scope
/// (when the composing service resolved one) + the public tier. Call as
/// the FIRST statements after `begin()` on every public read/write path
/// (the RLS LAW's public arm).
pub async fn bind_public_scope(tx: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    if let Some(scope) = backbone_orm::org_scope::current_org_scope() {
        backbone_orm::org_scope::bind_org_scope_on(&mut *tx, &scope).await?;
    }
    sqlx::query("SELECT set_config('app.blog_tier', $1, true)")
        .bind(PUBLIC_TIER)
        .execute(tx)
        .await?;
    Ok(())
}
