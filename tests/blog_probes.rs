//! The module's probe suite entry.
//!
//! GATES THIS SUITE SERVES (fail-hard; a vacuous skip is a failure,
//! never a green tick):
//! - `cargo check --all-targets` — compiles clean.
//! - `cargo clippy --all-targets -- -D clippy::expect_used` — the
//!   module lint bar (never `expect` in library code).
//! - `cargo fmt --check` — formatting.
//! - `cargo test` — THIS suite: every probe mints one disposable
//!   database on the scratch Postgres (127.0.0.1:5433, NEVER the live
//!   dev database on 5432), applies the module's migrations, runs, and
//!   drops it. The module carries NO tenancy of its own (ADR-0029): the
//!   row-level-security fence probes live with the composing service,
//!   which owns the tenancy decorator and the org session guard — the
//!   probes here run undecorated and prove the module-local behaviors.
//! - Schema regen DRIFT=0 (outside cargo; the generator's own gate).

mod probes;
