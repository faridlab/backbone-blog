//! The module's ADMIN route surface (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 10.
//!
//! The module DOES NOT SELF-MOUNT: it exports [`blog_admin_routes`],
//! a plain `axum::Router` the composing host nests under the schema
//! name BEHIND its org session guard (`backbone_auth`'s `org_auth`
//! middleware, which authenticates the session and inserts
//! [`OrgContext`]). The tenancy posture (ADR-0029) holds at the
//! repository layer: every transactional method relays the ambient
//! org scope onto its transaction; the admin reads see ALL states
//! (the declared "employees see everything" posture — SPEC section
//! 2.3).
//!
//! The acting OFFICER id is derived from the [`OrgContext`] the
//! guard inserted (the authenticated principal, `org.user_id`); a
//! principal id that is not a uuid audits with a NULL actor — the
//! admin verbs never fall back to a public principal. Because
//! [`OrgContext`] is a REQUIRED extractor, a request that reaches a
//! handler without the guard having run is answered 401 (a
//! miscomposed host fails loud, not silently unattributed).
//!
//! Route table:
//! - GET/POST     /admin/blogs                        list (?website_id=) / create
//! - GET/PATCH    /admin/blogs/{id}                   read / typed patch (website_id
//!                                                    refused while posts exist)
//! - DELETE       /admin/blogs/{id}                   empty-blog delete (409 while posts)
//! - POST         /admin/blogs/{id}/archive           the cascade verb
//! - POST         /admin/blogs/{id}/unarchive         the marker-restore verb
//! - GET          /admin/blogs/{id}/tags              admin tag cloud (full set)
//! - GET/POST     /admin/posts                        list (?state=, ?blog_id=, counts
//!                                                    future-aware) / create
//! - GET/PATCH    /admin/posts/{id}                   read / typed patch (FENCE:
//!                                                    is_published, published_date)
//! - POST         /admin/posts/{id}/publish           the coupling verb (section 4.1)
//! - POST         /admin/posts/{id}/unpublish         flip false, stamp retained
//! - POST         /admin/posts/{id}/archive           forced unpublish, one-way
//! - POST         /admin/posts/{id}/unarchive         liveness only, never re-publish
//! - PUT          /admin/posts/{id}/tags              set the tag id list (resolved)
//! - GET/POST     /admin/tags                         list / create
//! - PATCH/DELETE /admin/tags/{id}                    rename (redirect recorded) /
//!                                                    untag-everywhere
//! - GET/POST     /admin/tag-categories               (+ PATCH/DELETE /{id})
//!
//! Slug changes on live content record the stale-slug redirect
//! through the website seam (moved_301) — the surface is OPTIONAL at
//! state compose so probes without one still exercise the verbs; a
//! slug change without a surface parks a WARN (the redirect is
//! best-effort by design; the rename itself already committed).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use serde_json::{json, Value as Body};
use uuid::Uuid;

use backbone_auth::org::OrgContext;
use backbone_website::exports::WebsiteSurface;

use crate::application::service::blog_error::{BlogError, BlogResult};
use crate::application::service::blog_service::BlogCommandService;
use crate::application::service::notifier_port::BlogPublishNotifier;
use crate::application::service::post_service::{
    PostCommandService, PUBLISH_FENCED_FIELDS, REFUSED_FIELDS,
};
use crate::application::service::public_query_service::{
    record_post_slug_redirect, record_tag_slug_redirect,
};
use crate::application::service::tag_service::TagCommandService;
use crate::infrastructure::persistence::blog_command_repository::{
    CreateBlogInput, PatchBlogInput,
};
use crate::infrastructure::persistence::post_command_repository::{
    CreatePostInput, PatchPostInput,
};
use crate::infrastructure::persistence::public_query_repository::{
    AdminState, PublicQueryRepository,
};
use crate::infrastructure::persistence::tag_command_repository::normalize_slug;

/// The acting officer, derived from the org session the host's guard
/// authenticated: the principal id (`user_id`) parsed as a uuid. A
/// non-uuid principal audits with a NULL actor — attribution is
/// best-effort at this version, never a fake identity.
fn officer_of(org: &OrgContext) -> Option<Uuid> {
    Uuid::parse_str(&org.user_id).ok()
}

/// The shared admin state (cheap-to-clone service handles).
#[derive(Clone)]
pub struct BlogAdminState {
    blogs: Arc<BlogCommandService>,
    posts: Arc<PostCommandService>,
    tags: Arc<TagCommandService>,
    queries: Arc<PublicQueryRepository>,
    surface: Option<Arc<dyn WebsiteSurface>>,
}

impl BlogAdminState {
    /// Compose over one pool. `notifier` is the host's publish
    /// adapter (the module default is the loud refusing one);
    /// `surface` enables slug-redirect recording (None = park WARNs).
    pub fn new(
        pool: sqlx::PgPool,
        notifier: Arc<dyn BlogPublishNotifier>,
        surface: Option<Arc<dyn WebsiteSurface>>,
    ) -> Self {
        Self::from_parts(
            Arc::new(BlogCommandService::new(
                crate::infrastructure::persistence::blog_command_repository::BlogCommandRepository::new(
                    pool.clone(),
                ),
            )),
            Arc::new(PostCommandService::new(
                crate::infrastructure::persistence::post_command_repository::PostCommandRepository::new(
                    pool.clone(),
                ),
                notifier,
            )),
            Arc::new(TagCommandService::new(
                crate::infrastructure::persistence::tag_command_repository::TagCommandRepository::new(
                    pool.clone(),
                ),
            )),
            Arc::new(PublicQueryRepository::new(pool)),
            surface,
        )
    }

    /// Compose over already-built services (the module builder's
    /// entry — no second construction).
    pub fn from_parts(
        blogs: Arc<BlogCommandService>,
        posts: Arc<PostCommandService>,
        tags: Arc<TagCommandService>,
        queries: Arc<PublicQueryRepository>,
        surface: Option<Arc<dyn WebsiteSurface>>,
    ) -> Self {
        Self {
            blogs,
            posts,
            tags,
            queries,
            surface,
        }
    }
}

/// THE ADMIN TREE (the table in the module doc).
pub fn blog_admin_routes(state: BlogAdminState) -> Router {
    Router::new()
        .route("/admin/blogs", get(list_blogs).post(create_blog))
        .route(
            "/admin/blogs/:id",
            get(get_blog).patch(patch_blog).delete(delete_blog),
        )
        .route("/admin/blogs/:id/archive", post(archive_blog))
        .route("/admin/blogs/:id/unarchive", post(unarchive_blog))
        .route("/admin/blogs/:id/tags", get(blog_tag_cloud))
        .route("/admin/posts", get(list_posts).post(create_post))
        .route("/admin/posts/:id", get(get_post).patch(patch_post))
        .route("/admin/posts/:id/publish", post(publish_post))
        .route("/admin/posts/:id/unpublish", post(unpublish_post))
        .route("/admin/posts/:id/archive", post(archive_post))
        .route("/admin/posts/:id/unarchive", post(unarchive_post))
        .route("/admin/posts/:id/tags", put(set_post_tags))
        .route("/admin/tags", get(list_tags).post(create_tag))
        .route("/admin/tags/:id", patch(patch_tag).delete(delete_tag))
        .route(
            "/admin/tag-categories",
            get(list_tag_categories).post(create_tag_category),
        )
        .route(
            "/admin/tag-categories/:id",
            patch(patch_tag_category).delete(delete_tag_category),
        )
        .with_state(state)
}

// ── blogs ─────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct IdQuery {
    website_id: Option<String>,
    blog_id: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
}

fn uuid_param(raw: &Option<String>) -> Option<Uuid> {
    raw.as_deref().and_then(|s| Uuid::parse_str(s).ok())
}

async fn list_blogs(State(state): State<BlogAdminState>, Query(q): Query<IdQuery>) -> Response {
    reply(state.blogs.list(uuid_param(&q.website_id)).await.map(Json))
}

async fn create_blog(
    State(state): State<BlogAdminState>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let input = CreateBlogInput {
        website_id: match uuid_of(&body, "website_id") {
            Some(id) => id,
            None => {
                return BlogError::InvalidInput("website_id is required".into()).into_response()
            }
        },
        name: match string_of(&body, "name") {
            Ok(name) => name,
            Err(e) => return e.into_response(),
        },
        subtitle: opt_string_of(&body, "subtitle"),
        description: opt_string_of(&body, "description"),
    };
    reply(
        state
            .blogs
            .create(&input, officer_of(&org))
            .await
            .map(|row| (axum::http::StatusCode::CREATED, Json(row))),
    )
}

async fn get_blog(State(state): State<BlogAdminState>, Path(id): Path<Uuid>) -> Response {
    reply(state.blogs.get(id).await.map(Json))
}

async fn patch_blog(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let patch = PatchBlogInput {
        name: opt_string_of(&body, "name"),
        subtitle: opt_string_of(&body, "subtitle"),
        description: opt_string_of(&body, "description"),
    };
    let website_move = uuid_of(&body, "website_id");
    reply(
        state
            .blogs
            .patch(id, &patch, website_move, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn delete_blog(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .blogs
            .delete(id, officer_of(&org))
            .await
            .map(|_| {
                (
                    axum::http::StatusCode::NO_CONTENT,
                    Json(json!({ "deleted": true })),
                )
            }),
    )
}

async fn archive_blog(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .blogs
            .archive(id, officer_of(&org))
            .await
            .map(|(row, cascaded)| Json(json!({ "blog": row, "posts_cascade": cascaded }))),
    )
}

async fn unarchive_blog(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .blogs
            .unarchive(id, officer_of(&org))
            .await
            .map(|(row, restored)| Json(json!({ "blog": row, "posts_restored": restored }))),
    )
}

async fn blog_tag_cloud(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    Query(q): Query<IdQuery>,
) -> Response {
    // The ADMIN cloud: the full set, no visibility predicate (the
    // fence-joined twin is the public route's).
    reply(
        state
            .queries
            .cloud_admin(id, q.limit.unwrap_or(1).max(1))
            .await
            .map(|entries| Json(json!({ "tags": entries }))),
    )
}

// ── posts ─────────────────────────────────────────────────────────────────

async fn list_posts(State(state): State<BlogAdminState>, Query(q): Query<IdQuery>) -> Response {
    let blog_state = match q.state.as_deref() {
        Some("published") => AdminState::Published,
        Some("unpublished") => AdminState::Unpublished,
        _ => AdminState::All,
    };
    reply(
        state
            .queries
            .admin_list_posts(
                blog_state,
                uuid_param(&q.blog_id),
                q.limit.unwrap_or(50).clamp(1, 500),
            )
            .await
            .map(|(items, counts)| Json(json!({ "items": items, "counts": counts }))),
    )
}

async fn create_post(
    State(state): State<BlogAdminState>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let input = CreatePostInput {
        blog_id: match uuid_of(&body, "blog_id") {
            Some(id) => id,
            None => return BlogError::InvalidInput("blog_id is required".into()).into_response(),
        },
        title: match string_of(&body, "title") {
            Ok(t) => t,
            Err(e) => return e.into_response(),
        },
        slug: opt_string_of(&body, "slug")
            .unwrap_or_else(|| string_of(&body, "title").unwrap_or_default()),
        content: opt_string_of(&body, "content"),
        teaser: opt_string_of(&body, "teaser"),
        cover: body.get("cover").cloned().filter(|v| v.is_object()),
        author_officer: uuid_of(&body, "author_officer"),
        author_name: opt_string_of(&body, "author_name"),
        post_date: match opt_ts(&body, "post_date") {
            Ok(d) => d.unwrap_or_else(chrono::Utc::now),
            Err(e) => return e.into_response(),
        },
        allow_comments: bool_of(&body, "allow_comments").unwrap_or(false),
    };
    reply(
        state
            .posts
            .create(&input, officer_of(&org))
            .await
            .map(|row| (axum::http::StatusCode::CREATED, Json(row))),
    )
}

async fn get_post(State(state): State<BlogAdminState>, Path(id): Path<Uuid>) -> Response {
    reply(state.posts.get(id).await.map(Json))
}

async fn patch_post(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    // THE FENCE: the publication pair in a patch body is the typed
    // refusal + the `publish_refused` audit fact (publish/unpublish
    // are the only writers). Structurally-refused fields (no patch
    // arm exists) answer the same typed refusal.
    if let Some(obj) = body.as_object() {
        let fence_hits: Vec<&str> = PUBLISH_FENCED_FIELDS
            .iter()
            .chain(REFUSED_FIELDS.iter())
            .filter(|f| obj.contains_key(**f))
            .copied()
            .collect();
        if !fence_hits.is_empty() {
            let fields = fence_hits.join(", ");
            let audit_only_fence = fence_hits.iter().any(|f| PUBLISH_FENCED_FIELDS.contains(f));
            if audit_only_fence {
                let _ = state
                    .posts
                    .audit_fence_refusal(id, &fields, officer_of(&org))
                    .await;
            }
            return BlogError::FieldNotPatchable(fields).into_response();
        }
    }
    let patch = PatchPostInput {
        title: opt_string_of(&body, "title"),
        slug: opt_string_of(&body, "slug"),
        content: opt_string_of(&body, "content"),
        teaser: opt_string_of(&body, "teaser"),
        cover: body.get("cover").cloned().filter(|v| v.is_object()),
        post_date: match opt_ts(&body, "post_date") {
            Ok(d) => d,
            Err(e) => return e.into_response(),
        },
        allow_comments: bool_of(&body, "allow_comments"),
        author_officer: uuid_of(&body, "author_officer"),
        author_name: opt_string_of(&body, "author_name"),
    };
    match state.posts.patch(id, &patch, officer_of(&org)).await {
        Ok((row, prior_slug)) => {
            // The stale-slug redirect (the verb already committed; the
            // recording is best-effort — see the module doc). The
            // recorded path is the FULL public post URL, so the blog's
            // derived slug is resolved first.
            if let (Some(prior), Some(surface)) = (&prior_slug, &state.surface) {
                match state.blogs.get(row.blog_id).await {
                    Ok(blog) => {
                        record_post_slug_redirect(
                            surface.as_ref(),
                            row.website_id,
                            &normalize_slug(&blog.name),
                            prior,
                            &row.slug,
                        )
                        .await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            post = row.id.to_string(),
                            error = e.code(),
                            "post slug redirect not recorded (blog unreadable)"
                        );
                    }
                }
            }
            (axum::http::StatusCode::OK, Json(row)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn publish_post(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .posts
            .publish(id, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn unpublish_post(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .posts
            .unpublish(id, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn archive_post(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .posts
            .archive(id, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn unarchive_post(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .posts
            .unarchive(id, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn set_post_tags(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let Some(tag_ids) = uuid_vec_of(&body, "tag_ids") else {
        return BlogError::InvalidInput("tag_ids is required".into()).into_response();
    };
    reply(
        state
            .posts
            .set_tags(id, &tag_ids, officer_of(&org))
            .await
            .map(|ids| Json(json!({ "tag_ids": ids }))),
    )
}

// ── tags ──────────────────────────────────────────────────────────────────

async fn list_tags(State(state): State<BlogAdminState>) -> Response {
    reply(state.tags.list().await.map(Json))
}

async fn create_tag(
    State(state): State<BlogAdminState>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let name = match string_of(&body, "name") {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    reply(
        state
            .tags
            .create(
                &name,
                uuid_of(&body, "category_id"),
                officer_of(&org),
            )
            .await
            .map(|row| (axum::http::StatusCode::CREATED, Json(row))),
    )
}

async fn patch_tag(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let name = opt_string_of(&body, "name");
    // `category_id` is a two-level Option: absent = untouched,
    // explicit null = clear the grouping.
    let category_id = match body.get("category_id") {
        None => None,
        Some(v) if v.is_null() => Some(None),
        Some(v) => match v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            Some(id) => Some(Some(id)),
            None => {
                return BlogError::InvalidInput("category_id must be a uuid or null".into())
                    .into_response()
            }
        },
    };
    let website_id = uuid_of(&body, "website_id");
    match state
        .tags
        .rename(id, name.as_deref(), category_id, officer_of(&org))
        .await
    {
        Ok((row, prior_slug)) => {
            if let (Some(prior), Some(surface)) = (&prior_slug, &state.surface) {
                if let Some(website) = website_id {
                    record_tag_slug_redirect(surface.as_ref(), website, prior, &row.slug).await;
                } else {
                    tracing::warn!(
                        tag = row.id.to_string(),
                        from = prior,
                        "tag slug changed without website_id: no redirect recorded"
                    );
                }
            }
            (axum::http::StatusCode::OK, Json(row)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn delete_tag(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .tags
            .delete(id, officer_of(&org))
            .await
            .map(|untagged| Json(json!({ "untagged": untagged }))),
    )
}

// ── tag categories ────────────────────────────────────────────────────────

async fn list_tag_categories(State(state): State<BlogAdminState>) -> Response {
    reply(state.tags.list_categories().await.map(Json))
}

async fn create_tag_category(
    State(state): State<BlogAdminState>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let name = match string_of(&body, "name") {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    reply(
        state
            .tags
            .create_category(&name, officer_of(&org))
            .await
            .map(|row| (axum::http::StatusCode::CREATED, Json(row))),
    )
}

async fn patch_tag_category(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
    Json(body): Json<Body>,
) -> Response {
    let name = match string_of(&body, "name") {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    reply(
        state
            .tags
            .patch_category(id, &name, officer_of(&org))
            .await
            .map(Json),
    )
}

async fn delete_tag_category(
    State(state): State<BlogAdminState>,
    Path(id): Path<Uuid>,
    org: OrgContext,
) -> Response {
    reply(
        state
            .tags
            .delete_category(id, officer_of(&org))
            .await
            .map(|_| axum::http::StatusCode::NO_CONTENT),
    )
}

// ── tiny JSON arms (the typed parse layer) ────────────────────────────────

fn string_of(body: &Body, key: &str) -> BlogResult<String> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| BlogError::InvalidInput(format!("{key} is required")))
}

fn opt_string_of(body: &Body, key: &str) -> Option<String> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn bool_of(body: &Body, key: &str) -> Option<bool> {
    body.get(key).and_then(|v| v.as_bool())
}

fn uuid_of(body: &Body, key: &str) -> Option<Uuid> {
    body.get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn uuid_vec_of(body: &Body, key: &str) -> Option<Vec<Uuid>> {
    body.get(key)?
        .as_array()?
        .iter()
        .map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .collect()
}

fn opt_ts(body: &Body, key: &str) -> BlogResult<Option<chrono::DateTime<chrono::Utc>>> {
    if body.get(key).is_none() {
        return Ok(None);
    }
    body.get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .map(Some)
        .ok_or_else(|| BlogError::InvalidInput(format!("{key} must be an RFC 3339 timestamp")))
}

fn reply<T: IntoResponse>(result: BlogResult<T>) -> Response {
    match result {
        Ok(payload) => payload.into_response(),
        Err(e) => e.into_response(),
    }
}
