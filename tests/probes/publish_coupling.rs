//! PUBLISH COUPLING (the BLOG-PUBLISH register row): the publish
//! flip, the date stamp, the ONE audit event, the notifier, and the
//! PATCH fence around the pair — SPEC sections 4.1 and 13.1.
//!
//! Claims, in order:
//! 1. draft → publish: `is_published` flips, `published_date` stamps
//!    now, exactly ONE `post_published` audit row exists (the row IS
//!    the event — no second table), the notifier fired ONCE.
//! 2. a pre-set FUTURE `published_date` survives the stamp (the lazy
//!    schedule arm: the CASE keeps the future date, never now).
//! 3. a PATCH carrying the fence pair → 422 `blog_field_not_patchable`
//!    + one `publish_refused` audit fact (a refused write is a durable
//!    fact), proven through the ADMIN ROUTER (the wire, not the
//!    service).
//! 4. re-publish on a published row → no-op success: no re-stamp, no
//!    second audit row, no second notify.
//! 5. unpublish RETAINS the date (history) and flips the flag.
//! 6. publish under the REFUSING notifier → the verb still commits +
//!    one `notify_parked` audit row (publishing is not
//!    deliverable-contingent).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    extract::Request as ExtractRequest,
    middleware::{self, Next},
    response::Response,
};
use tower::ServiceExt;

use backbone_blog::application::service::notifier_port::RefusingPublishNotifier;
use backbone_blog::presentation::http::{blog_admin_routes, BlogAdminState};

use super::common::{
    audit_count, make_blog, make_post_draft, posts_with, preset_future_published_date,
    probe_website, TestDb,
};

/// The probe officer: a fixed uuid so the admin verbs attribute audit
/// rows to a parseable principal.
const PROBE_OFFICER_ID: &str = "00000000-0000-0000-0000-00000000d001";

/// The probe's identity layer: inserts the `OrgContext` the composing
/// host's org session guard would. The guard itself is host-owned
/// (ADR-0029 composition); the guarded handlers extract only this
/// extension, so the probe stands in with it directly.
async fn probe_org_context(
    mut req: ExtractRequest,
    next: Next,
) -> Response {
    req.extensions_mut().insert(backbone_auth::org::OrgContext {
        acting_unit_id: PROBE_OFFICER_ID.parse().unwrap(),
        entitled_units: Vec::new(),
        legacy_company_id: None,
        user_id: PROBE_OFFICER_ID.to_string(),
    });
    next.run(req).await
}

fn admin_app(db: &TestDb) -> axum::Router {
    let state = BlogAdminState::new(
        db.pool.clone(),
        Arc::new(backbone_blog::application::service::notifier_port::RecordingNotifier::new()),
        None,
    );
    blog_admin_routes(state).layer(middleware::from_fn(probe_org_context))
}

async fn patch_status(
    app: &axum::Router,
    post_id: uuid::Uuid,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/admin/posts/{post_id}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
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
async fn publish_flips_stamps_audits_once_and_notifies_once() {
    let db = TestDb::new("pubc").await;
    let view = probe_website();
    let blog = make_blog(&db, view.id, "Probe Journal").await;
    let post_id = make_post_draft(&db, blog.id, "coupling-post").await;

    let notifier =
        Arc::new(backbone_blog::application::service::notifier_port::RecordingNotifier::new());
    let service = posts_with(&db, notifier.clone());
    let outcome = service.publish(post_id, None).await.unwrap();

    // 1a. The flip + the stamp.
    assert!(outcome.row.is_published, "publish must flip the flag");
    let stamped = outcome.row.published_date.unwrap();
    assert!(
        (chrono::Utc::now() - stamped).num_seconds() <= 60,
        "the stamp is now, not a pre-set or future date"
    );
    // 1b. Exactly ONE event row (this row IS the BlogPostPublished
    // event; there is no second event table).
    assert_eq!(audit_count(&db, "post_published").await, 1);
    // 1c. The notifier fired ONCE.
    assert_eq!(
        notifier.calls().len(),
        1,
        "notify exactly once per transition"
    );

    // 4. Re-publish: the guarded flip sees true → no-op success, no
    // re-stamp (the stamp is unchanged), no second audit, no re-notify.
    let before = outcome.row.published_date;
    let again = service.publish(post_id, None).await.unwrap();
    assert!(!again.changed, "re-publish must report the no-op");
    assert!(again.row.is_published);
    assert_eq!(again.row.published_date, before, "no re-stamp on the no-op");
    assert_eq!(
        audit_count(&db, "post_published").await,
        1,
        "no second event row on the no-op"
    );
    assert_eq!(notifier.calls().len(), 1, "no re-notify on the no-op");

    // 5. Unpublish retains the date as history.
    let unpublished = service.unpublish(post_id, None).await.unwrap();
    assert!(!unpublished.is_published);
    assert!(
        unpublished.published_date.is_some(),
        "unpublish RETAINS the stamp as history"
    );

    db.dispose().await;
}

#[tokio::test]
async fn a_future_published_date_survives_the_stamp() {
    let db = TestDb::new("pubfut").await;
    let view = probe_website();
    let blog = make_blog(&db, view.id, "Probe Journal").await;
    let post_id = make_post_draft(&db, blog.id, "future-stamp").await;

    // Pre-set the schedule: publish is asked to hold a FUTURE date.
    preset_future_published_date(&db, post_id).await;

    let service = posts_with(
        &db,
        Arc::new(backbone_blog::application::service::notifier_port::RecordingNotifier::new()),
    );
    let outcome = service.publish(post_id, None).await.unwrap();
    assert!(outcome.row.is_published);
    let stamped = outcome.row.published_date.unwrap();
    assert!(
        stamped > chrono::Utc::now(),
        "the FUTURE schedule survives the stamp (the CASE keeps it, never now)"
    );

    db.dispose().await;
}

#[tokio::test]
async fn patch_carrying_the_fence_pair_is_refused_and_audited() {
    let db = TestDb::new("pubfence").await;
    let view = probe_website();
    let blog = make_blog(&db, view.id, "Probe Journal").await;
    let post_id = make_post_draft(&db, blog.id, "fenced-post").await;

    let app = admin_app(&db);

    // The fence pair in a patch body → the typed refusal.
    let (status, body) = patch_status(
        &app,
        post_id,
        serde_json::json!({ "is_published": true, "title": "smuggled" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "blog_field_not_patchable");
    // The refusal is a durable fact.
    assert_eq!(audit_count(&db, "publish_refused").await, 1);
    // And nothing was written: the smuggled title did not land.
    let service = posts_with(
        &db,
        Arc::new(backbone_blog::application::service::notifier_port::RecordingNotifier::new()),
    );
    let row = service.get(post_id).await.unwrap();
    assert_eq!(row.title, "Probe Post", "the refused patch wrote nothing");
    assert!(!row.is_published, "the fence held: still a draft");

    // A structurally-refused field answers the same typed refusal
    // (no `publish_refused` audit — it is not a publication attempt).
    let (status, body) = patch_status(&app, post_id, serde_json::json!({ "visits": 999 })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "blog_field_not_patchable");
    assert_eq!(audit_count(&db, "publish_refused").await, 1);

    db.dispose().await;
}

#[tokio::test]
async fn a_refused_notify_parks_but_the_publish_commits() {
    let db = TestDb::new("pubpark").await;
    let view = probe_website();
    let blog = make_blog(&db, view.id, "Probe Journal").await;
    let post_id = make_post_draft(&db, blog.id, "parked-post").await;

    let service = posts_with(&db, Arc::new(RefusingPublishNotifier::new()));
    let outcome = service.publish(post_id, None).await.unwrap();
    assert!(
        outcome.row.is_published,
        "publishing is not deliverable-contingent"
    );
    assert_eq!(audit_count(&db, "post_published").await, 1);
    assert_eq!(
        audit_count(&db, "notify_parked").await,
        1,
        "the refused notify is parked as a durable fact, never a rollback"
    );

    db.dispose().await;
}
