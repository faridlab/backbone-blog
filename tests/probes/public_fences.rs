//! PUBLIC FENCES (the BLOG-RULES register row, mount shape): the
//! exhaustive public allowlist, the no-fallback host binding, and the
//! fail-closed secret — SPEC sections 2.3/10 and 13.6.
//!
//! Claims:
//! 1. NEGATIVE ENUMERATION: the exported public router answers
//!    EXACTLY the five allowlisted paths; anything else is the
//!    router's own empty 404 (no handler ran).
//! 2. METHOD discipline: a allowlisted path under the wrong method is
//!    the router's 405.
//! 3. HOST-BINDING MISS: an unbound `Host` → the typed
//!    `blog_website_not_found` on EVERY public handler — no fallback
//!    to any first website.
//! 4. SECRET UNSET: the visit verb answers the typed 503
//!    `blog_capability_secret_not_configured`; the GET surface is
//!    unaffected.
//! 5. `caller_ip` (pure): rightmost forwarded hop ONLY under the
//!    trusted-proxy posture, the bare socket IP otherwise, `"unknown"`
//!    when nothing is available.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, HeaderMap, Request, StatusCode},
};
use tower::ServiceExt;

use backbone_blog::presentation::http::{
    blog_public_routes, caller_ip, BlogPublicState, BLOG_TRUSTED_PROXY_ENV,
};

use super::common::{
    make_blog, make_post_visible, probe_tenancy, StubSurface, TestDb, PROBE_HOST, PROBE_SECRET,
};

async fn get(app: &axum::Router, path: &str, host: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .uri(path)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn post_visit(app: &axum::Router, path: &str, host: &str) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn the_public_tree_is_exactly_five_paths_and_the_host_binding_has_no_fallback() {
    let db = TestDb::new("pubfence").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    make_post_visible(&db, company, blog.id, "live-post").await;

    let state = BlogPublicState::with_secret(db.pool.clone(), surface, PROBE_SECRET.to_string());
    let app = blog_public_routes(state);

    // THE FIVE (each answers a handler-produced JSON body, even when
    // the honest answer is a miss).
    for path in [
        "/public/blogs",
        "/public/blogs/probe-journal/posts",
        "/public/blogs/probe-journal/posts/live-post",
        "/public/blogs/probe-journal/tags",
    ] {
        let (status, body) = get(&app, path, PROBE_HOST).await;
        assert_eq!(status, StatusCode::OK, "path {path}");
        assert!(!body.is_empty(), "path {path} answered a handler body");
    }
    let (status, body) = post_visit(&app, "/public/posts/live-post/visit", PROBE_HOST).await;
    assert_eq!(status, StatusCode::OK, "visit body {body}");

    // NEGATIVE ENUMERATION: near-misses and admin paths get the
    // ROUTER's empty 404 — no handler ran, nothing was queried.
    for path in [
        "/public/blogs/probe-journal/posts/live-post/extra",
        "/public/blogs/probe-journal",
        "/public/posts",
        "/public/posts/live-post",
        "/public/visit",
        "/admin/blogs",
        "/blogs",
        "/",
    ] {
        let (status, body) = get(&app, path, PROBE_HOST).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "path {path}");
        assert!(
            body.is_empty(),
            "path {path} must be the router's own 404 (no handler body)"
        );
    }

    // METHOD discipline: the listing under POST is the router's 405.
    let request = Request::builder()
        .method("POST")
        .uri("/public/blogs/probe-journal/posts")
        .header(header::HOST, PROBE_HOST)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    // HOST-BINDING MISS: an unbound host → the typed
    // `blog_website_not_found` on every public handler (no fallback).
    for path in [
        "/public/blogs",
        "/public/blogs/probe-journal/posts",
        "/public/blogs/probe-journal/posts/live-post",
        "/public/blogs/probe-journal/tags",
    ] {
        let (status, body) = get(&app, path, "stranger.example.test").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "path {path}");
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        assert_eq!(
            json["error"]["code"], "blog_website_not_found",
            "path {path}: the miss is loud and typed"
        );
    }
    let (status, body) = post_visit(
        &app,
        "/public/posts/live-post/visit",
        "stranger.example.test",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "blog_website_not_found");

    // A MISSING Host header is the same typed miss (never a fallback).
    let request = Request::builder()
        .uri("/public/blogs")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    db.dispose().await;
}

#[tokio::test]
async fn an_unset_secret_fails_the_visit_verb_closed_and_leaves_gets_alone() {
    let db = TestDb::new("pubsecret").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    make_post_visible(&db, company, blog.id, "live-post").await;

    // The secret is EMPTY — the fail-closed posture.
    let state = BlogPublicState::with_secret(db.pool.clone(), surface, String::new());
    assert!(!state.secret_is_configured());
    let app = blog_public_routes(state);

    let (status, body) = post_visit(&app, "/public/posts/live-post/visit", PROBE_HOST).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body {body}");
    assert_eq!(
        body["error"]["code"],
        "blog_capability_secret_not_configured"
    );

    // The GET surface never needed the secret.
    let (status, body) = get(&app, "/public/blogs", PROBE_HOST).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty(), "GETs are unaffected by the unset secret");

    db.dispose().await;
}

#[test]
fn caller_ip_reads_only_the_rightmost_hop_and_only_under_trust() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "1.1.1.1, 2.2.2.2, 3.3.3.3".parse().unwrap(),
    );

    // UNTRUSTED: the header is client-controlled text and is ignored.
    assert_eq!(
        caller_ip(&headers, Some("9.9.9.9"), false),
        "9.9.9.9",
        "an untrusted proxy posture never reads the forwarded header"
    );
    // TRUSTED: only the RIGHTMOST hop (the entry the nearest trusted
    // proxy appended); every hop to its left is client-supplied.
    assert_eq!(caller_ip(&headers, Some("9.9.9.9"), true), "3.3.3.3");
    // No header under trust → the socket IP.
    assert_eq!(
        caller_ip(&HeaderMap::new(), Some("9.9.9.9"), true),
        "9.9.9.9"
    );
    // Nothing at all → "unknown".
    assert_eq!(caller_ip(&HeaderMap::new(), None, false), "unknown");
    // The env var name is part of the operator surface (documented,
    // stable).
    assert_eq!(BLOG_TRUSTED_PROXY_ENV, "BLOG_TRUSTED_PROXY");
}
