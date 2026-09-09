//! The public query service (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC sections 4.3/4.4/5.
//!
//! Owns the ONE sibling-module edge (`Arc<dyn WebsiteSurface>`):
//! host binding (no fallback), the stale-slug redirect seam, and the
//! derived blog-slug match. The fence-composed SQL itself lives in
//! `PublicQueryRepository`; this layer resolves the website, the
//! blog, and the tag filter, then delegates.
//!
//! URL-shape decisions (port decisions, recorded here):
//! - Blog has NO slug column (the field census is binding): the
//!   public `{blog_slug}` path arm matches the DERIVED
//!   `normalize_slug(blog.name)` — stable while the name is.
//! - Post slug redirects use the full public path:
//!   `/public/blogs/{blog_slug}/posts/{old}` →
//!   `/public/blogs/{blog_slug}/posts/{new}` (moved_301). The detail
//!   handler asks the seam with the same shape.
//! - Tag slug redirects are module-relative (no single blog path
//!   exists): `/tags/{old}` → `/tags/{new}` (moved_301); the
//!   listing's tag filter asks the seam with that shape on a miss.
//!
//! Multi-tag GET rule (SPEC 4.4): more than one tag in the
//! comma-joined `?tag=` → 302 to the first tag's canonical listing
//! URL. Unknown member (no resolution, no redirect answer) → the
//! uniform 404 (D8 — lookup, never trust).

use std::sync::Arc;

use serde_json::{json, Value as Json};
use uuid::Uuid;

use backbone_website::exports::WebsiteSurface;

use crate::infrastructure::persistence::public_query_repository::{
    CloudEntry, ListingOrder, ListingPage, PublicBlog, PublicPostDetail, PublicQueryRepository,
};
use crate::infrastructure::persistence::tag_command_repository::{
    normalize_slug, TagCommandRepository,
};

use super::blog_error::{BlogError, BlogResult};

/// The tag-filter redirect shape (module-relative; see module doc).
fn tag_path(slug: &str) -> String {
    format!("/tags/{slug}")
}

/// A listing answer: the page, or the multi-tag 302.
#[derive(Debug)]
pub enum ListingAnswer {
    Page(ListingPage),
    RedirectTo(String),
}

/// A detail answer: the page, or the seam's redirect (301/302/308 by
/// the recorded kind; gone_404 carries no location).
#[derive(Debug)]
pub enum DetailAnswer {
    Page(PublicPostDetail),
    Redirect {
        status: u16,
        location: Option<String>,
    },
}

/// The public query service.
pub struct PublicQueryService {
    queries: PublicQueryRepository,
    tags: TagCommandRepository,
    surface: Arc<dyn WebsiteSurface>,
}

impl PublicQueryService {
    pub fn new(pool: sqlx::PgPool, surface: Arc<dyn WebsiteSurface>) -> Self {
        Self {
            queries: PublicQueryRepository::new(pool.clone()),
            tags: TagCommandRepository::new(pool),
            surface,
        }
    }

    /// Host → website binding (no fallback; the miss is the typed
    /// `blog_website_not_found`).
    pub async fn resolve_website(
        &self,
        host: &str,
    ) -> BlogResult<backbone_website::exports::WebsiteView> {
        self.surface
            .resolve_website_by_host(host)
            .await
            .map_err(|_| BlogError::WebsiteNotFound)
    }

    /// The website's live blogs (public tier).
    pub async fn blogs(&self, host: &str) -> BlogResult<PublicBlogsAnswer> {
        let website = self.resolve_website(host).await?;
        let blogs = self.queries.list_blogs(website.id).await?;
        Ok(PublicBlogsAnswer {
            host_binding: website,
            blogs,
        })
    }

    /// The listing (public tier) with the tag-filter grammar. `tags`
    /// is the RAW comma-joined `?tag=` value (None/empty = no filter).
    pub async fn listing(
        &self,
        host: &str,
        blog_slug: &str,
        tags: Option<&str>,
        page: i64,
        order: ListingOrder,
        search: Option<&str>,
    ) -> BlogResult<ListingAnswer> {
        let website = self.resolve_website(host).await?;
        let blogs = self.queries.list_blogs(website.id).await?;
        let blog = derive_blog(&blogs, blog_slug)?;

        let mut tag_slug = None;
        if let Some(raw) = tags.map(str::trim).filter(|t| !t.is_empty()) {
            let asked: Vec<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if asked.is_empty() {
                return Err(BlogError::InvalidInput("empty tag filter".to_string()));
            }
            // Scoped resolution (D8): lookup, never trust.
            let (found, missed) = self.tags.resolve_many(&asked).await?;
            let mut canonical: Vec<String> = found.iter().map(|t| t.slug.clone()).collect();
            for stale in &missed {
                // The seam, ONCE per stale member.
                if let Some(answer) = self
                    .surface
                    .redirect_answer(website.id, &tag_path(stale))
                    .await
                {
                    let Some(target) = answer.url_to else {
                        return Err(BlogError::NotFound);
                    };
                    let resolved = target.rsplit('/').next().unwrap_or_default().to_string();
                    if resolved.is_empty() {
                        return Err(BlogError::NotFound);
                    }
                    canonical.push(resolved);
                } else {
                    // Unknown slug: the uniform 404 (no oracle).
                    return Err(BlogError::NotFound);
                }
            }
            if canonical.is_empty() {
                return Err(BlogError::NotFound);
            }
            // MORE THAN ONE tag on a GET → 302 to the first tag's
            // canonical URL (the register's rule, faithful).
            if canonical.len() > 1 {
                return Ok(ListingAnswer::RedirectTo(format!(
                    "/public/blogs/{blog_slug}/posts?tag={}",
                    canonical[0]
                )));
            }
            tag_slug = canonical.into_iter().next();
        }

        let page = self
            .queries
            .listing(
                website.id,
                blog.id,
                &crate::infrastructure::persistence::public_query_repository::ListingQuery {
                    page,
                    tag_slug,
                    order,
                    search: search.map(str::to_string),
                },
            )
            .await?;
        Ok(ListingAnswer::Page(page))
    }

    /// The detail read (public tier): the visible post, or the seam's
    /// redirect ONCE, or the uniform 404.
    pub async fn detail(
        &self,
        host: &str,
        blog_slug: &str,
        post_slug: &str,
    ) -> BlogResult<DetailAnswer> {
        let website = self.resolve_website(host).await?;
        let blogs = self.queries.list_blogs(website.id).await?;
        let blog = derive_blog(&blogs, blog_slug)?;
        match self.queries.detail(website.id, blog.id, post_slug).await?
        {
            Some(mut detail) => {
                let og = og_meta(blog_slug, &detail);
                detail.og = Some(og);
                Ok(DetailAnswer::Page(detail))
            }
            None => {
                // The seam, ONCE: a recorded stale-slug 301 answers;
                // anything else is the uniform miss.
                let asked = format!("/public/blogs/{blog_slug}/posts/{post_slug}");
                if let Some(answer) = self.surface.redirect_answer(website.id, &asked).await {
                    let status = match answer.redirect_type.as_str() {
                        "moved_301" => 301,
                        "found_302" => 302,
                        "alias_308" => 308,
                        "gone_404" => 404,
                        _ => 301,
                    };
                    return Ok(DetailAnswer::Redirect {
                        status,
                        location: answer.url_to,
                    });
                }
                Err(BlogError::NotFound)
            }
        }
    }

    /// The public tag cloud (fence-joined, BL-6's declared query).
    pub async fn cloud(
        &self,
        host: &str,
        blog_slug: &str,
        min_limit: i64,
    ) -> BlogResult<Vec<CloudEntry>> {
        let website = self.resolve_website(host).await?;
        let blogs = self.queries.list_blogs(website.id).await?;
        let blog = derive_blog(&blogs, blog_slug)?;
        self.queries
            .cloud_public(website.id, blog.id, min_limit)
            .await
    }
}

/// The blogs answer: the host binding (the caller may need the
/// website row) plus the live blogs.
#[derive(Debug)]
pub struct PublicBlogsAnswer {
    pub host_binding: backbone_website::exports::WebsiteView,
    pub blogs: Vec<PublicBlog>,
}

/// Match a `{blog_slug}` path arm against the DERIVED slug of each
/// live blog name (Blog has no slug column — the census is binding).
fn derive_blog(blogs: &[PublicBlog], blog_slug: &str) -> BlogResult<PublicBlog> {
    blogs
        .iter()
        .find(|b| normalize_slug(&b.name) == blog_slug)
        .cloned()
        .ok_or(BlogError::NotFound)
}

/// The OpenGraph meta shape (derived, never stored).
fn og_meta(blog_slug: &str, detail: &PublicPostDetail) -> Json {
    json!({
        "og:title": detail.title,
        "og:description": detail.teaser.clone()
            .or_else(|| detail.content.clone())
            .map(|c| c.chars().take(200).collect::<String>()),
        "og:image": detail.cover.as_ref()
            .and_then(|c| c.get("url"))
            .and_then(|u| u.as_str())
            .map(str::to_string),
        "og:url": format!("/public/blogs/{blog_slug}/posts/{}", detail.slug),
        "og:type": "article",
    })
}

/// The post-redirect recorder (shared by the patch routes): ONE call,
/// never `from == to` (callers only invoke on a real change), never a
/// target that does not resolve (the target is the row's own new
/// slug). The permanent kind rides WB-3's table.
pub async fn record_post_slug_redirect(
    surface: &dyn WebsiteSurface,
    website_id: Uuid,
    blog_slug: &str,
    from_slug: &str,
    to_slug: &str,
) {
    let from = format!("/public/blogs/{blog_slug}/posts/{from_slug}");
    let to = format!("/public/blogs/{blog_slug}/posts/{to_slug}");
    if from == to {
        return; // the loop guard
    }
    if let Err(e) = surface
        .record_redirect(website_id, &from, &to, "moved_301")
        .await
    {
        // Best-effort by design (the stale slug 404s without the
        // redirect; the rename itself already committed) — park loud.
        tracing::warn!(from, to, error = %e, "post slug redirect not recorded");
    }
}

/// The tag-redirect recorder (module-relative shape; see the module
/// doc).
pub async fn record_tag_slug_redirect(
    surface: &dyn WebsiteSurface,
    website_id: Uuid,
    from_slug: &str,
    to_slug: &str,
) {
    if from_slug == to_slug {
        return; // the loop guard
    }
    let (from, to) = (tag_path(from_slug), tag_path(to_slug));
    if let Err(e) = surface
        .record_redirect(website_id, &from, &to, "moved_301")
        .await
    {
        tracing::warn!(from, to, error = %e, "tag slug redirect not recorded");
    }
}
