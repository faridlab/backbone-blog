//! The fence-composed read family (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC sections 4.3/4.4/5.
//!
//! Every PUBLIC method opens ONE transaction and binds the PUBLIC
//! scope FIRST (`bind_public_scope`: company + `app.blog_tier`) —
//! then composes the visibility predicate (`is_published AND
//! post_date <= now() AND archived_at IS NULL AND not soft-deleted`)
//! EXPLICITLY in the query text. Belt and suspenders: the predicate
//! is the honest read; the restrictive policy double-enforces it at
//! the DB for public connections even if a predicate were forgotten.
//! The ADMIN methods bind the plain company scope and compose NO
//! visibility predicate (the declared "employees see everything"
//! posture — SPEC section 2.3).
//!
//! Pagination note (the port decision): the listing walks the
//! `(post_date DESC, id DESC)` order the composite index keys, served
//! as fixed 12-row pages by OFFSET — the Odoo page grain. Deep paging
//! stays cheap because a blog's visible set is bounded by its
//! content, not by the table.

use serde_json::Value as Json;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::application::service::blog_error::BlogResult;
use crate::application::service::site_scope::bind_public_scope;

/// The Odoo listing page grain.
pub const PAGE_SIZE: i64 = 12;

/// The visible-post predicate, spelled identically everywhere a
/// public read touches posts (SPEC section 4.3). Every column is
/// qualified with the alias — a bare `metadata` turns ambiguous the
/// moment a query joins a second metadata-bearing table (the tag
/// filter joins `blog.tags`).
fn visible(alias: &str) -> String {
    format!(
        "{a}.is_published AND {a}.post_date <= now() AND {a}.archived_at IS NULL \
         AND {a}.metadata->>'deleted_at' IS NULL",
        a = alias
    )
}

/// A blog as the public list serves it.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct PublicBlog {
    pub id: Uuid,
    pub name: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
}

/// One listing card.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ListingItem {
    pub id: Uuid,
    pub blog_id: Uuid,
    pub title: String,
    pub slug: String,
    pub teaser: Option<String>,
    pub cover: Option<Json>,
    pub author_name: Option<String>,
    pub post_date: chrono::DateTime<chrono::Utc>,
    pub visits: i32,
}

/// The listing query surface (SPEC section 5).
#[derive(Debug, Clone, Default)]
pub struct ListingQuery {
    /// 1-based page number.
    pub page: i64,
    /// Tag filter (single canonical slug — the route 302s multi-tag
    /// GETs to the first tag before querying).
    pub tag_slug: Option<String>,
    /// `recent` (default) or `visits` (Most Viewed).
    pub order: ListingOrder,
    /// ILIKE title/teaser/author_name.
    pub search: Option<String>,
}

/// The listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListingOrder {
    #[default]
    Recent,
    Visits,
}

/// A listing page.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ListingPage {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub items: Vec<ListingItem>,
}

/// A tag card in a cloud / detail payload.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TagLite {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

/// A cloud entry (BL-6, re-expressed as this declared guarded query).
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct CloudEntry {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub post_count: i64,
}

/// The public detail payload (OpenGraph-shaped meta is derived by
/// the service; the repository returns facts).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicPostDetail {
    pub id: Uuid,
    pub blog_id: Uuid,
    pub title: String,
    pub slug: String,
    pub content: Option<String>,
    pub teaser: Option<String>,
    pub cover: Option<Json>,
    pub author_name: Option<String>,
    pub post_date: chrono::DateTime<chrono::Utc>,
    pub published_date: Option<chrono::DateTime<chrono::Utc>>,
    pub visits: i32,
    pub allow_comments: bool,
    pub tags: Vec<TagLite>,
    /// The circular-tour neighbor (BL-11's modulo wrap-around).
    pub nav_next: Option<NavTarget>,
    pub updated_at: Option<String>,
    /// Derived OpenGraph meta (shaped by the service, not stored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub og: Option<Json>,
}

/// A nav target.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct NavTarget {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
}

/// The admin `?state=` split counts (future-aware: a future-dated
/// published post counts as UNPUBLISHED — SPEC section 4.3).
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct StateCounts {
    pub published: i64,
    pub unpublished: i64,
}

#[derive(Clone)]
pub struct PublicQueryRepository {
    pool: PgPool,
}

impl PublicQueryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The live blogs of the website (public tier).
    pub async fn list_blogs(&self, company: Uuid, website_id: Uuid) -> BlogResult<Vec<PublicBlog>> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx, company).await?;
        let blogs = sqlx::query_as::<_, PublicBlog>(
            "SELECT id, name, subtitle, description FROM blog.blogs
              WHERE website_id = $1
                AND archived_at IS NULL
                AND metadata->>'deleted_at' IS NULL
              ORDER BY name",
        )
        .bind(website_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(blogs)
    }

    /// The listing (public tier): the fence arms + optional single-tag
    /// filter + order + search, fixed 12-row pages.
    pub async fn listing(
        &self,
        company: Uuid,
        website_id: Uuid,
        blog_id: Uuid,
        query: &ListingQuery,
    ) -> BlogResult<ListingPage> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx, company).await?;
        let page = query.page.max(1);
        let offset = (page - 1) * PAGE_SIZE;

        // Two fixed parameter layouts (the tag join cannot be a
        // NULL-tolerant arm — an INNER JOIN would silently drop
        // untagged posts from an unfiltered listing):
        //  - no tag: $1 website, $2 blog, $3 search (NULL = none)
        //  - tag:    $1 website, $2 blog, $3 tag,  $4 search (NULL = none)
        let order = match query.order {
            ListingOrder::Recent => "p.post_date DESC, p.id DESC",
            ListingOrder::Visits => "p.visits DESC, p.post_date DESC, p.id DESC",
        };
        let search_pattern = query.search.as_ref().map(|s| format!("%{s}%"));
        let visible_p = visible("p");
        let (items_sql, count_sql, has_tag) = match query.tag_slug.as_deref() {
            Some(_) => (
                format!(
                    "SELECT p.id, p.blog_id, p.title, p.slug, p.teaser, p.cover, p.author_name,
                            p.post_date, p.visits
                       FROM blog.posts p
                       JOIN blog.post_tags pt_f ON pt_f.post_id = p.id
                       JOIN blog.tags t_f ON t_f.id = pt_f.tag_id AND t_f.slug = $3
                      WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
                        AND ($4::text IS NULL
                             OR p.title ILIKE $4 OR p.teaser ILIKE $4 OR p.author_name ILIKE $4)
                      ORDER BY {order}
                      LIMIT {PAGE_SIZE} OFFSET {offset}"
                ),
                format!(
                    "SELECT count(*)
                       FROM blog.posts p
                       JOIN blog.post_tags pt_f ON pt_f.post_id = p.id
                       JOIN blog.tags t_f ON t_f.id = pt_f.tag_id AND t_f.slug = $3
                      WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
                        AND ($4::text IS NULL
                             OR p.title ILIKE $4 OR p.teaser ILIKE $4 OR p.author_name ILIKE $4)"
                ),
                true,
            ),
            None => (
                format!(
                    "SELECT p.id, p.blog_id, p.title, p.slug, p.teaser, p.cover, p.author_name,
                            p.post_date, p.visits
                       FROM blog.posts p
                      WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
                        AND ($3::text IS NULL
                             OR p.title ILIKE $3 OR p.teaser ILIKE $3 OR p.author_name ILIKE $3)
                      ORDER BY {order}
                      LIMIT {PAGE_SIZE} OFFSET {offset}"
                ),
                format!(
                    "SELECT count(*) FROM blog.posts p
                      WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
                        AND ($3::text IS NULL
                             OR p.title ILIKE $3 OR p.teaser ILIKE $3 OR p.author_name ILIKE $3)"
                ),
                false,
            ),
        };

        let mut q = sqlx::query_as::<_, ListingItem>(&items_sql)
            .bind(website_id)
            .bind(blog_id);
        if has_tag {
            q = q.bind(query.tag_slug.clone().unwrap_or_default());
        }
        q = q.bind(search_pattern.clone());
        let items = q.fetch_all(&mut *tx).await?;

        let mut cq = sqlx::query_scalar::<_, i64>(&count_sql)
            .bind(website_id)
            .bind(blog_id);
        if has_tag {
            cq = cq.bind(query.tag_slug.clone().unwrap_or_default());
        }
        cq = cq.bind(search_pattern);
        let total = cq.fetch_one(&mut *tx).await?;

        tx.commit().await?;
        Ok(ListingPage {
            page,
            page_size: PAGE_SIZE,
            total,
            items,
        })
    }

    /// The detail read (public tier): one visible post + its tags +
    /// the wrap-around nav target. `None` = the uniform miss (the
    /// service asks the redirect seam once before refusing).
    pub async fn detail(
        &self,
        company: Uuid,
        website_id: Uuid,
        blog_id: Uuid,
        post_slug: &str,
    ) -> BlogResult<Option<PublicPostDetail>> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx, company).await?;
        let visible_p = visible("p");
        let row: Option<DetailRow> = sqlx::query_as::<_, DetailRow>(&format!(
            "SELECT p.id, p.blog_id, p.title, p.slug, p.content, p.teaser, p.cover, p.author_name,
                    p.post_date, p.published_date, p.visits, p.allow_comments,
                    p.metadata->>'updated_at' AS updated_at
               FROM blog.posts p
              WHERE p.website_id = $1 AND p.blog_id = $2 AND p.slug = $3 AND {visible_p}"
        ))
        .bind(website_id)
        .bind(blog_id)
        .bind(post_slug)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let tags = Self::tags_of_post(&mut tx, row.id).await?;
        let nav_next = Self::nav_next(&mut tx, website_id, blog_id, row.post_date, row.id).await?;
        tx.commit().await?;
        Ok(Some(PublicPostDetail {
            id: row.id,
            blog_id: row.blog_id,
            title: row.title,
            slug: row.slug,
            content: row.content,
            teaser: row.teaser,
            cover: row.cover,
            author_name: row.author_name,
            post_date: row.post_date,
            published_date: row.published_date,
            visits: row.visits,
            allow_comments: row.allow_comments,
            tags,
            nav_next,
            updated_at: row.updated_at,
            og: None,
        }))
    }

    /// The tag cloud (public tier) — BL-6's GROUP BY re-expressed as
    /// this ONE declared guarded query (SPEC section 4.4). The public
    /// variant joins the fence; the admin variant ([`cloud_admin`])
    /// joins the full set.
    pub async fn cloud_public(
        &self,
        company: Uuid,
        website_id: Uuid,
        blog_id: Uuid,
        min_limit: i64,
    ) -> BlogResult<Vec<CloudEntry>> {
        let mut tx = self.pool.begin().await?;
        bind_public_scope(&mut tx, company).await?;
        let entries = sqlx::query_as::<_, CloudEntry>(
            "SELECT t.id, t.name, t.slug, count(*) AS post_count
               FROM blog.tags t
               JOIN blog.post_tags pt ON pt.tag_id = t.id
               JOIN blog.posts p ON p.id = pt.post_id
              WHERE p.website_id = $1 AND p.blog_id = $2
                AND p.archived_at IS NULL AND p.is_published AND p.post_date <= now()
                AND p.metadata->>'deleted_at' IS NULL
                AND t.metadata->>'deleted_at' IS NULL
              GROUP BY t.id, t.name, t.slug
             HAVING count(*) >= $3
              ORDER BY post_count DESC, t.name",
        )
        .bind(website_id)
        .bind(blog_id)
        .bind(min_limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(entries)
    }

    /// The admin tag cloud (the full set — no visibility predicate,
    /// plain company scope).
    pub async fn cloud_admin(&self, blog_id: Uuid, min_limit: i64) -> BlogResult<Vec<CloudEntry>> {
        let mut tx = self.pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let entries = sqlx::query_as::<_, CloudEntry>(
            "SELECT t.id, t.name, t.slug, count(*) AS post_count
               FROM blog.tags t
               JOIN blog.post_tags pt ON pt.tag_id = t.id
               JOIN blog.posts p ON p.id = pt.post_id
              WHERE p.blog_id = $1
                AND p.metadata->>'deleted_at' IS NULL
                AND t.metadata->>'deleted_at' IS NULL
              GROUP BY t.id, t.name, t.slug
             HAVING count(*) >= $2
              ORDER BY post_count DESC, t.name",
        )
        .bind(blog_id)
        .bind(min_limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(entries)
    }

    /// The admin listing with the `?state=` fork (SPEC section 4.3):
    /// *published* ≡ visible (future-aware — a future-dated published
    /// post counts as unpublished in the split).
    pub async fn admin_list_posts(
        &self,
        state: AdminState,
        blog_id: Option<Uuid>,
        limit: i64,
    ) -> BlogResult<(Vec<ListingItem>, StateCounts)> {
        let state_arm = match state {
            AdminState::All => "",
            AdminState::Published => {
                "AND is_published AND post_date <= now() AND archived_at IS NULL"
            }
            AdminState::Unpublished => {
                "AND NOT (is_published AND post_date <= now() AND archived_at IS NULL)"
            }
        };
        let rows = backbone_orm::company_scope::fetch_all_scoped(
            &self.pool,
            sqlx::query_as::<_, ListingItem>(&format!(
                "SELECT id, blog_id, title, slug, teaser, cover, author_name, post_date, visits
                   FROM blog.posts
                  WHERE metadata->>'deleted_at' IS NULL
                    AND ($1::uuid IS NULL OR blog_id = $1)
                    {state_arm}
                  ORDER BY post_date DESC, id DESC
                  LIMIT $2"
            ))
            .bind(blog_id)
            .bind(limit),
        )
        .await?;
        let counts = self.state_counts(blog_id).await?;
        Ok((rows, counts))
    }

    /// Future-aware split counts for the admin listing (same query
    /// family, no visibility state filter).
    pub async fn state_counts(&self, blog_id: Option<Uuid>) -> BlogResult<StateCounts> {
        let (published, unpublished): (i64, i64) = backbone_orm::company_scope::fetch_one_scoped(
            &self.pool,
            sqlx::query_as(
                "SELECT
                        COALESCE(SUM(CASE WHEN is_published AND post_date <= now()
                                           AND archived_at IS NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN NOT (is_published AND post_date <= now()
                                           AND archived_at IS NULL) THEN 1 ELSE 0 END), 0)
                       FROM blog.posts
                      WHERE metadata->>'deleted_at' IS NULL
                        AND ($1::uuid IS NULL OR blog_id = $1)",
            )
            .bind(blog_id),
        )
        .await?;
        Ok(StateCounts {
            published,
            unpublished,
        })
    }

    /// The tags of one post (detail payload; public tier — composed
    /// INSIDE the detail transaction by [`Self::detail`]).
    async fn tags_of_post(
        tx: &mut sqlx::PgConnection,
        post_id: Uuid,
    ) -> Result<Vec<TagLite>, sqlx::Error> {
        sqlx::query_as::<_, TagLite>(
            "SELECT t.id, t.name, t.slug
               FROM blog.tags t
               JOIN blog.post_tags pt ON pt.tag_id = t.id
              WHERE pt.post_id = $1 AND t.metadata->>'deleted_at' IS NULL
              ORDER BY t.name",
        )
        .bind(post_id)
        .fetch_all(tx)
        .await
    }

    /// The circular-tour neighbor: the next older visible post, or —
    /// at the end of the tour — the NEWEST visible post (BL-11's
    /// modulo wrap-around, faithful; a one-post set wraps to itself).
    async fn nav_next(
        tx: &mut sqlx::PgConnection,
        website_id: Uuid,
        blog_id: Uuid,
        post_date: chrono::DateTime<chrono::Utc>,
        id: Uuid,
    ) -> Result<Option<NavTarget>, sqlx::Error> {
        let visible_p = visible("p");
        let next = sqlx::query_as::<_, NavTarget>(&format!(
            "SELECT p.id, p.slug, p.title FROM blog.posts p
              WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
                AND (p.post_date, p.id) < ($3, $4)
              ORDER BY p.post_date DESC, p.id DESC
              LIMIT 1"
        ))
        .bind(website_id)
        .bind(blog_id)
        .bind(post_date)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if next.is_some() {
            return Ok(next);
        }
        // Wrap-around: the newest visible post of the set.
        sqlx::query_as::<_, NavTarget>(&format!(
            "SELECT p.id, p.slug, p.title FROM blog.posts p
              WHERE p.website_id = $1 AND p.blog_id = $2 AND {visible_p}
              ORDER BY p.post_date DESC, p.id DESC
              LIMIT 1"
        ))
        .bind(website_id)
        .bind(blog_id)
        .fetch_optional(&mut *tx)
        .await
    }
}

/// The admin listing's state fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdminState {
    #[default]
    All,
    Published,
    Unpublished,
}

/// The intermediate detail row (repository-internal).
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
struct DetailRow {
    id: Uuid,
    blog_id: Uuid,
    title: String,
    slug: String,
    content: Option<String>,
    teaser: Option<String>,
    cover: Option<Json>,
    author_name: Option<String>,
    post_date: chrono::DateTime<chrono::Utc>,
    published_date: Option<chrono::DateTime<chrono::Utc>>,
    visits: i32,
    allow_comments: bool,
    updated_at: Option<String>,
}
