//! One file per register row / fence claim (SPEC section 13).

pub mod archive_rules;
pub mod common;
pub mod fenced_runtime;
pub mod lazy_visibility;
pub mod public_fences;
pub mod publish_coupling;
pub mod tag_grammar;
pub mod visits_dedup;
