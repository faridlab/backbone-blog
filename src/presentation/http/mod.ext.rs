// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// The verb route trees (hand-written; user-owned — see
// metaphor.codegen.yaml): the five-path public allowlist (mounted BARE
// of any session guard by the host) and the officer tree (mounted
// behind the host's org session guard).
pub mod admin_routes;
pub mod public_routes;
pub use admin_routes::{blog_admin_routes, BlogAdminState};
pub use public_routes::{
    blog_public_routes, caller_ip, trusted_proxy_from_env, BlogPublicState, BLOG_TRUSTED_PROXY_ENV,
};
