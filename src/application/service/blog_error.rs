//! The module's typed error surface (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — ONE enum, stable wire codes, private
//! status mapping, one `IntoResponse`.
//!
//! Wire shape: `{"error":{"code":"blog_...","message":"..."}}` (+
//! `Retry-After` on the throttle). Internal shapes never leak text:
//! the database/internal arms log via tracing and answer a generic
//! body — the detail rides the log, never the response.
//!
//! Status classes (SPEC section 11):
//!  - 404 not-found family — `blog_not_found` is THE uniform content
//!    answer (unknown blog/post/slug/tag, wrong blog, unpublished,
//!    future-dated public, archived, cross-tenant — one answer, no
//!    oracle); `blog_website_not_found` is the loud host-binding
//!    miss (no fallback to any first website).
//!  - 422 domain refusals/validation.
//!  - 429 throttle (+ Retry-After).
//!  - 503 unconfigured capability secret (fail-closed).
//!  - 409 busy/conflict (the unique walls, the RESTRICT delete).
//!  - 500 infra (generic body).

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// The module's result alias.
pub type BlogResult<T> = Result<T, BlogError>;

/// ONE typed error enum for the whole module.
#[derive(Debug, thiserror::Error)]
pub enum BlogError {
    /// THE uniform content miss — no oracle, one answer.
    #[error("not found")]
    NotFound,

    /// The host binding missed (loud, no fallback).
    #[error("website not found for host")]
    WebsiteNotFound,

    /// Malformed body/dates/tag list; also the bad/expired view-token
    /// signature (constant-time verify, uniform message — never say
    /// which arm refused).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A PATCH carried the fence pair (`is_published`/`published_date`)
    /// or another field refused at this version.
    #[error("field not patchable: {0}")]
    FieldNotPatchable(String),

    /// The visit windows are exhausted.
    #[error("throttled")]
    Throttled {
        /// The fixed-window length, surfaced as `retry_after_secs`.
        retry_after_secs: u32,
    },

    /// Mint/verify under an empty `BLOG_CAPABILITY_SECRET` —
    /// fail-closed, never a mint under "".
    #[error("capability secret not configured")]
    CapabilitySecretNotConfigured,

    /// The `(blog_id, slug)` wall (posts) or a tag slug wall.
    #[error("slug already taken")]
    SlugTaken,

    /// A tag name wall (the module ships none; a composing service's
    /// decorator-installed twin surfaces here when it reuses the
    /// historical wall name).
    #[error("tag name already taken")]
    TagNameTaken,

    /// Blog delete with any post (the RESTRICT FK, mapped).
    #[error("blog has posts")]
    BlogHasPosts,

    /// Infrastructure failure (detail logged, never leaked).
    #[error("database error")]
    Database(String),

    /// Unexpected internal failure (detail logged, never leaked).
    #[error("internal error")]
    Internal(String),
}

impl BlogError {
    /// The stable wire code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "blog_not_found",
            Self::WebsiteNotFound => "blog_website_not_found",
            Self::InvalidInput(_) => "blog_invalid_input",
            Self::FieldNotPatchable(_) => "blog_field_not_patchable",
            Self::Throttled { .. } => "blog_throttled",
            Self::CapabilitySecretNotConfigured => "blog_capability_secret_not_configured",
            Self::SlugTaken => "blog_slug_taken",
            Self::TagNameTaken => "blog_tag_name_taken",
            Self::BlogHasPosts => "blog_blog_has_posts",
            Self::Database(_) => "blog_database",
            Self::Internal(_) => "blog_internal",
        }
    }

    /// The HTTP status (private — the wire carries the code).
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound | Self::WebsiteNotFound => StatusCode::NOT_FOUND,
            Self::InvalidInput(_) | Self::FieldNotPatchable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Throttled { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::CapabilitySecretNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            Self::SlugTaken | Self::TagNameTaken | Self::BlogHasPosts => StatusCode::CONFLICT,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The public message (uniform where the spec demands an oracle
    /// be absent).
    fn message(&self) -> String {
        match self {
            Self::NotFound => "not found".to_string(),
            Self::WebsiteNotFound => "no website bound to this host".to_string(),
            Self::InvalidInput(detail) => detail.clone(),
            Self::FieldNotPatchable(field) => format!("field not patchable: {field}"),
            Self::Throttled { retry_after_secs } => {
                format!("throttled; retry after {retry_after_secs}s")
            }
            Self::CapabilitySecretNotConfigured => "capability secret not configured".to_string(),
            Self::SlugTaken => "slug already taken".to_string(),
            Self::TagNameTaken => "tag name already taken".to_string(),
            Self::BlogHasPosts => "blog still has posts".to_string(),
            // Never leak infra detail.
            Self::Database(_) => "database error".to_string(),
            Self::Internal(_) => "internal error".to_string(),
        }
    }
}

impl IntoResponse for BlogError {
    fn into_response(self) -> Response {
        let body = json!({"error": {"code": self.code(), "message": self.message()}});
        let mut response = (self.status(), Json(body)).into_response();
        if let Self::Throttled { retry_after_secs } = &self {
            if let Ok(value) = retry_after_secs.to_string().parse() {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        // The infra detail rides the log, never the response.
        if let Self::Database(detail) | Self::Internal(detail) = &self {
            tracing::error!(
                code = self.code(),
                detail = detail.as_str(),
                "blog infra failure"
            );
        }
        response
    }
}

impl From<sqlx::Error> for BlogError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<anyhow::Error> for BlogError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

/// Map a unique-violation error onto the typed 409s by constraint
/// name (the H4/H5/H6 walls; `23505`). The module itself ships no tag
/// name/slug wall since the tenancy strip — a composing service's
/// decorator-installed per-unit twins answer the typed 409s when they
/// reuse the historical wall names, and fall through to the generic
/// duplicate mapping otherwise.
pub fn map_unique_violation(err: sqlx::Error) -> BlogError {
    if let sqlx::Error::Database(db) = &err {
        if db.code().as_deref() == Some("23505") {
            let detail = db.constraint().unwrap_or_default().to_string();
            return match detail.as_str() {
                "uq_tags_company_name" => BlogError::TagNameTaken,
                "uq_tags_company_slug" | "uq_posts_blog_slug" => BlogError::SlugTaken,
                other => BlogError::InvalidInput(format!("duplicate: {other}")),
            };
        }
        if db.code().as_deref() == Some("23503") {
            // FK refusal: only the blog-delete RESTRICT wall (posts
            // referencing the blog) maps to the typed 409; every other
            // FK refusal stays a Database error (never mislabeled).
            return match db.constraint().unwrap_or_default() {
                "fk_posts_blog_id" => BlogError::BlogHasPosts,
                _ => BlogError::Database(err.to_string()),
            };
        }
    }
    BlogError::Database(err.to_string())
}
