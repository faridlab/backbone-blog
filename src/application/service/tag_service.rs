//! The tag verbs (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The generated CRUD alias first (the generator declares this module
//! but emits no file for this model), then the verb layer: create /
//! rename / delete, and the tag-category master verbs. The rename
//! returns the prior slug — the CALLER (the route, which holds the
//! website surface) records the stale-slug redirect through WB-3's
//! seam after the verb commits; the service owns the loop-guard facts
//! (`from != to` is "prior slug differs").
//!
//! The vocabulary grain is the composing deployment's tenancy unit
//! (ADR-0029): the module ships no uniqueness wall of its own — the
//! composing service's decorator installs the per-unit twins where a
//! deployment declares them, and this layer maps their collisions to
//! the typed 409s when they reuse the historical wall names.

use uuid::Uuid;

use crate::domain::entity::Tag;
use crate::infrastructure::persistence::tag_command_repository::{
    TagCategoryRow, TagCommandRepository, TagRow,
};
use crate::presentation::dto::{CreateTagDto, UpdateTagDto};
use backbone_core::GenericCrudService;

/// Generated CRUD alias (keeps lib.rs's wiring compiling).
pub type TagService = GenericCrudService<
    Tag,
    CreateTagDto,
    UpdateTagDto,
    crate::infrastructure::persistence::TagRepository,
>;

use super::blog_error::{BlogError, BlogResult};

/// The tag verb service.
pub struct TagCommandService {
    tags: TagCommandRepository,
}

impl TagCommandService {
    pub fn new(tags: TagCommandRepository) -> Self {
        Self { tags }
    }

    /// Create a tag (name required, non-blank; slug derived).
    pub async fn create(
        &self,
        name: &str,
        category_id: Option<Uuid>,
        actor: Option<Uuid>,
    ) -> BlogResult<TagRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(BlogError::InvalidInput("name is required".to_string()));
        }
        self.tags.create(name, category_id, actor).await
    }

    /// Rename / re-categorize. Returns the row and the PRIOR slug when
    /// it changed — the caller records the stale-slug redirect through
    /// the website seam (ONE call; loop-guard: nothing recorded when
    /// the slug did not move).
    pub async fn rename(
        &self,
        id: Uuid,
        name: Option<&str>,
        category_id: Option<Option<Uuid>>,
        actor: Option<Uuid>,
    ) -> BlogResult<(TagRow, Option<String>)> {
        if let Some(name) = name {
            if name.trim().is_empty() {
                return Err(BlogError::InvalidInput("name cannot be blank".to_string()));
            }
        }
        let trimmed = name.map(str::trim);
        self.tags.rename(id, trimmed, category_id, actor).await
    }

    /// Delete a tag: untag everywhere in one statement, then remove
    /// the row. Returns how many post links were shed.
    pub async fn delete(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<i64> {
        self.tags.delete(id, actor).await
    }

    /// The tag vocabulary (admin).
    pub async fn list(&self) -> BlogResult<Vec<TagRow>> {
        self.tags.list(1000).await
    }

    /// Resolve one slug (scoped lookup, D8).
    pub async fn resolve_by_slug(&self, slug: &str) -> BlogResult<TagRow> {
        self.tags.resolve_by_slug(slug).await
    }

    // ── tag categories (master data) ─────────────────────────────────

    pub async fn list_categories(&self) -> BlogResult<Vec<TagCategoryRow>> {
        self.tags.list_categories(500).await
    }

    pub async fn create_category(
        &self,
        name: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<TagCategoryRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(BlogError::InvalidInput("name is required".to_string()));
        }
        self.tags.create_category(name, actor).await
    }

    pub async fn patch_category(
        &self,
        id: Uuid,
        name: &str,
        actor: Option<Uuid>,
    ) -> BlogResult<TagCategoryRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(BlogError::InvalidInput("name cannot be blank".to_string()));
        }
        self.tags.patch_category(id, name, actor).await
    }

    /// Delete a category; a category still grouping tags is the typed
    /// refusal (surfaced as 422 with a reason, not a raw FK 500).
    pub async fn delete_category(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<()> {
        self.tags.delete_category(id, actor).await
    }
}
