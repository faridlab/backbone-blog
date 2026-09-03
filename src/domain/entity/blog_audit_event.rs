use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "blog_audit_event", rename_all = "snake_case")]
pub enum BlogAuditEvent {
    BlogCreated,
    BlogUpdated,
    BlogArchived,
    BlogUnarchived,
    BlogDeleteRefused,
    PostCreated,
    PostUpdated,
    PublishRefused,
    PostPublished,
    NotifyParked,
    PostUnpublished,
    PostArchived,
    PostUnarchived,
    TagCreated,
    TagUpdated,
    TagDeleted,
    VisitThrottled,
    CapabilityRefused,
}

impl std::fmt::Display for BlogAuditEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlogCreated => write!(f, "blog_created"),
            Self::BlogUpdated => write!(f, "blog_updated"),
            Self::BlogArchived => write!(f, "blog_archived"),
            Self::BlogUnarchived => write!(f, "blog_unarchived"),
            Self::BlogDeleteRefused => write!(f, "blog_delete_refused"),
            Self::PostCreated => write!(f, "post_created"),
            Self::PostUpdated => write!(f, "post_updated"),
            Self::PublishRefused => write!(f, "publish_refused"),
            Self::PostPublished => write!(f, "post_published"),
            Self::NotifyParked => write!(f, "notify_parked"),
            Self::PostUnpublished => write!(f, "post_unpublished"),
            Self::PostArchived => write!(f, "post_archived"),
            Self::PostUnarchived => write!(f, "post_unarchived"),
            Self::TagCreated => write!(f, "tag_created"),
            Self::TagUpdated => write!(f, "tag_updated"),
            Self::TagDeleted => write!(f, "tag_deleted"),
            Self::VisitThrottled => write!(f, "visit_throttled"),
            Self::CapabilityRefused => write!(f, "capability_refused"),
        }
    }
}

impl FromStr for BlogAuditEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "blog_created" => Ok(Self::BlogCreated),
            "blog_updated" => Ok(Self::BlogUpdated),
            "blog_archived" => Ok(Self::BlogArchived),
            "blog_unarchived" => Ok(Self::BlogUnarchived),
            "blog_delete_refused" => Ok(Self::BlogDeleteRefused),
            "post_created" => Ok(Self::PostCreated),
            "post_updated" => Ok(Self::PostUpdated),
            "publish_refused" => Ok(Self::PublishRefused),
            "post_published" => Ok(Self::PostPublished),
            "notify_parked" => Ok(Self::NotifyParked),
            "post_unpublished" => Ok(Self::PostUnpublished),
            "post_archived" => Ok(Self::PostArchived),
            "post_unarchived" => Ok(Self::PostUnarchived),
            "tag_created" => Ok(Self::TagCreated),
            "tag_updated" => Ok(Self::TagUpdated),
            "tag_deleted" => Ok(Self::TagDeleted),
            "visit_throttled" => Ok(Self::VisitThrottled),
            "capability_refused" => Ok(Self::CapabilityRefused),
            _ => Err(format!("Unknown BlogAuditEvent variant: {}", s)),
        }
    }
}

impl Default for BlogAuditEvent {
    fn default() -> Self {
        Self::BlogCreated
    }
}
