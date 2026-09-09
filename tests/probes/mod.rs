//! One file per register row / fence claim (SPEC section 13).
//!
//! Tenancy (ADR-0029): the module is composed — the row-level-security
//! fence probes live with the composing service, which owns the
//! decorator and the org session guard. These probes run undecorated
//! on a plain scratch database.

pub mod archive_rules;
pub mod common;
pub mod lazy_visibility;
pub mod public_fences;
pub mod publish_coupling;
pub mod tag_grammar;
pub mod visits_dedup;
