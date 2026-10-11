// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// The verb layer (hand-written; user-owned — see metaphor.codegen.yaml):
// the typed error surface, the Tier A view-token family, the public-tier
// scope binder, the publish-notify port, and the three verb services the
// route layer composes.
pub mod blog_error;
pub mod capability;
pub mod notifier_port;
pub mod public_query_service;
pub mod site_scope;
pub mod visit_service;
pub use blog_error::{BlogError, BlogResult};
pub use notifier_port::{BlogPublishNotifier, RefusingPublishNotifier};
pub use public_query_service::{DetailAnswer, ListingAnswer, PublicQueryService};
pub use visit_service::{FixedWindows, VisitRequest, VisitResponse, VisitService, VISIT_THROTTLE};
