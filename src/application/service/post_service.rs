//! The post verbs (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The generated CRUD alias first, then the verb layer:
//!
//! - create / the typed PATCH whitelist — the FENCE is here:
//!   `is_published`/`published_date` in a patch body is a typed
//!   `blog_field_not_patchable` (+ audit `publish_refused`), and the
//!   repository's patch input has no arm that could write the pair
//!   even if a future caller forgot the check;
//! - publish — the ONE coupling transaction (SPEC section 4.1):
//!   guarded FOR UPDATE flip + the CASE stamp that PRESERVES a
//!   pre-set future `published_date` + ONE audit row (the row IS the
//!   publish event) + the notifier port fired INSIDE the transaction
//!   (a refusal WARNs + audits `notify_parked`; the verb still
//!   commits — publishing is not deliverable-contingent);
//! - unpublish (flip false, stamp retained);
//! - the one-way archive pair (forced unpublish embedded; unarchive
//!   NEVER re-publishes);
//! - the tag set verb (resolved ids only).

use serde_json::json;
use uuid::Uuid;

use crate::domain::entity::Post;
use crate::infrastructure::persistence::blog_command_repository::record_audit;
use crate::infrastructure::persistence::post_command_repository::{
    CreatePostInput, PatchPostInput, PostCommandRepository, PostRow,
};
use crate::infrastructure::persistence::tag_command_repository::normalize_slug;
use crate::presentation::dto::{CreatePostDto, UpdatePostDto};
use backbone_core::GenericCrudService;

/// Generated CRUD alias (keeps lib.rs's wiring compiling).
pub type PostService = GenericCrudService<
    Post,
    CreatePostDto,
    UpdatePostDto,
    crate::infrastructure::persistence::PostRepository,
>;

use super::blog_error::{BlogError, BlogResult};
use super::notifier_port::BlogPublishNotifier;

/// The publication-fence pair: fields a generic PATCH may NEVER
/// write. `publish`/`unpublish` are the only writers.
pub const PUBLISH_FENCED_FIELDS: [&str; 2] = ["is_published", "published_date"];

/// Fields refused at PATCH for structural reasons (no arm exists; the
/// refusal is typed for honest callers).
pub const REFUSED_FIELDS: [&str; 3] = ["blog_id", "website_id", "visits"];

/// The outcome of the publish verb.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublishResult {
    pub row: PostRow,
    /// false = already published (no-op success: no stamp, no audit
    /// re-emit, no notify — D6).
    pub changed: bool,
}

/// The post verb service.
pub struct PostCommandService {
    posts: PostCommandRepository,
    notifier: std::sync::Arc<dyn BlogPublishNotifier>,
}

impl PostCommandService {
    pub fn new(
        posts: PostCommandRepository,
        notifier: std::sync::Arc<dyn BlogPublishNotifier>,
    ) -> Self {
        Self { posts, notifier }
    }

    /// Create a post (title required; slug normalized, non-empty).
    pub async fn create(
        &self,
        input: &CreatePostInput,
        actor: Option<Uuid>,
    ) -> BlogResult<PostRow> {
        let title = input.title.trim();
        if title.is_empty() {
            return Err(BlogError::InvalidInput("title is required".to_string()));
        }
        let slug = normalize_slug(&input.slug);
        if slug.is_empty() {
            return Err(BlogError::InvalidInput(
                "slug normalizes to nothing".to_string(),
            ));
        }
        let mut cleaned = input.clone();
        cleaned.title = title.to_string();
        cleaned.slug = slug;
        self.posts.create(&cleaned, actor).await
    }

    /// The typed patch: the fence is checked at the route AND
    /// structurally here (the input has no pair arm). Returns the row
    /// and the PRIOR slug when it changed — the caller records the
    /// stale-slug redirect through the website seam after the verb
    /// commits (ONE call; nothing recorded when the slug did not
    /// move).
    pub async fn patch(
        &self,
        id: Uuid,
        patch: &PatchPostInput,
        actor: Option<Uuid>,
    ) -> BlogResult<(PostRow, Option<String>)> {
        if let Some(slug) = &patch.slug {
            let normalized = normalize_slug(slug);
            if normalized.is_empty() {
                return Err(BlogError::InvalidInput(
                    "slug normalizes to nothing".to_string(),
                ));
            }
        }
        let (row, slug_was) = self.posts.patch(id, patch, actor).await?;
        if let Some(from) = &slug_was {
            tracing::info!(
                post = row.id.to_string(),
                from,
                to = row.slug.as_str(),
                "post slug changed"
            );
        }
        Ok((row, slug_was))
    }

    /// THE publish coupling verb — ONE transaction (SPEC section 4.1):
    /// flip + stamp + ONE audit row + notifier, in order. A refused
    /// notify WARNs + audits `notify_parked` and the verb STILL
    /// commits.
    pub async fn publish(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PublishResult> {
        let mut tx = self.posts.begin().await?;
        if let Some(scope) = backbone_orm::org_scope::current_org_scope() {
            backbone_orm::org_scope::bind_org_scope_on(&mut *tx, &scope).await?;
        }
        let outcome = self.posts.publish_flip(&mut tx, id, actor).await?;
        if outcome.changed {
            match self
                .notifier
                .post_published(
                    outcome.row.blog_id,
                    outcome.row.id,
                    &outcome.row.title,
                    actor,
                )
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    // The park: the audit trail is the retry surface.
                    tracing::warn!(
                        post = id.to_string(),
                        error = e.code(),
                        "publish notify parked"
                    );
                    record_audit(
                        &mut tx,
                        "notify_parked",
                        actor,
                        "post",
                        Some(id),
                        json!({ "reason": e.code() }),
                    )
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(PublishResult {
            row: outcome.row,
            changed: outcome.changed,
        })
    }

    /// The unpublish fence verb (flip false, stamp retained as
    /// history; idempotent no-op when already false).
    pub async fn unpublish(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        self.posts.unpublish(id, actor).await
    }

    /// The one-way archive pair.
    pub async fn archive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        self.posts.archive(id, actor).await
    }

    pub async fn unarchive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<PostRow> {
        self.posts.unarchive(id, actor).await
    }

    /// Set the post's tag list: tag ids are resolved under the
    /// ambient scope by the caller (D8: lookup, never trust); an
    /// unknown id is the uniform 404.
    pub async fn set_tags(
        &self,
        post_id: Uuid,
        tag_ids: &[Uuid],
        actor: Option<Uuid>,
    ) -> BlogResult<Vec<Uuid>> {
        self.posts.set_tags(post_id, tag_ids, actor).await
    }

    /// Read one post (admin).
    pub async fn get(&self, id: Uuid) -> BlogResult<PostRow> {
        self.posts.get(id).await
    }

    /// Record the fence refusal's audit fact (the route calls this
    /// BEFORE answering the typed 422 — a refused write is a durable
    /// fact).
    pub async fn audit_fence_refusal(
        &self,
        id: Uuid,
        fields: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<()> {
        self.posts.audit_fence_refusal(id, fields, actor).await
    }
}
