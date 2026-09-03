//! The tag verb repository (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) + the module's slug normalizer.
//!
//! RLS LAW: every transactional method begins with `pool.begin()` +
//! `company_scope::bind_current_company`; the direct-pool reads go
//! through the `*_scoped` helpers. Slug resolution is a SCOPED
//! LOOKUP, never URL-derived trust (D8: `slug -> id` under the bound
//! company; unknown slug = the uniform miss).
//!
//! The uniqueness grain is THE COMPANY (D3 / hardening H3): Odoo's
//! global `unique(name)` is the addon's only 2 SQL constraints and is
//! wrong for multi-tenancy — a global wall is a cross-tenant
//! existence oracle and a collision blocker. Within one company the
//! vocabulary is shared across that company's blogs and websites.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::application::service::blog_error::{map_unique_violation, BlogError, BlogResult};

use super::blog_command_repository::record_audit;

const TAG_COLUMNS: &str = "id, company_id, name, slug, category_id";
const CATEGORY_COLUMNS: &str = "id, company_id, name";

/// A tag row.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TagRow {
    pub id: Uuid,
    pub company_id: Uuid,
    pub name: String,
    pub slug: String,
    pub category_id: Option<Uuid>,
}

/// A tag-category row.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TagCategoryRow {
    pub id: Uuid,
    pub company_id: Uuid,
    pub name: String,
}

/// The module's slug normalizer: lowercase, ASCII alphanumerics,
/// runs of anything else collapsed to single `-`, edges trimmed.
/// Kebab shape is enforced AGAIN at the DB (H7 CHECK) — the
/// normalizer makes honest input conform; the wall makes hostile
/// input refuse.
pub fn normalize_slug(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    out
}

#[derive(Clone)]
pub struct TagCommandRepository {
    pool: PgPool,
}

impl TagCommandRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a tag at the company grain: name trimmed by the
    /// service, slug derived here. Collisions surface as the typed
    /// 409s (`uq_tags_company_name` / `uq_tags_company_slug`).
    pub async fn create(
        &self,
        company: Uuid,
        name: &str,
        category_id: Option<Uuid>,
        actor: Option<Uuid>,
    ) -> BlogResult<TagRow> {
        let slug = normalize_slug(name);
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: TagRow = match sqlx::query_as::<_, TagRow>(&format!(
            "INSERT INTO blog.tags (id, company_id, name, slug, category_id)
             VALUES (gen_random_uuid(), $1, $2, $3, $4)
             RETURNING {TAG_COLUMNS}"
        ))
        .bind(company)
        .bind(name)
        .bind(&slug)
        .bind(category_id)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        record_audit(
            &mut tx,
            company,
            "tag_created",
            actor,
            "tag",
            Some(row.id),
            json!({ "name": row.name, "slug": row.slug }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Rename / re-categorize: the slug is RE-DERIVED from the new
    /// name (the service records the redirect through the website
    /// surface after commit).
    pub async fn rename(
        &self,
        id: Uuid,
        name: Option<&str>,
        category_id: Option<Option<Uuid>>,
        actor: Option<Uuid>,
    ) -> BlogResult<(TagRow, Option<String>)> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let before: Option<(String, String)> = sqlx::query_as(
            "SELECT name, slug FROM blog.tags
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let (_, prior_slug) = before.ok_or(BlogError::NotFound)?;
        let derived_slug = name.map(normalize_slug);
        // Two-level Option flattened into an explicit arm: $4 says
        // whether the patch touches the category at all; $5 carries
        // the value (NULL = clear the grouping).
        let touches_category = category_id.is_some();
        let category_value = category_id.flatten();
        let row: TagRow = match sqlx::query_as::<_, TagRow>(&format!(
            "UPDATE blog.tags SET
                 name = COALESCE($2, name),
                 slug = COALESCE($3, slug),
                 category_id = CASE WHEN $4 THEN $5::uuid ELSE category_id END
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
               RETURNING {TAG_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(&derived_slug)
        .bind(touches_category)
        .bind(category_value)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(map_unique_violation(e)),
        };
        let slug_change = if prior_slug != row.slug {
            Some(prior_slug)
        } else {
            None
        };
        record_audit(
            &mut tx,
            row.company_id,
            "tag_updated",
            actor,
            "tag",
            Some(row.id),
            json!({ "slug_was": slug_change, "slug_is": row.slug }),
        )
        .await?;
        tx.commit().await?;
        Ok((row, slug_change))
    }

    /// Delete a tag: untag everywhere in ONE statement, then remove
    /// the row (the FK would refuse the delete with links — the
    /// untag-first order makes the verb total).
    pub async fn delete(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<i64> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let company: Uuid = sqlx::query_scalar(
            "SELECT company_id FROM blog.tags
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        let untagged = sqlx::query("DELETE FROM blog.post_tags WHERE tag_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected() as i64;
        sqlx::query("DELETE FROM blog.tags WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        record_audit(
            &mut tx,
            company,
            "tag_deleted",
            actor,
            "tag",
            Some(id),
            json!({ "untagged": untagged }),
        )
        .await?;
        tx.commit().await?;
        Ok(untagged)
    }

    /// The admin list (the full company vocabulary, all states).
    pub async fn list(&self, limit: i64) -> BlogResult<Vec<TagRow>> {
        let rows = backbone_orm::company_scope::fetch_all_scoped(
            &self.pool,
            sqlx::query_as::<_, TagRow>(&format!(
                "SELECT {TAG_COLUMNS} FROM blog.tags
                  WHERE metadata->>'deleted_at' IS NULL
                  ORDER BY name LIMIT $1"
            ))
            .bind(limit),
        )
        .await?;
        Ok(rows)
    }

    /// Resolve a slug to the tag row under the bound company (D8:
    /// scoped lookup; a miss is the uniform 404).
    pub async fn resolve_by_slug(&self, slug: &str) -> BlogResult<TagRow> {
        let row = backbone_orm::company_scope::fetch_optional_scoped(
            &self.pool,
            sqlx::query_as::<_, TagRow>(&format!(
                "SELECT {TAG_COLUMNS} FROM blog.tags
                  WHERE slug = $1 AND metadata->>'deleted_at' IS NULL"
            ))
            .bind(slug),
        )
        .await?
        .ok_or(BlogError::NotFound)?;
        Ok(row)
    }

    /// Resolve a batch of slugs at once (the listing's tag filter):
    /// returns the misses so the caller can try the redirect seam
    /// before refusing.
    pub async fn resolve_many(&self, slugs: &[String]) -> BlogResult<(Vec<TagRow>, Vec<String>)> {
        let rows = backbone_orm::company_scope::fetch_all_scoped(
            &self.pool,
            sqlx::query_as::<_, TagRow>(&format!(
                "SELECT {TAG_COLUMNS} FROM blog.tags
                  WHERE slug = ANY($1) AND metadata->>'deleted_at' IS NULL"
            ))
            .bind(slugs),
        )
        .await?;
        let found: Vec<String> = rows.iter().map(|r| r.slug.clone()).collect();
        let missed = slugs
            .iter()
            .filter(|s| !found.contains(s))
            .cloned()
            .collect();
        Ok((rows, missed))
    }

    // ── tag categories (master data) ─────────────────────────────────

    pub async fn list_categories(&self, limit: i64) -> BlogResult<Vec<TagCategoryRow>> {
        let rows = backbone_orm::company_scope::fetch_all_scoped(
            &self.pool,
            sqlx::query_as::<_, TagCategoryRow>(&format!(
                "SELECT {CATEGORY_COLUMNS} FROM blog.tag_categories
                  WHERE metadata->>'deleted_at' IS NULL
                  ORDER BY name LIMIT $1"
            ))
            .bind(limit),
        )
        .await?;
        Ok(rows)
    }

    pub async fn create_category(
        &self,
        company: Uuid,
        name: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<TagCategoryRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: TagCategoryRow = sqlx::query_as::<_, TagCategoryRow>(&format!(
            "INSERT INTO blog.tag_categories (id, company_id, name)
             VALUES (gen_random_uuid(), $1, $2)
             RETURNING {CATEGORY_COLUMNS}"
        ))
        .bind(company)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        record_audit(
            &mut tx,
            company,
            "tag_created",
            actor,
            "tag_category",
            Some(row.id),
            json!({ "name": row.name }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn patch_category(
        &self,
        id: Uuid,
        name: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<TagCategoryRow> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let row: TagCategoryRow = sqlx::query_as::<_, TagCategoryRow>(&format!(
            "UPDATE blog.tag_categories SET name = $2
               WHERE id = $1 AND metadata->>'deleted_at' IS NULL
               RETURNING {CATEGORY_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        record_audit(
            &mut tx,
            row.company_id,
            "tag_updated",
            actor,
            "tag_category",
            Some(row.id),
            json!({ "name": row.name }),
        )
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Delete a category; a category still grouping tags is a typed
    /// refusal (the RESTRICT FK surfaced as a 422 with a reason).
    pub async fn delete_category(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<()> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let company: Uuid = sqlx::query_scalar(
            "SELECT company_id FROM blog.tag_categories
              WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(BlogError::NotFound)?;
        let linked: i64 =
            sqlx::query_scalar("SELECT count(*) FROM blog.tags WHERE category_id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if linked > 0 {
            return Err(BlogError::InvalidInput(
                "category still groups tags".to_string(),
            ));
        }
        sqlx::query("DELETE FROM blog.tag_categories WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        record_audit(
            &mut tx,
            company,
            "tag_deleted",
            actor,
            "tag_category",
            Some(id),
            json!({}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_slug;

    #[test]
    fn kebab_normalization() {
        assert_eq!(normalize_slug("News & Announcements"), "news-announcements");
        assert_eq!(normalize_slug("  Über–Flow  "), "ber-flow");
        assert_eq!(normalize_slug("already-kebab"), "already-kebab");
        assert_eq!(normalize_slug("!!!"), "");
        assert_eq!(normalize_slug("A--B"), "a-b");
        assert_eq!(normalize_slug("-leading"), "leading");
    }
}
