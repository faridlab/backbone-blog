// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// The hand repositories (user-owned — see metaphor.codegen.yaml): the
// transactional SQL the verb services compose. Services hold no raw
// sqlx; every transactional method relays the ambient org scope onto
// its transaction first (the composing service's fence, when one is
// resolved).
pub mod blog_command_repository;
pub mod post_command_repository;
pub mod public_query_repository;
// Shared read transport for the module's own tables: ambient-scope-bound transactions
// (the composing service's fence, when one is resolved).
pub mod scoped_read;
pub mod tag_command_repository;
pub mod visit_command_repository;
pub use blog_command_repository::{
    BlogCommandRepository, BlogRow, CreateBlogInput, PatchBlogInput,
};
pub use post_command_repository::{
    CreatePostInput, PatchPostInput, PostCommandRepository, PostRow, PublishOutcome,
};
pub use public_query_repository::{
    AdminState, CloudEntry, ListingItem, ListingOrder, ListingPage, ListingQuery, PublicBlog,
    PublicPostDetail, PublicQueryRepository, StateCounts, TagLite, PAGE_SIZE,
};
pub use tag_command_repository::{normalize_slug, TagCategoryRow, TagCommandRepository, TagRow};
pub use visit_command_repository::{VisitCommandRepository, VisitOutcome, RECEIPT_RETENTION_DAYS};
