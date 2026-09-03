//! The blog verb repository (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! RLS LAW: every transactional method begins with
//! `pool.begin()` + `company_scope::bind_current_company` — first
//! statements, every method. Direct-pool statements (the admin list)
//! go through the `*_scoped` helpers. Services hold no raw sqlx.
//!
//! The audit writer ([`record_audit`]) lives here and is shared by
//! every other repository in the module: one INSERT into
//! `blog.blog_audit_log`, always inside the caller's transaction (a
//! refused write still commits its refusal as a durable fact).

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::application::service::blog_error::{map_unique_violation, BlogError, BlogResult};

/// A blog row as the verbs read/write it.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct BlogRow {
    pub id: Uuid,
    pub company_id: Uuid,
    pub website_id: Uuid,
    pub name: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Create-input (the website binds the site scope; posts derive it).
#[derive(Debug, Clone)]
pub struct CreateBlogInput {
    pub company_id: Uuid,
    pub website_id: Uuid,
    pub name: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
}

/// The typed PATCH whitelist — `website_id` is structurally absent
/// (service-refused while posts exist; the H1b trigger is the wall).
#[derive(Debug, Clone, Default)]
pub struct PatchBlogInput {
    pub name: Option<String>,
    pub subtitle: Option<String>,
    pub description: Option<String>,
}

/// The audit writer: one append-only row inside the caller's
/// transaction. `event` is a closed-vocabulary literal (the DB enum
/// rejects anything else — a typo is a loud runtime failure the
/// probes catch, never a silent drop).
pub async fn record_audit(
    tx: &mut sqlx::PgConnection,
    company: Uuid,
    event: &str,
    actor: Option<Uuid>,
    subject_type: &str,
    subject_id: Option<Uuid>,
    detail: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO blog.blog_audit_log
             (id, company_id, event, actor, subject_type, subject_id, detail, occurred_at)
         VALUES (gen_random_uuid(), $1, $2::blog_audit_event, $3, $4, $5, $6, now())",
    )
    .bind(company)
    .bind(event)
    .bind(actor)
    .bind(subject_type)
    .bind(subject_id)
    .bind(detail)
    .execute(tx)
    .await?;
    Ok(())
}

const BLOG_COLUMNS: &str = "id, company_id, website_id, name, subtitle, description, archived_at";

#[derive(Clone)]
pub struct BlogCommandRepository {
    pool: PgPool,
}

impl BlogCommandRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a blog + its audit row, one transaction.
    pub async fn create(
        &self,
        input: &CreateBlogInput,
        actor: Option<Uuid>,
    ) -> BlogResult<BlogRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: BlogRow = match sqlx::query_as::<_, BlogRow>(&format!(
            "INSERT INTO blog.blogs (id, company_id, website_id, name, subtitle, description)
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5)
             RETURNING {BLOG_COLUMNS}"
        ))
        .bind(input.company_id)
        .bind(input.website_id)
        .bind(&input.name)
        .bind(&input.subtitle)
        .bind(&input.description)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        record_audit(
            &mut tx,
            row.company_id,
            "blog_created",
            actor,
            "blog",
            Some(row.id),
            json!({ "name": row.name, "website_id": row.website_id }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// One blog by id (live or archived — the admin tree sees all
    /// rows; soft-deleted rows are misses).
    pub async fn get(&self, id: Uuid) -> BlogResult<BlogRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row = sqlx::query_as::<_, BlogRow>(&format!(
            "SELECT {BLOG_COLUMNS} FROM blog.blogs
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        tx.commit().await?;
        Ok(row)
    }

    /// The admin list (optionally website-scoped), all states.
    pub async fn list(&self, website_id: Option<Uuid>, limit: i64) -> BlogResult<Vec<BlogRow>> {
        let rows = backbone_orm::company_scope::fetch_all_scoped(
            &self.pool,
            sqlx::query_as::<_, BlogRow>(&format!(
                "SELECT {BLOG_COLUMNS} FROM blog.blogs
                  WHERE metadata->>'deleted_at' IS NULL
                    AND ($1::uuid IS NULL OR website_id = $1)
                  ORDER BY name
                  LIMIT $2"
            ))
            .bind(website_id)
            .bind(limit),
        )
        .await?;
        Ok(rows)
    }

    /// True when any post (archived or not) exists on the blog — the
    /// service-level website-move refusal's check (the H1b trigger is
    /// the wall; this makes the refusal a typed 422, not a 500).
    pub async fn has_posts(&self, id: Uuid) -> BlogResult<bool> {
        let count = backbone_orm::company_scope::fetch_optional_scalar_scoped(
            &self.pool,
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM blog.posts WHERE blog_id = $1")
                .bind(id),
        )
        .await?
        .unwrap_or(0);
        Ok(count > 0)
    }

    /// Move an EMPTY blog to another website (the guarded arm: the
    /// WHERE refuses the move outright when any post exists, so the
    /// H1b trigger never even needs to fire; a loaded blog is the
    /// typed 422 at the service).
    pub async fn move_website(
        &self,
        id: Uuid,
        website_id: Uuid,
        actor: Option<Uuid>,
    ) -> BlogResult<BlogRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: BlogRow = sqlx::query_as::<_, BlogRow>(&format!(
            "UPDATE blog.blogs SET website_id = $2
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
                 AND NOT EXISTS (SELECT 1 FROM blog.posts WHERE blog_id = $1)
               RETURNING {BLOG_COLUMNS}"
        ))
        .bind(id)
        .bind(website_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::BlogHasPosts)?;
        record_audit(
            &mut tx,
            row.company_id,
            "blog_updated",
            actor,
            "blog",
            Some(row.id),
            json!({ "website_moved_to": website_id }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// The typed patch (whitelist only — `website_id` has no arm
    /// here, structurally) + audit row.
    pub async fn patch(
        &self,
        id: Uuid,
        patch: &PatchBlogInput,
        actor: Option<Uuid>,
    ) -> BlogResult<BlogRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: BlogRow = sqlx::query_as::<_, BlogRow>(&format!(
            "UPDATE blog.blogs SET
                 name = COALESCE($2, name),
                 subtitle = COALESCE($3, subtitle),
                 description = COALESCE($4, description)
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
               RETURNING {BLOG_COLUMNS}"
        ))
        .bind(id)
        .bind(&patch.name)
        .bind(&patch.subtitle)
        .bind(&patch.description)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        record_audit(
            &mut tx,
            row.company_id,
            "blog_updated",
            actor,
            "blog",
            Some(row.id),
            json!({ "fields": patch_fields(patch) }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// The archive verb: ONE transaction, ONE single-statement marker
    /// cascade over the posts, then the blog row. Returns the row and
    /// the cascade count (audited).
    pub async fn archive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<(BlogRow, i64)> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        // The single-statement cascade (D5): posts the cascade
        // archives carry the marker so unarchive restores exactly
        // these rows.
        let cascaded = sqlx::query(
            "UPDATE blog.posts
                SET archived_at = now(),
                    archived_by_blog_id = $1,
                    is_published = false
              WHERE blog_id = $1 AND archived_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
        let row: BlogRow = sqlx::query_as::<_, BlogRow>(&format!(
            "UPDATE blog.blogs SET archived_at = now()
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL
              RETURNING {BLOG_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        record_audit(
            &mut tx,
            row.company_id,
            "blog_archived",
            actor,
            "blog",
            Some(row.id),
            json!({ "posts_cascade": cascaded }),
        )
        .await?;
        tx.commit().await?;
        Ok((row, cascaded))
    }

    /// The unarchive verb: restore the blog row and EXACTLY the
    // marker rows (posts archived individually before or after stay
    /// archived). Nothing re-publishes.
    pub async fn unarchive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<(BlogRow, i64)> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let restored = sqlx::query(
            "UPDATE blog.posts
                SET archived_at = NULL, archived_by_blog_id = NULL
              WHERE archived_by_blog_id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
        let row: BlogRow = sqlx::query_as::<_, BlogRow>(&format!(
            "UPDATE blog.blogs SET archived_at = NULL
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL
              RETURNING {BLOG_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        record_audit(
            &mut tx,
            row.company_id,
            "blog_unarchived",
            actor,
            "blog",
            Some(row.id),
            json!({ "posts_restored": restored, "republished": false }),
        )
        .await?;
        tx.commit().await?;
        Ok((row, restored))
    }

    /// The guarded empty-blog delete. Any post (archived or not)
    /// trips the RESTRICT FK — surfaced as the typed 409 and audited
    /// as `blog_delete_refused` (a refused write is a durable fact).
    pub async fn delete(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<()> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let company = match sqlx::query_scalar::<_, Uuid>(
            "SELECT company_id FROM blog.blogs
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        {
            Some(company) => company,
            None => return Err(BlogError::NotFound),
        };
        match sqlx::query("DELETE FROM blog.blogs WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                let mapped = map_unique_violation(e);
                // Roll back the failed delete, then commit ONLY the
                // refusal fact in its own transaction.
                tx.rollback().await?;
                let mut audit_tx = self.pool.begin().await?;
                company_scope::bind_current_company(&mut audit_tx).await?;
                record_audit(
                    &mut audit_tx,
                    company,
                    "blog_delete_refused",
                    actor,
                    "blog",
                    Some(id),
                    json!({ "reason": "posts exist" }),
                )
                .await?;
                audit_tx.commit().await?;
                return Err(mapped);
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

fn patch_fields(patch: &PatchBlogInput) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if patch.name.is_some() {
        fields.push("name");
    }
    if patch.subtitle.is_some() {
        fields.push("subtitle");
    }
    if patch.description.is_some() {
        fields.push("description");
    }
    fields
}
