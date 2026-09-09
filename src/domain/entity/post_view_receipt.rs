use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Strongly-typed ID for PostViewReceipt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PostViewReceiptId(pub Uuid);

impl PostViewReceiptId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PostViewReceiptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PostViewReceiptId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PostViewReceiptId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PostViewReceiptId> for Uuid {
    fn from(id: PostViewReceiptId) -> Self { id.0 }
}

impl AsRef<Uuid> for PostViewReceiptId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PostViewReceiptId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PostViewReceipt {
    pub id: Uuid,
    pub post_id: Uuid,
    pub view_token: String,
    pub window_start: DateTime<Utc>,
    pub occurred_at: DateTime<Utc>,
}

impl PostViewReceipt {
    /// Create a builder for PostViewReceipt
    pub fn builder() -> PostViewReceiptBuilder {
        <PostViewReceiptBuilder as Default>::default()
    }

    /// Create a new PostViewReceipt with required fields
    pub fn new(post_id: Uuid, view_token: String, window_start: DateTime<Utc>, occurred_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            post_id,
            view_token,
            window_start,
            occurred_at,
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PostViewReceiptId {
        PostViewReceiptId(self.id)
    }


    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "post_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.post_id = v; }
                }
                "view_token" => {
                    if let Ok(v) = serde_json::from_value(value) { self.view_token = v; }
                }
                "window_start" => {
                    if let Ok(v) = serde_json::from_value(value) { self.window_start = v; }
                }
                "occurred_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.occurred_at = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PostViewReceipt {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PostViewReceipt"
    }
}

impl backbone_core::PersistentEntity for PostViewReceipt {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        let _ = ts;
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        let _ = ts;
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        let _ = ts;
    }
}

impl backbone_orm::EntityRepoMeta for PostViewReceipt {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("post_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["view_token"]
    }
}

/// Builder for PostViewReceipt entity
///
/// Provides a fluent API for constructing PostViewReceipt instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PostViewReceiptBuilder {
    post_id: Option<Uuid>,
    view_token: Option<String>,
    window_start: Option<DateTime<Utc>>,
    occurred_at: Option<DateTime<Utc>>,
}

impl PostViewReceiptBuilder {
    /// Set the post_id field (required)
    pub fn post_id(mut self, value: Uuid) -> Self {
        self.post_id = Some(value);
        self
    }

    /// Set the view_token field (required)
    pub fn view_token(mut self, value: String) -> Self {
        self.view_token = Some(value);
        self
    }

    /// Set the window_start field (required)
    pub fn window_start(mut self, value: DateTime<Utc>) -> Self {
        self.window_start = Some(value);
        self
    }

    /// Set the occurred_at field (default: `Utc::now()`)
    pub fn occurred_at(mut self, value: DateTime<Utc>) -> Self {
        self.occurred_at = Some(value);
        self
    }

    /// Build the PostViewReceipt entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PostViewReceipt, String> {
        let post_id = self.post_id.ok_or_else(|| "post_id is required".to_string())?;
        let view_token = self.view_token.ok_or_else(|| "view_token is required".to_string())?;
        let window_start = self.window_start.ok_or_else(|| "window_start is required".to_string())?;

        Ok(PostViewReceipt {
            id: Uuid::new_v4(),
            post_id,
            view_token,
            window_start,
            occurred_at: self.occurred_at.unwrap_or(Utc::now()),
        })
    }
}
