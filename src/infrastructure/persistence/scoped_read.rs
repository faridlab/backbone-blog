//! Shared read transport for the module's own tables (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The module ships no tenancy of its own (composition-installed tenancy, ADR-0029): each helper
//! opens a short-lived transaction, binds the AMBIENT org scope onto it when a composing service
//! resolved one — so the decorator-installed fence applies to the read — and runs the caller's
//! statement there; on an undecorated deployment the statement runs plainly. This is the
//! multi-row and scalar read twin of `backbone_orm::org_scope`'s request-connection helpers,
//! which cover executes and single-row reads.

use sqlx::{PgPool, Postgres};

async fn begin_scoped(pool: &PgPool) -> Result<sqlx::Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Some(scope) = backbone_orm::org_scope::current_org_scope() {
        backbone_orm::org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    Ok(tx)
}

/// Multi-row read under the ambient scope: the `query_as(...)` + `fetch_all` pair.
pub async fn fetch_all<'q, O>(
    pool: &PgPool,
    query: sqlx::query::QueryAs<'q, Postgres, O, sqlx::postgres::PgArguments>,
) -> Result<Vec<O>, sqlx::Error>
where
    O: 'q + Send + Unpin + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>,
{
    let mut tx = begin_scoped(pool).await?;
    let rows = query.fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(rows)
}

/// Optional single-row read under the ambient scope: the `query_as(...)` + `fetch_optional` pair.
pub async fn fetch_optional<'q, O>(
    pool: &PgPool,
    query: sqlx::query::QueryAs<'q, Postgres, O, sqlx::postgres::PgArguments>,
) -> Result<Option<O>, sqlx::Error>
where
    O: 'q + Send + Unpin + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>,
{
    let mut tx = begin_scoped(pool).await?;
    let row = query.fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(row)
}

/// Exactly-one-row read under the ambient scope: the `query_as(...)` + `fetch_one` pair.
pub async fn fetch_one<'q, O>(
    pool: &PgPool,
    query: sqlx::query::QueryAs<'q, Postgres, O, sqlx::postgres::PgArguments>,
) -> Result<O, sqlx::Error>
where
    O: 'q + Send + Unpin + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>,
{
    let mut tx = begin_scoped(pool).await?;
    let row = query.fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(row)
}

/// Optional scalar read under the ambient scope: the `query_scalar(...)` + `fetch_optional`
/// pair — `None` maps to the caller's default.
pub async fn fetch_optional_scalar<'q, O>(
    pool: &PgPool,
    query: sqlx::query::QueryScalar<'q, Postgres, O, sqlx::postgres::PgArguments>,
) -> Result<Option<O>, sqlx::Error>
where
    O: 'q + Send + Unpin,
    (O,): for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>,
{
    let mut tx = begin_scoped(pool).await?;
    let row = query.fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(row)
}
