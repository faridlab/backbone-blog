//! VISITS DEDUP (the BLOG-VISITS register row): the receipt wall, the
//! token grammar, the absence of any visitor profile, and the throttle
//! — SPEC sections 4.5/6 and 13.4.
//!
//! Claims:
//! 1. the SAME verified token within the day → `counted: false`, the
//!    counter unmoved (the unique `(post_id, view_token,
//!    window_start)` receipt IS the dedup wall).
//! 2. a NEW token → counts.
//! 3. 25 CONCURRENT first-visits with one token → the counter moves
//!    exactly ONCE (every call still 200 — the wall never errors on a
//!    duplicate).
//! 4. N distinct tokens → exactly N counts.
//! 5. an INVISIBLE post (draft) → the uniform 404 AND zero receipt
//!    rows (the miss rolls back — no oracle on what almost existed).
//! 6. schema census: no table in the `blog` schema matches
//!    `%visitor%` (D4 — there is no visitor profile anywhere).
//! 7. the throttle burst → 429 `blog_throttled` + `Retry-After`
//!    (+ the `visit_throttled` refusal audit).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use backbone_blog::application::service::capability::mint_view_token;
use backbone_blog::presentation::http::{blog_public_routes, BlogPublicState};

use super::common::{
    make_blog, make_post_draft, make_post_visible, probe_tenancy, StubSurface, TestDb, PROBE_HOST,
    PROBE_SECRET,
};

type Answer = (StatusCode, serde_json::Value, Option<u64>);

async fn visit(app: &axum::Router, post_slug: &str, token: Option<&str>) -> Answer {
    let body = token
        .map(|t| serde_json::json!({ "view_token": t }).to_string())
        .unwrap_or_else(|| "{}".to_string());
    let request = Request::builder()
        .method("POST")
        .uri(format!("/public/posts/{post_slug}/visit"))
        .header("host", PROBE_HOST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, retry_after)
}

fn mint() -> String {
    mint_view_token(PROBE_SECRET, chrono::Utc::now()).unwrap()
}

#[tokio::test]
async fn the_receipt_wall_dedupes_tokens_not_people() {
    let db = TestDb::new("visitdedup").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let _post_id = make_post_visible(&db, company, blog.id, "counted-post").await;

    let state = BlogPublicState::with_secret(db.pool.clone(), surface, PROBE_SECRET.to_string());
    let app = blog_public_routes(state);

    // A first visit with NO token mints one and counts.
    let (status, body, _) = visit(&app, "counted-post", None).await;
    assert_eq!(status, StatusCode::OK, "body {body}");
    assert_eq!(body["counted"], serde_json::json!(true));
    assert_eq!(body["visits"], serde_json::json!(1));
    let minted = body["view_token"].as_str().unwrap().to_string();

    // The SAME token again (the next page view) → the wall eats it.
    let (status, body, _) = visit(&app, "counted-post", Some(&minted)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["counted"],
        serde_json::json!(false),
        "the duplicate view is not counted"
    );
    assert_eq!(body["visits"], serde_json::json!(1));

    // A NEW token → counts.
    let fresh = mint();
    let (status, body, _) = visit(&app, "counted-post", Some(&fresh)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["counted"], serde_json::json!(true));
    assert_eq!(body["visits"], serde_json::json!(2));

    // A FORGED token → the uniform refusal (never an oracle on which
    // arm failed).
    let (status, body, _) = visit(&app, "counted-post", Some("v1.forged.sig")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body {body}");
    assert_eq!(body["error"]["code"], "blog_invalid_input");

    db.dispose().await;
}

#[tokio::test]
async fn concurrent_first_visits_with_one_token_move_the_counter_once() {
    let db = TestDb::new("visitrace").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let _post_id = make_post_visible(&db, company, blog.id, "raced-post").await;

    let state = Arc::new(BlogPublicState::with_secret(
        db.pool.clone(),
        surface,
        PROBE_SECRET.to_string(),
    ));

    // 25 concurrent first-visits, ONE shared token: every call 200,
    // exactly one counted, the counter ends at 1.
    let token = mint();
    let mut tasks = Vec::new();
    for _ in 0..25 {
        let app = blog_public_routes((*state).clone());
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            visit(&app, "raced-post", Some(&token)).await
        }));
    }
    let mut counted = 0;
    for task in tasks {
        let (status, body, _) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "body {body}");
        if body["counted"] == serde_json::json!(true) {
            counted += 1;
        }
    }
    assert_eq!(counted, 1, "exactly one of the 25 concurrent views counts");
    let (_, body, _) = visit(
        &blog_public_routes((*state).clone()),
        "raced-post",
        Some(&token),
    )
    .await;
    assert_eq!(
        body["visits"],
        serde_json::json!(1),
        "the counter moved exactly once under contention"
    );

    db.dispose().await;
}

#[tokio::test]
async fn n_distinct_tokens_count_exactly_n_and_invisible_posts_leave_no_receipt() {
    let db = TestDb::new("visittok").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let visible_id = make_post_visible(&db, company, blog.id, "token-post").await;
    let draft_id = make_post_draft(&db, company, blog.id, "draft-post").await;

    let state = BlogPublicState::with_secret(db.pool.clone(), surface, PROBE_SECRET.to_string());
    let app = blog_public_routes(state);

    // N distinct tokens → exactly N counts.
    for n in 1..=5i32 {
        let (status, body, _) = visit(&app, "token-post", Some(&mint())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["visits"],
            serde_json::json!(n),
            "distinct token {n} moves the counter by exactly one"
        );
    }

    // The INVISIBLE post: the uniform 404 and NO receipt row.
    let (status, body, _) = visit(&app, "draft-post", Some(&mint())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body {body}");
    assert_eq!(body["error"]["code"], "blog_not_found");
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blog.post_view_receipts WHERE post_id = $1")
            .bind(draft_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(receipts, 0, "the miss rolls back — no receipt survives");
    let visible_receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blog.post_view_receipts WHERE post_id = $1")
            .bind(visible_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        visible_receipts, 5,
        "the five counted views left five receipts"
    );
    let _ = visible_receipts;

    // The census: no visitor-profile table exists anywhere in the
    // schema (D4).
    let visitor_tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables
          WHERE table_schema = 'blog' AND table_name ILIKE '%visitor%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        visitor_tables, 0,
        "no %visitor% table may exist in schema blog"
    );

    db.dispose().await;
}

#[tokio::test]
async fn the_burst_trips_the_throttle_with_retry_after() {
    let db = TestDb::new("visitburst").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let _post_id = make_post_visible(&db, company, blog.id, "burst-post").await;

    let state = BlogPublicState::with_secret(db.pool.clone(), surface, PROBE_SECRET.to_string());
    let app = blog_public_routes(state);

    // Under oneshot there is no socket address, so every call shares
    // the one per-IP bucket ("unknown") — the 61st call inside the
    // hour window trips the fixed window.
    let mut throttled_at: Option<usize> = None;
    let mut retry_after_seen = None;
    for n in 0..70usize {
        let (status, body, retry_after) = visit(&app, "burst-post", Some(&mint())).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(body["error"]["code"], "blog_throttled", "body {body}");
            assert_eq!(retry_after, Some(3600), "Retry-After carries the window");
            throttled_at = Some(n);
            retry_after_seen = retry_after;
            break;
        }
        assert_eq!(status, StatusCode::OK, "call {n} body {body}");
    }
    let tripped = throttled_at.unwrap();
    assert_eq!(
        tripped, 60,
        "the per-IP window admits exactly 60 before refusing"
    );
    assert_eq!(retry_after_seen, Some(3600));

    // The refusal is audited (refusals are the ONLY audited visit
    // facts).
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blog.blog_audit_log WHERE event = 'visit_throttled'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(audited >= 1, "the throttled burst left its audit fact");

    db.dispose().await;
}
