//! ARCHIVE RULES (the BLOG-PUBLISH archive arm + the blog cascade):
//! the one-way archive pair, the single-statement cascade, the marker
//! restore, and the guarded delete — SPEC sections 4.1/4.2 and 13.5.
//!
//! Claims:
//! 1. archiving a PUBLISHED post forces the unpublish (one-way).
//! 2. unarchive restores liveness ONLY — `is_published` stays false.
//! 3. blog archive cascades over the still-live posts in ONE
//!    transaction (marker `archived_by_blog_id` set, cascade count
//!    audited).
//! 4. blog unarchive restores EXACTLY the marker rows — a post
//!    archived individually before the cascade STAYS archived.
//! 5. deleting a blog that still has posts → the typed 409
//!    `blog_blog_has_posts` + the `blog_delete_refused` audit fact.

use std::sync::Arc;

use backbone_blog::application::service::blog_service::BlogCommandService;
use backbone_blog::application::service::notifier_port::RecordingNotifier;
use backbone_blog::infrastructure::persistence::blog_command_repository::BlogCommandRepository;
use backbone_orm::company_scope::with_company_scope;

use super::common::{audit_count, make_blog, make_post_visible, posts_with, probe_tenancy, TestDb};

#[tokio::test]
async fn archive_is_one_way_and_the_cascade_restores_exactly_its_marker_rows() {
    let db = TestDb::new("archrule").await;
    let (company, view) = probe_tenancy();
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;

    let posts = posts_with(&db, Arc::new(RecordingNotifier::new()));
    let blogs = BlogCommandService::new(BlogCommandRepository::new(db.pool.clone()));

    // Three published posts: p0 will archive INDIVIDUALLY before the
    // cascade; p1/p2 stay live until the blog cascade takes them.
    let p0 = make_post_visible(&db, company, blog.id, "solo-archived").await;
    let p1 = make_post_visible(&db, company, blog.id, "cascaded-one").await;
    let p2 = make_post_visible(&db, company, blog.id, "cascaded-two").await;

    // 1. Post archive forces the unpublish.
    let archived = with_company_scope(Some(company), posts.archive(p0, None))
        .await
        .unwrap();
    assert!(archived.archived_at.is_some());
    assert!(
        !archived.is_published,
        "archiving a published post forces the unpublish"
    );
    assert!(
        archived.archived_by_blog_id.is_none(),
        "an individual archive carries NO cascade marker"
    );

    // 2. Unarchive restores liveness only; nothing re-publishes.
    let revived = with_company_scope(Some(company), posts.unarchive(p0, None))
        .await
        .unwrap();
    assert!(revived.archived_at.is_none(), "unarchive restores liveness");
    assert!(!revived.is_published, "unarchive NEVER re-publishes");

    // Re-archive p0 so the marker-restore claim is observable: it is
    // now archived individually (no marker).
    with_company_scope(Some(company), posts.archive(p0, None))
        .await
        .unwrap();

    // 3. Blog archive: ONE transaction, marker cascade over the LIVE
    //    posts only, count audited.
    let (row, cascaded) = with_company_scope(Some(company), blogs.archive(blog.id, None))
        .await
        .unwrap();
    assert!(row.archived_at.is_some());
    assert_eq!(cascaded, 2, "exactly the two live posts cascaded");
    assert_eq!(audit_count(&db, "blog_archived").await, 1);
    let detail: serde_json::Value = sqlx::query_scalar(
        "SELECT detail FROM blog.blog_audit_log \
          WHERE event = 'blog_archived' AND subject_id = $1",
    )
    .bind(blog.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        detail["posts_cascade"],
        serde_json::json!(2),
        "the cascade count is audited"
    );
    for post_id in [p1, p2] {
        let marker: Option<Option<uuid::Uuid>> =
            sqlx::query_scalar("SELECT archived_by_blog_id FROM blog.posts WHERE id = $1")
                .bind(post_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(marker, Some(Some(blog.id)), "the cascade sets the marker");
    }
    // NULL-proof form: a bare NULL scalar under fetch_one is itself
    // None, so ask the boolean question instead.
    let p0_has_marker: bool =
        sqlx::query_scalar("SELECT archived_by_blog_id IS NOT NULL FROM blog.posts WHERE id = $1")
            .bind(p0)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        !p0_has_marker,
        "the individually-archived post keeps NO marker"
    );

    // 4. Blog unarchive restores EXACTLY the marker rows; p0 stays
    //    archived; nothing re-publishes.
    let (row, restored) = with_company_scope(Some(company), blogs.unarchive(blog.id, None))
        .await
        .unwrap();
    assert!(row.archived_at.is_none());
    assert_eq!(restored, 2, "exactly the two marker rows returned");
    for post_id in [p1, p2] {
        let state: (bool, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as("SELECT is_published, archived_at FROM blog.posts WHERE id = $1")
                .bind(post_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(state, (false, None), "restored live, never re-published");
    }
    let p0_state: (bool, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT is_published, archived_at FROM blog.posts WHERE id = $1")
            .bind(p0)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        p0_state.1.is_some(),
        "the individually-archived post STAYS archived"
    );

    db.dispose().await;
}

#[tokio::test]
async fn deleting_a_blog_with_posts_is_the_typed_refusal() {
    let db = TestDb::new("archdel").await;
    let (company, view) = probe_tenancy();
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let _post = make_post_visible(&db, company, blog.id, "blocking-post").await;

    let blogs = BlogCommandService::new(BlogCommandRepository::new(db.pool.clone()));
    let refusal = with_company_scope(Some(company), blogs.delete(blog.id, None))
        .await
        .unwrap_err();
    assert_eq!(refusal.code(), "blog_blog_has_posts");
    assert_eq!(
        audit_count(&db, "blog_delete_refused").await,
        1,
        "the refused delete is a durable fact"
    );
    // The blog row survived.
    let still_there: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blog.blogs WHERE id = $1 AND metadata->>'deleted_at' IS NULL",
    )
    .bind(blog.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(still_there, 1);

    // The EMPTY-blog delete goes through (the guarded arm's other
    // side) — delete needs ZERO posts; a fresh empty blog cleans up.
    let empty = make_blog(&db, company, view.id, "Empty Journal").await;
    with_company_scope(Some(company), blogs.delete(empty.id, None))
        .await
        .unwrap();

    db.dispose().await;
}
