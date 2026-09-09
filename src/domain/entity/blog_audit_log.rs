use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::BlogAuditEvent;

/// Strongly-typed ID for BlogAuditLog
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlogAuditLogId(pub Uuid);

impl BlogAuditLogId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for BlogAuditLogId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for BlogAuditLogId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for BlogAuditLogId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<BlogAuditLogId> for Uuid {
    fn from(id: BlogAuditLogId) -> Self { id.0 }
}

impl AsRef<Uuid> for BlogAuditLogId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for BlogAuditLogId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BlogAuditLog {
    pub id: Uuid,
    pub event: BlogAuditEvent,
    pub actor: Option<Uuid>,
    pub subject_type: Option<String>,
    pub subject_id: Option<Uuid>,
    pub detail: Option<serde_json::Value>,
    pub occurred_at: DateTime<Utc>,
}

impl BlogAuditLog {
    /// Create a builder for BlogAuditLog
    pub fn builder() -> BlogAuditLogBuilder {
        <BlogAuditLogBuilder as Default>::default()
    }

    /// Create a new BlogAuditLog with required fields
    pub fn new(event: BlogAuditEvent, occurred_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            event,
            actor: None,
            subject_type: None,
            subject_id: None,
            detail: None,
            occurred_at,
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> BlogAuditLogId {
        BlogAuditLogId(self.id)
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the actor field (chainable)
    pub fn with_actor(mut self, value: Uuid) -> Self {
        self.actor = Some(value);
        self
    }

    /// Set the subject_type field (chainable)
    pub fn with_subject_type(mut self, value: String) -> Self {
        self.subject_type = Some(value);
        self
    }

    /// Set the subject_id field (chainable)
    pub fn with_subject_id(mut self, value: Uuid) -> Self {
        self.subject_id = Some(value);
        self
    }

    /// Set the detail field (chainable)
    pub fn with_detail(mut self, value: serde_json::Value) -> Self {
        self.detail = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "event" => {
                    if let Ok(v) = serde_json::from_value(value) { self.event = v; }
                }
                "actor" => {
                    if let Ok(v) = serde_json::from_value(value) { self.actor = v; }
                }
                "subject_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.subject_type = v; }
                }
                "subject_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.subject_id = v; }
                }
                "detail" => {
                    if let Ok(v) = serde_json::from_value(value) { self.detail = v; }
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

impl super::Entity for BlogAuditLog {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "BlogAuditLog"
    }
}

impl backbone_core::PersistentEntity for BlogAuditLog {
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

impl backbone_orm::EntityRepoMeta for BlogAuditLog {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("subject_id".to_string(), "uuid".to_string());
        m.insert("event".to_string(), "blog_audit_event".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for BlogAuditLog entity
///
/// Provides a fluent API for constructing BlogAuditLog instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct BlogAuditLogBuilder {
    event: Option<BlogAuditEvent>,
    actor: Option<Uuid>,
    subject_type: Option<String>,
    subject_id: Option<Uuid>,
    detail: Option<serde_json::Value>,
    occurred_at: Option<DateTime<Utc>>,
}

impl BlogAuditLogBuilder {
    /// Set the event field (required)
    pub fn event(mut self, value: BlogAuditEvent) -> Self {
        self.event = Some(value);
        self
    }

    /// Set the actor field (optional)
    pub fn actor(mut self, value: Uuid) -> Self {
        self.actor = Some(value);
        self
    }

    /// Set the subject_type field (optional)
    pub fn subject_type(mut self, value: String) -> Self {
        self.subject_type = Some(value);
        self
    }

    /// Set the subject_id field (optional)
    pub fn subject_id(mut self, value: Uuid) -> Self {
        self.subject_id = Some(value);
        self
    }

    /// Set the detail field (optional)
    pub fn detail(mut self, value: serde_json::Value) -> Self {
        self.detail = Some(value);
        self
    }

    /// Set the occurred_at field (default: `Utc::now()`)
    pub fn occurred_at(mut self, value: DateTime<Utc>) -> Self {
        self.occurred_at = Some(value);
        self
    }

    /// Build the BlogAuditLog entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<BlogAuditLog, String> {
        let event = self.event.ok_or_else(|| "event is required".to_string())?;

        Ok(BlogAuditLog {
            id: Uuid::new_v4(),
            event,
            actor: self.actor,
            subject_type: self.subject_type,
            subject_id: self.subject_id,
            detail: self.detail,
            occurred_at: self.occurred_at.unwrap_or(Utc::now()),
        })
    }
}
