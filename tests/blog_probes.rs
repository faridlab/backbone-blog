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
//!   drops it. `fenced_runtime` additionally mints a NOSUPERUSER
//!   NOBYPASSRLS role and proves the row-level-security fence binds
//!   for real (a superuser run is RLS-blind — the lesson this probe
//!   exists to enforce).
//! - Schema regen DRIFT=0 (outside cargo; the generator's own gate).

mod probes;
