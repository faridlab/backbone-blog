//! The module's PUBLIC route surface (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — SPEC section 10.
//!
//! The module DOES NOT SELF-MOUNT: it exports
//! [`blog_public_routes`], a plain `axum::Router` the composing host
//! nests BARE of any session guard under the schema name —
//! `Router::new().nest("/api/v1/blog", blog_public_routes(state))`.
//! The public-tier fence (the ambient org scope relay + the tier
//! GUC), the capability token, and the fixed-window throttle are the
//! wall; there is no session and no auth middleware here.
//!
//! The allowlist (exhaustive — the negative-enumeration probe's
//! target):
//! - `GET  /public/blogs`                              the site's blogs
//! - `GET  /public/blogs/{blog_slug}/posts`            the listing
//! - `GET  /public/blogs/{blog_slug}/posts/{post_slug}` the detail
//! - `GET  /public/blogs/{blog_slug}/tags`             the tag cloud
//! - `POST /public/posts/{post_slug}/visit`            the visit verb
//!
//! Host binding: every handler resolves the website from the `Host`
//! header through `WebsiteSurface` — no fallback; a miss is the typed
//! `blog_website_not_found`.
//!
//! The secret: `BLOG_CAPABILITY_SECRET` at compose; unset = the visit
//! verb answers the typed 503 `blog_capability_secret_not_configured`
//! (fail-closed — GETs are unaffected, they need no token).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use backbone_website::exports::WebsiteSurface;

use crate::application::service::blog_error::BlogError;
use crate::application::service::capability::{
    capability_secret_from_env, BLOG_CAPABILITY_SECRET_ENV,
};
use crate::application::service::public_query_service::{
    DetailAnswer, ListingAnswer, PublicQueryService,
};
use crate::application::service::visit_service::{VisitRequest, VisitService};

/// The trusted-proxy env var (see [`trusted_proxy_from_env`]).
pub const BLOG_TRUSTED_PROXY_ENV: &str = "BLOG_TRUSTED_PROXY";

/// The shared public state (cheap-to-clone service handles).
#[derive(Clone)]
pub struct BlogPublicState {
    public_queries: Arc<PublicQueryService>,
    visits: Arc<VisitService>,
    trusted_proxy: bool,
}

impl BlogPublicState {
    /// Compose over one pool + the host's website surface; the secret
    /// comes from [`BLOG_CAPABILITY_SECRET_ENV`] (empty = the typed
    /// 503 at the visit verb) and the trusted-proxy posture from
    /// [`BLOG_TRUSTED_PROXY_ENV`] (unset = direct connections — the
    /// forwarded header is client-controlled text and never read).
    pub fn compose(pool: sqlx::PgPool, surface: Arc<dyn WebsiteSurface>) -> Self {
        Self::with_secret_and_trusted_proxy(
            pool,
            surface,
            capability_secret_from_env(),
            trusted_proxy_from_env(),
        )
    }

    /// [`Self::compose`] with the secret explicit (the probe entry —
    /// tests must not depend on process environment other tests
    /// mutate) under the direct-connection posture.
    pub fn with_secret(
        pool: sqlx::PgPool,
        surface: Arc<dyn WebsiteSurface>,
        secret: String,
    ) -> Self {
        Self::with_secret_and_trusted_proxy(pool, surface, secret, false)
    }

    /// [`Self::with_secret`] with the trusted-proxy posture explicit.
    pub fn with_secret_and_trusted_proxy(
        pool: sqlx::PgPool,
        surface: Arc<dyn WebsiteSurface>,
        secret: String,
        trusted_proxy: bool,
    ) -> Self {
        Self {
            public_queries: Arc::new(PublicQueryService::new(pool.clone(), surface.clone())),
            visits: Arc::new(VisitService::new(pool, surface, secret)),
            trusted_proxy,
        }
    }

    /// Whether the compose secret is set (never the value itself).
    pub fn secret_is_configured(&self) -> bool {
        self.visits.secret_is_configured()
    }
}

/// THE PUBLIC TREE (the exhaustive allowlist — see the module doc).
pub fn blog_public_routes(state: BlogPublicState) -> Router {
    Router::new()
        .route("/public/blogs", get(blogs_handler))
        .route("/public/blogs/:blog_slug/posts", get(listing_handler))
        .route(
            "/public/blogs/:blog_slug/posts/:post_slug",
            get(detail_handler),
        )
        .route("/public/blogs/:blog_slug/tags", get(cloud_handler))
        .route("/public/posts/:post_slug/visit", post(visit_handler))
        .with_state(state)
}

/// The trusted-proxy posture (bool-tolerant, fail-closed): `true` /
/// `1` / `yes` / `on` (any case) arm it; unset or anything else keeps
/// the direct-connection posture.
pub fn trusted_proxy_from_env() -> bool {
    matches!(
        std::env::var(BLOG_TRUSTED_PROXY_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "true" | "1" | "yes" | "on"
    )
}

/// Resolve the caller address for the visit throttle (never
/// authorization): the RIGHTMOST forwarded hop ONLY under the
/// trusted-proxy posture (the entry the nearest trusted proxy
/// appended; every hop to its left is client-supplied text); every
/// hop is ignored otherwise and the connection's socket IP wins.
/// Falls back to the socket IP when a trusted chain emits no header,
/// and to `"unknown"` when no socket address is available. The socket
/// arm is the bare IP, never the `ip:port` pair — the port is
/// per-connection and would fragment a throttle bucket per reconnect.
pub fn caller_ip(headers: &HeaderMap, socket_ip: Option<&str>, trusted_proxy: bool) -> String {
    if trusted_proxy {
        if let Some(fwd) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(last) = fwd.rsplit(',').next() {
                let trimmed = last.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    socket_ip.unwrap_or("unknown").to_string()
}

/// The `Host` header (the website binding arm; a miss is the typed
/// `blog_website_not_found` — no fallback, ever).
fn host_of(headers: &HeaderMap) -> BlogResult<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .ok_or_else(|| BlogError::WebsiteNotFound)
}

type BlogResult<T> = Result<T, BlogError>;

#[derive(Debug, Deserialize)]
struct ListingParams {
    tag: Option<String>,
    page: Option<i64>,
    order: Option<String>,
    search: Option<String>,
}

async fn blogs_handler(State(state): State<BlogPublicState>, headers: HeaderMap) -> Response {
    let host = match host_of(&headers) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    match state.public_queries.blogs(&host).await {
        Ok(answer) => (
            StatusCode::OK,
            Json(serde_json::json!({ "blogs": answer.blogs })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn listing_handler(
    State(state): State<BlogPublicState>,
    Path(blog_slug): Path<String>,
    Query(params): Query<ListingParams>,
    headers: HeaderMap,
) -> Response {
    let host = match host_of(&headers) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let order = match params.order.as_deref() {
        Some("visits") => {
            crate::infrastructure::persistence::public_query_repository::ListingOrder::Visits
        }
        _ => crate::infrastructure::persistence::public_query_repository::ListingOrder::Recent,
    };
    match state
        .public_queries
        .listing(
            &host,
            &blog_slug,
            params.tag.as_deref(),
            params.page.unwrap_or(1),
            order,
            params.search.as_deref(),
        )
        .await
    {
        Ok(ListingAnswer::Page(page)) => (StatusCode::OK, Json(page)).into_response(),
        Ok(ListingAnswer::RedirectTo(location)) => {
            (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn detail_handler(
    State(state): State<BlogPublicState>,
    Path((blog_slug, post_slug)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let host = match host_of(&headers) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    match state
        .public_queries
        .detail(&host, &blog_slug, &post_slug)
        .await
    {
        Ok(DetailAnswer::Page(detail)) => (StatusCode::OK, Json(detail)).into_response(),
        Ok(DetailAnswer::Redirect { status, location }) => {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::MOVED_PERMANENTLY);
            match location {
                Some(location) => (code, [(header::LOCATION, location)]).into_response(),
                // A recorded `gone_404`: the resource is gone, by
                // record, not by guess.
                None => code.into_response(),
            }
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CloudParams {
    min_limit: Option<i64>,
}

async fn cloud_handler(
    State(state): State<BlogPublicState>,
    Path(blog_slug): Path<String>,
    Query(params): Query<CloudParams>,
    headers: HeaderMap,
) -> Response {
    let host = match host_of(&headers) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    match state
        .public_queries
        .cloud(&host, &blog_slug, params.min_limit.unwrap_or(1).max(1))
        .await
    {
        Ok(entries) => {
            (StatusCode::OK, Json(serde_json::json!({ "tags": entries }))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn visit_handler(
    State(state): State<BlogPublicState>,
    Path(post_slug): Path<String>,
    headers: HeaderMap,
    connect_info: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    body: Option<Json<VisitRequest>>,
) -> Response {
    let host = match host_of(&headers) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let ip = caller_ip(
        &headers,
        connect_info.map(|c| c.ip().to_string()).as_deref(),
        state.trusted_proxy,
    );
    let presented = body.and_then(|Json(req)| req.view_token);
    match state
        .visits
        .visit(&host, &post_slug, presented.as_deref(), &ip)
        .await
    {
        Ok(answer) => (StatusCode::OK, Json(answer)).into_response(),
        Err(e) => e.into_response(),
    }
}
