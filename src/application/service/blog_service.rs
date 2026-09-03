//! The blog verbs (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The generated CRUD alias first (the generator declares this module
//! but emits no file for this model), then the verb layer: create /
//! typed patch (website_id refused while posts exist) / the archive
//! cascade verbs (single-UPDATE marker cascade + marker restore) /
//! the guarded empty-blog delete (SPEC sections 4.2, 9.2).

use uuid::Uuid;

use crate::domain::entity::Blog;
use crate::infrastructure::persistence::blog_command_repository::{
    BlogCommandRepository, BlogRow, CreateBlogInput, PatchBlogInput,
};
use crate::presentation::dto::{CreateBlogDto, UpdateBlogDto};
use backbone_core::GenericCrudService;

/// Generated CRUD alias (keeps lib.rs's wiring compiling).
pub type BlogService = GenericCrudService<
    Blog,
    CreateBlogDto,
    UpdateBlogDto,
    crate::infrastructure::persistence::BlogRepository,
>;

use super::blog_error::{BlogError, BlogResult};

/// The blog verb service.
pub struct BlogCommandService {
    blogs: BlogCommandRepository,
}

impl BlogCommandService {
    pub fn new(blogs: BlogCommandRepository) -> Self {
        Self { blogs }
    }

    /// Create a blog (name required, non-blank).
    pub async fn create(
        &self,
        input: &CreateBlogInput,
        actor: Option<Uuid>,
    ) -> BlogResult<BlogRow> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(BlogError::InvalidInput("name is required".to_string()));
        }
        let mut cleaned = input.clone();
        cleaned.name = name.to_string();
        self.blogs.create(&cleaned, actor).await
    }

    /// The typed patch. A `website_id` change is refused while any
    /// post exists (typed 422 — moving a loaded blog would silently
    /// re-scope its posts, the BL-1 latent behavior); an EMPTY blog
    /// may move (the H1a trigger keeps posts honest for any future
    /// writer).
    pub async fn patch(
        &self,
        id: Uuid,
        patch: &PatchBlogInput,
        website_move: Option<Uuid>,
        actor: Option<Uuid>,
    ) -> BlogResult<BlogRow> {
        if let Some(target) = website_move {
            if self.blogs.has_posts(id).await? {
                return Err(BlogError::FieldNotPatchable(
                    "website_id (blog has posts; moving would re-scope them)".to_string(),
                ));
            }
            return self.blogs.move_website(id, target, actor).await;
        }
        if let Some(name) = &patch.name {
            if name.trim().is_empty() {
                return Err(BlogError::InvalidInput("name cannot be blank".to_string()));
            }
        }
        self.blogs.patch(id, patch, actor).await
    }

    /// Archive: the single-UPDATE marker cascade + the blog row, one
    /// transaction. Returns (row, posts_cascade).
    pub async fn archive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<(BlogRow, i64)> {
        self.blogs.archive(id, actor).await
    }

    /// Unarchive: restores the blog row and exactly the marker rows
    /// (posts archived individually before or after stay archived);
    /// nothing re-publishes. Returns (row, posts_restored).
    pub async fn unarchive(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<(BlogRow, i64)> {
        self.blogs.unarchive(id, actor).await
    }

    /// The guarded empty-blog delete (any post → typed 409).
    pub async fn delete(&self, id: Uuid, actor: Option<Uuid>) -> BlogResult<()> {
        self.blogs.delete(id, actor).await
    }

    /// Read one blog (admin).
    pub async fn get(&self, id: Uuid) -> BlogResult<BlogRow> {
        self.blogs.get(id).await
    }

    /// The admin list (optionally website-scoped).
    pub async fn list(&self, website_id: Option<Uuid>) -> BlogResult<Vec<BlogRow>> {
        self.blogs.list(website_id, 500).await
    }
}
