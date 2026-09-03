//! The post verb repository (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! RLS LAW: every transactional method begins with
//! `pool.begin()` + `company_scope::bind_current_company`. The ONE
//! publish coupling runs on a CALLER-OWNED transaction
//! ([`PostCommandRepository::publish_flip`]) so the service can fire
//! the notifier port and park its refusal INSIDE the same
//! transaction (SPEC section 4.1: the audit row IS the event; a
//! parked notify is an audit row, never a rollback).
//!
//! The patch input struct carries NO `is_published`/`published_date`
//! arms — the fence is structural: no generic patch path can write
//! the pair even if a future caller forgets the check.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::application::service::blog_error::{map_unique_violation, BlogError, BlogResult};

use super::blog_command_repository::record_audit;

const POST_COLUMNS: &str = "id, company_id, blog_id, website_id, title, slug, content, teaser, \
     cover, author_officer, author_name, is_published, published_date, post_date, visits, \
     allow_comments, archived_at, archived_by_blog_id";

/// A post row as the verbs read/write it.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct PostRow {
    pub id: Uuid,
    pub company_id: Uuid,
    pub blog_id: Uuid,
    pub website_id: Uuid,
    pub title: String,
    pub slug: String,
    pub content: Option<String>,
    pub teaser: Option<String>,
    pub cover: Option<serde_json::Value>,
    pub author_officer: Option<Uuid>,
    pub author_name: Option<String>,
    pub is_published: bool,
    pub published_date: Option<chrono::DateTime<chrono::Utc>>,
    pub post_date: chrono::DateTime<chrono::Utc>,
    pub visits: i32,
    pub allow_comments: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub archived_by_blog_id: Option<Uuid>,
}

/// Create-input. `website_id` is NOT taken from the caller: it is
/// derived from the parent blog inside the transaction (the owned
/// column; the H1a constraint trigger is the wall).
#[derive(Debug, Clone)]
pub struct CreatePostInput {
    pub company_id: Uuid,
    pub blog_id: Uuid,
    pub title: String,
    pub slug: String,
    pub content: Option<String>,
    pub teaser: Option<String>,
    pub cover: Option<serde_json::Value>,
    pub author_officer: Option<Uuid>,
    pub author_name: Option<String>,
    pub post_date: chrono::DateTime<chrono::Utc>,
    pub allow_comments: bool,
}

/// The typed PATCH whitelist — the fence pair is structurally absent
/// (publish/unpublish are the only writers), and `blog_id`/
/// `website_id`/`visits` are refused fields with no arm here either.
#[derive(Debug, Clone, Default)]
pub struct PatchPostInput {
    pub title: Option<String>,
    pub slug: Option<String>,
    pub content: Option<String>,
    pub teaser: Option<String>,
    pub cover: Option<serde_json::Value>,
    pub post_date: Option<chrono::DateTime<chrono::Utc>>,
    pub allow_comments: Option<bool>,
    pub author_officer: Option<Uuid>,
    pub author_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub row: PostRow,
    /// false = the guarded flip found the post already published —
    /// no stamp, no audit re-emit, no notify (D6).
    pub changed: bool,
}

#[derive(Clone)]
pub struct PostCommandRepository {
    pool: PgPool,
}

impl PostCommandRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Open a transaction for the caller-owned publish coupling flow
    /// (the service binds the company scope, drives
    /// [`Self::publish_flip`], fires the notifier port, and commits).
    pub async fn begin(&self) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
        self.pool.begin().await
    }

    /// Create a post: the website scope is derived from the parent
    /// blog (one statement family inside the create transaction — the
    /// H1a trigger is the wall, the derivation is the belt).
    pub async fn create(
        &self,
        input: &CreatePostInput,
        actor: Option<Uuid>,
    ) -> BlogResult<PostRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let website_id: Uuid = sqlx::query_scalar(
            "SELECT website_id FROM blog.blogs
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(input.blog_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        let row: PostRow = match sqlx::query_as::<_, PostRow>(&format!(
            "INSERT INTO blog.posts
                 (id, company_id, blog_id, website_id, title, slug, content, teaser, cover,
                  author_officer, author_name, post_date, allow_comments)
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             RETURNING {POST_COLUMNS}"
        ))
        .bind(input.company_id)
        .bind(input.blog_id)
        .bind(website_id)
        .bind(&input.title)
        .bind(&input.slug)
        .bind(&input.content)
        .bind(&input.teaser)
        .bind(&input.cover)
        .bind(input.author_officer)
        .bind(&input.author_name)
        .bind(input.post_date)
        .bind(input.allow_comments)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        record_audit(
            &mut tx,
            row.company_id,
            "post_created",
            actor,
            "post",
            Some(row.id),
            json!({ "blog_id": row.blog_id, "slug": row.slug }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// One post by id (admin read; all states).
    pub async fn get(&self, id: Uuid) -> BlogResult<PostRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row = sqlx::query_as::<_, PostRow>(&format!(
            "SELECT {POST_COLUMNS} FROM blog.posts
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        tx.commit().await?;
        Ok(row)
    }

    /// The typed patch: whitelist-only SET arms, no fence pair
    /// (structural). Returns the row and whether the slug changed
    /// (the service records the redirect).
    pub async fn patch(
        &self,
        id: Uuid,
        patch: &PatchPostInput,
        actor: Option<Uuid>,
    ) -> BlogResult<(PostRow, Option<String>)> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let before_slug: Option<String> = sqlx::query_scalar(
            "SELECT slug FROM blog.posts WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        let row: PostRow = match sqlx::query_as::<_, PostRow>(&format!(
            "UPDATE blog.posts SET
                 title = COALESCE($2, title),
                 slug = COALESCE($3, slug),
                 content = COALESCE($4, content),
                 teaser = COALESCE($5, teaser),
                 cover = COALESCE($6, cover),
                 post_date = COALESCE($7, post_date),
                 allow_comments = COALESCE($8, allow_comments),
                 author_officer = COALESCE($9, author_officer),
                 author_name = COALESCE($10, author_name)
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
               RETURNING {POST_COLUMNS}"
        ))
        .bind(id)
        .bind(&patch.title)
        .bind(&patch.slug)
        .bind(&patch.content)
        .bind(&patch.teaser)
        .bind(&patch.cover)
        .bind(patch.post_date)
        .bind(patch.allow_comments)
        .bind(patch.author_officer)
        .bind(&patch.author_name)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        let slug_change = if before_slug.as_deref() != Some(row.slug.as_str()) {
            before_slug.clone()
        } else {
            None
        };
        record_audit(
            &mut tx,
            row.company_id,
            "post_updated",
            actor,
            "post",
            Some(row.id),
            json!({ "slug_was": slug_change, "slug_is": row.slug }),
        )
        .await?;
        tx.commit().await?;
        Ok((row, slug_change))
    }

    /// Step 1+3 of the ONE publish coupling, on the CALLER-OWNED
    /// transaction: the guarded FOR UPDATE flip (already published →
    /// `changed: false`, no stamp, no audit — D6) and the CASE stamp
    /// that PRESERVES a pre-set future `published_date` (the
    /// lazy-schedule arm). The audit row (`post_published` — this row
    /// IS the BlogPostPublished event, D1) is written here too; the
    /// service fires the notifier and parks refusals in the same
    /// transaction, then commits.
    pub async fn publish_flip(
        &self,
        tx: &mut sqlx::PgConnection,
        id: Uuid,
        actor: Option<Uuid>,
    ) -> BlogResult<PublishOutcome> {
        // The guarded flip: lock the row, read the pair.
        let guarded: Option<(bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
            "SELECT is_published, published_date FROM blog.posts
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL
              FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let (already, prior_date) = guarded.ok_or(BlogError::NotFound)?;
        if already {
            // No-op success: no stamp, no audit re-emit, no notify.
            let row = sqlx::query_as::<_, PostRow>(&format!(
                "SELECT {POST_COLUMNS} FROM blog.posts WHERE id = $1"
            ))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            return Ok(PublishOutcome {
                row,
                changed: false,
            });
        }
        let row: PostRow = match sqlx::query_as::<_, PostRow>(&format!(
            "UPDATE blog.posts SET
                 is_published = true,
                 published_date = CASE
                     WHEN published_date IS NULL OR published_date <= now() THEN now()
                     ELSE published_date
                 END
               WHERE id = $1
               RETURNING {POST_COLUMNS}"
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        let future_survived = prior_date.map(|d| d > chrono::Utc::now()).unwrap_or(false);
        record_audit(
            tx,
            row.company_id,
            "post_published",
            actor,
            "post",
            Some(row.id),
            json!({
                "published_date": row.published_date,
                "future_survived": future_survived,
                "actor": actor,
            }),
        )
        .await?;
        Ok(PublishOutcome { row, changed: true })
    }

    /// The unpublish fence verb: flip false, RETAIN the stamp as
    /// history. Idempotent no-op when already false (no audit row on
    /// the no-op).
    pub async fn unpublish(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        // Guarded: only a currently-published row transitions (and
        // audits).
        let transitioned: Option<PostRow> = sqlx::query_as::<_, PostRow>(&format!(
            "UPDATE blog.posts SET is_published = false
               WHERE id = $1 AND is_published AND metadata->>'deleted_at' IS NULL
               RETURNING {POST_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let row = match transitioned {
            Some(row) => {
                record_audit(
                    &mut tx,
                    row.company_id,
                    "post_unpublished",
                    actor,
                    "post",
                    Some(row.id),
                    json!({ "published_date_retained": row.published_date }),
                )
                .await?;
                row
            }
            None => sqlx::query_as::<_, PostRow>(&format!(
                "SELECT {POST_COLUMNS} FROM blog.posts
                  WHERE id = $1 AND metadata->>'deleted_at' IS NULL"
            ))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(BlogError::NotFound)?,
        };
        tx.commit().await?;
        Ok(row)
    }

    /// The post archive verb: forced unpublish embedded in the same
    /// transaction (D5 one-way). `detail.forced_unpublish` records
    /// whether the flip happened (the prior state is read under
    /// FOR UPDATE first — the returning row alone cannot tell).
    pub async fn archive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let live: Option<bool> = sqlx::query_scalar(
            "SELECT is_published FROM blog.posts
              WHERE id = $1 AND archived_at IS NULL
                AND metadata->>'deleted_at' IS NULL
              FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let prior_published = match live {
            // Live row: archive it (forced unpublish embedded).
            Some(prior) => prior,
            // Miss or already archived: idempotent — return the
            // current row without a second audit.
            None => {
                return match sqlx::query_as::<_, PostRow>(&format!(
                    "SELECT {POST_COLUMNS} FROM blog.posts
                      WHERE id = $1 AND metadata->>'deleted_at' IS NULL"
                ))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                {
                    Some(row) => {
                        tx.commit().await?;
                        Ok(row)
                    }
                    None => Err(BlogError::NotFound),
                };
            }
        };
        let row: PostRow = sqlx::query_as::<_, PostRow>(&format!(
            "UPDATE blog.posts SET archived_at = now(), is_published = false
               WHERE id = $1
               RETURNING {POST_COLUMNS}"
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        record_audit(
            &mut tx,
            row.company_id,
            "post_archived",
            actor,
            "post",
            Some(row.id),
            json!({ "forced_unpublish": prior_published }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// The post unarchive verb: restore liveness ONLY —
    /// `is_published` stays false (the one-way rule; nothing ever
    /// re-publishes except the publish verb).
    pub async fn unarchive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: PostRow = sqlx::query_as::<_, PostRow>(&format!(
            "UPDATE blog.posts SET archived_at = NULL
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
               RETURNING {POST_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        record_audit(
            &mut tx,
            row.company_id,
            "post_unarchived",
            actor,
            "post",
            Some(row.id),
            json!({ "republished": false }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Record a `publish_refused` audit fact (the route's fence arm —
    /// SPEC section 4.1): a refused PATCH is a durable fact even
    /// though nothing was written. The post's company is read under
    /// the bound scope; an unknown post id records nothing (the typed
    /// 404 answers for itself).
    pub async fn audit_fence_refusal(
        &self,
        id: Uuid,
        fields: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<()> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let company: Option<Uuid> =
            sqlx::query_scalar("SELECT company_id FROM blog.posts WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(company) = company {
            record_audit(
                &mut tx,
                company,
                "publish_refused",
                actor,
                "post",
                Some(id),
                json!({ "fields": fields }),
            )
            .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    /// Replace the post's tag set (the PUT verb): delete + insert in
    /// one statement family, inside one transaction. Tag ids were
    /// already resolved company-scoped by the service (D8: lookup,
    /// never trust).
    pub async fn set_tags(
        &self,
        post_id: Uuid,
        tag_ids: &[Uuid],
        actor: Option<Uuid>,
    ) -> BlogResult<Vec<Uuid>> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let company: Uuid = sqlx::query_scalar(
            "SELECT company_id FROM blog.posts
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(post_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        sqlx::query("DELETE FROM blog.post_tags WHERE post_id = $1")
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
        for tag_id in tag_ids {
            match sqlx::query(
                "INSERT INTO blog.post_tags (id, company_id, post_id, tag_id)
                 VALUES (gen_random_uuid(), $1, $2, $3)",
            )
            .bind(company)
            .bind(post_id)
            .bind(tag_id)
            .execute(&mut *tx)
            .await
            {
                Ok(_) => {}
                Err(e) => return Err(map_unique_violation(e)),
            }
        }
        record_audit(
            &mut tx,
            company,
            "post_updated",
            actor,
            "post",
            Some(post_id),
            json!({ "tags_set": tag_ids.len() }),
        )
        .await?;
        tx.commit().await?;
        Ok(tag_ids.to_vec())
    }
}
