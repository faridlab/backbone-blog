use super::AuditMetadata;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Strongly-typed ID for Post
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PostId(pub Uuid);

impl PostId {
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }
    pub fn into_inner(self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for PostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PostId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PostId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl From<PostId> for Uuid {
    fn from(id: PostId) -> Self {
        id.0
    }
}

impl AsRef<Uuid> for PostId {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}

impl std::ops::Deref for PostId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Post {
    pub id: Uuid,
    pub company_id: Uuid,
    pub blog_id: Uuid,
    pub website_id: Uuid,
    pub title: String,
    pub slug: String,
    pub content: Option<String>,
    pub teaser: Option<String>,
    pub cover: Option<serde_json::Value>,
    pub author_officer: Option<Uuid>,
    pub author_name: Option<String>,
    pub is_published: bool,
    pub published_date: Option<DateTime<Utc>>,
    pub post_date: DateTime<Utc>,
    pub visits: i32,
    pub allow_comments: bool,
    pub archived_at: Option<DateTime<Utc>>,
    pub archived_by_blog_id: Option<Uuid>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl Post {
    /// Create a builder for Post
    pub fn builder() -> PostBuilder {
        <PostBuilder as Default>::default()
    }

    /// Create a new Post with required fields
    pub fn new(
        company_id: Uuid,
        blog_id: Uuid,
        website_id: Uuid,
        title: String,
        slug: String,
        is_published: bool,
        post_date: DateTime<Utc>,
        visits: i32,
        allow_comments: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            blog_id,
            website_id,
            title,
            slug,
            content: None,
            teaser: None,
            cover: None,
            author_officer: None,
            author_name: None,
            is_published,
            published_date: None,
            post_date,
            visits,
            allow_comments,
            archived_at: None,
            archived_by_blog_id: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PostId {
        PostId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }

    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the content field (chainable)
    pub fn with_content(mut self, value: String) -> Self {
        self.content = Some(value);
        self
    }

    /// Set the teaser field (chainable)
    pub fn with_teaser(mut self, value: String) -> Self {
        self.teaser = Some(value);
        self
    }

    /// Set the cover field (chainable)
    pub fn with_cover(mut self, value: serde_json::Value) -> Self {
        self.cover = Some(value);
        self
    }

    /// Set the author_officer field (chainable)
    pub fn with_author_officer(mut self, value: Uuid) -> Self {
        self.author_officer = Some(value);
        self
    }

    /// Set the author_name field (chainable)
    pub fn with_author_name(mut self, value: String) -> Self {
        self.author_name = Some(value);
        self
    }

    /// Set the published_date field (chainable)
    pub fn with_published_date(mut self, value: DateTime<Utc>) -> Self {
        self.published_date = Some(value);
        self
    }

    /// Set the archived_at field (chainable)
    pub fn with_archived_at(mut self, value: DateTime<Utc>) -> Self {
        self.archived_at = Some(value);
        self
    }

    /// Set the archived_by_blog_id field (chainable)
    pub fn with_archived_by_blog_id(mut self, value: Uuid) -> Self {
        self.archived_by_blog_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.company_id = v;
                    }
                }
                "blog_id" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.blog_id = v;
                    }
                }
                "website_id" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.website_id = v;
                    }
                }
                "title" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.title = v;
                    }
                }
                "slug" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.slug = v;
                    }
                }
                "content" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.content = v;
                    }
                }
                "teaser" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.teaser = v;
                    }
                }
                "cover" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.cover = v;
                    }
                }
                "author_officer" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.author_officer = v;
                    }
                }
                "author_name" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.author_name = v;
                    }
                }
                "is_published" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.is_published = v;
                    }
                }
                "published_date" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.published_date = v;
                    }
                }
                "post_date" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.post_date = v;
                    }
                }
                "visits" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.visits = v;
                    }
                }
                "allow_comments" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.allow_comments = v;
                    }
                }
                "archived_at" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.archived_at = v;
                    }
                }
                "archived_by_blog_id" => {
                    if let Ok(v) = serde_json::from_value(value) {
                        self.archived_by_blog_id = v;
                    }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for Post {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "Post"
    }
}

impl backbone_core::PersistentEntity for Post {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for Post {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("blog_id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("archived_by_blog_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["title", "slug"]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("blog", "blogs", "blogId")]
    }
}

/// Builder for Post entity
///
/// Provides a fluent API for constructing Post instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PostBuilder {
    company_id: Option<Uuid>,
    blog_id: Option<Uuid>,
    website_id: Option<Uuid>,
    title: Option<String>,
    slug: Option<String>,
    content: Option<String>,
    teaser: Option<String>,
    cover: Option<serde_json::Value>,
    author_officer: Option<Uuid>,
    author_name: Option<String>,
    is_published: Option<bool>,
    published_date: Option<DateTime<Utc>>,
    post_date: Option<DateTime<Utc>>,
    visits: Option<i32>,
    allow_comments: Option<bool>,
    archived_at: Option<DateTime<Utc>>,
    archived_by_blog_id: Option<Uuid>,
}

impl PostBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the blog_id field (required)
    pub fn blog_id(mut self, value: Uuid) -> Self {
        self.blog_id = Some(value);
        self
    }

    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the title field (required)
    pub fn title(mut self, value: String) -> Self {
        self.title = Some(value);
        self
    }

    /// Set the slug field (required)
    pub fn slug(mut self, value: String) -> Self {
        self.slug = Some(value);
        self
    }

    /// Set the content field (optional)
    pub fn content(mut self, value: String) -> Self {
        self.content = Some(value);
        self
    }

    /// Set the teaser field (optional)
    pub fn teaser(mut self, value: String) -> Self {
        self.teaser = Some(value);
        self
    }

    /// Set the cover field (optional)
    pub fn cover(mut self, value: serde_json::Value) -> Self {
        self.cover = Some(value);
        self
    }

    /// Set the author_officer field (optional)
    pub fn author_officer(mut self, value: Uuid) -> Self {
        self.author_officer = Some(value);
        self
    }

    /// Set the author_name field (optional)
    pub fn author_name(mut self, value: String) -> Self {
        self.author_name = Some(value);
        self
    }

    /// Set the is_published field (default: `false`)
    pub fn is_published(mut self, value: bool) -> Self {
        self.is_published = Some(value);
        self
    }

    /// Set the published_date field (optional)
    pub fn published_date(mut self, value: DateTime<Utc>) -> Self {
        self.published_date = Some(value);
        self
    }

    /// Set the post_date field (default: `Utc::now()`)
    pub fn post_date(mut self, value: DateTime<Utc>) -> Self {
        self.post_date = Some(value);
        self
    }

    /// Set the visits field (default: `0`)
    pub fn visits(mut self, value: i32) -> Self {
        self.visits = Some(value);
        self
    }

    /// Set the allow_comments field (default: `false`)
    pub fn allow_comments(mut self, value: bool) -> Self {
        self.allow_comments = Some(value);
        self
    }

    /// Set the archived_at field (optional)
    pub fn archived_at(mut self, value: DateTime<Utc>) -> Self {
        self.archived_at = Some(value);
        self
    }

    /// Set the archived_by_blog_id field (optional)
    pub fn archived_by_blog_id(mut self, value: Uuid) -> Self {
        self.archived_by_blog_id = Some(value);
        self
    }

    /// Build the Post entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<Post, String> {
        let company_id = self
            .company_id
            .ok_or_else(|| "company_id is required".to_string())?;
        let blog_id = self
            .blog_id
            .ok_or_else(|| "blog_id is required".to_string())?;
        let website_id = self
            .website_id
            .ok_or_else(|| "website_id is required".to_string())?;
        let title = self.title.ok_or_else(|| "title is required".to_string())?;
        let slug = self.slug.ok_or_else(|| "slug is required".to_string())?;

        Ok(Post {
            id: Uuid::new_v4(),
            company_id,
            blog_id,
            website_id,
            title,
            slug,
            content: self.content,
            teaser: self.teaser,
            cover: self.cover,
            author_officer: self.author_officer,
            author_name: self.author_name,
            is_published: self.is_published.unwrap_or(false),
            published_date: self.published_date,
            post_date: self.post_date.unwrap_or(Utc::now()),
            visits: self.visits.unwrap_or(0),
            allow_comments: self.allow_comments.unwrap_or(false),
            archived_at: self.archived_at,
            archived_by_blog_id: self.archived_by_blog_id,
            metadata: AuditMetadata::default(),
        })
    }
}
