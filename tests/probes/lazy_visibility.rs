//! LAZY VISIBILITY (the BLOG-LAZY register row): a future-dated
//! published post is invisible to EVERY public read, visible to the
//! admin binding, counted as unpublished in the state split — and the
//! same row becomes publicly visible the moment `post_date` passes,
//! with ZERO schedulers — SPEC sections 4.3 and 13.2.
//!
//! The zero-cron claim is a SOURCE CENSUS, asserted here:
//! - `schema/hooks/index.hook.yaml` declares `events: {}` AND
//!   `scheduled_jobs: {}` (the module hooks into nothing periodic);
//! - no file under `src/` contains a `spawn_` token (no background
//!   task self-arms). Visibility is a read-time predicate
//!   (`post_date <= now()` in every public read), never a writer.

use std::sync::Arc;

use backbone_blog::application::service::public_query_service::{
    DetailAnswer, ListingAnswer, PublicQueryService,
};
use backbone_blog::infrastructure::persistence::public_query_repository::{
    AdminState, ListingOrder, PublicQueryRepository,
};
use backbone_blog::infrastructure::persistence::tag_command_repository::normalize_slug;

use super::common::{
    backdate_post, make_blog, make_post_future, make_post_visible, make_tag, probe_website,
    tag_post, StubSurface, TestDb, PROBE_HOST,
};

#[tokio::test]
async fn future_posts_are_invisible_publicly_visible_adminly_and_lazily_arrive() {
    let db = TestDb::new("lazyv").await;
    let view = probe_website();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, view.id, "Probe Journal").await;
    let blog_slug = normalize_slug(&blog.name);

    // One VISIBLE post (the older sibling the nav arm reads) + one
    // FUTURE published post (is_published = true, post_date a day out).
    let visible_id = make_post_visible(&db, blog.id, "visible-post").await;
    let future_id = make_post_future(&db, blog.id, "future-post").await;
    // Each carries a DISTINCT tag so the cloud arm is observable.
    let visible_tag = make_tag(&db, "Visible Only").await;
    let future_tag = make_tag(&db, "Future Only").await;
    tag_post(&db, visible_id, &[visible_tag.id]).await;
    tag_post(&db, future_id, &[future_tag.id]).await;

    let public_queries = PublicQueryService::new(db.pool.clone(), surface.clone());

    // LISTING: only the visible sibling.
    let answer = public_queries
        .listing(PROBE_HOST, &blog_slug, None, 1, ListingOrder::Recent, None)
        .await
        .unwrap();
    let ListingAnswer::Page(page) = answer else {
        panic!("unfiltered listing must answer a page");
    };
    assert_eq!(
        page.total, 1,
        "the future post is not in the public listing"
    );
    assert_eq!(page.items[0].id, visible_id);

    // DETAIL: the uniform 404 (no oracle — same answer as any other
    // miss).
    let miss = public_queries
        .detail(PROBE_HOST, &blog_slug, "future-post")
        .await
        .unwrap_err();
    assert_eq!(miss.code(), "blog_not_found");

    // CLOUD: the future post's tag does not appear; the visible
    // sibling's does (the cloud counts only visible posts).
    let cloud = public_queries
        .cloud(PROBE_HOST, &blog_slug, 1)
        .await
        .unwrap();
    let slugs: Vec<&str> = cloud.iter().map(|entry| entry.slug.as_str()).collect();
    assert!(
        slugs.contains(&"visible-only"),
        "the visible post's tag is counted: {slugs:?}"
    );
    assert!(
        !slugs.contains(&"future-only"),
        "the future post's tag is NOT counted: {slugs:?}"
    );

    // NAV: the visible post's wrap-around target is ITSELF (the only
    // visible post), never the future sibling.
    let DetailAnswer::Page(detail) = public_queries
        .detail(PROBE_HOST, &blog_slug, "visible-post")
        .await
        .unwrap()
    else {
        panic!("the visible post must serve detail");
    };
    let nav = detail.nav_next.unwrap();
    assert_eq!(nav.id, visible_id, "nav never points at a future post");

    // ADMIN: the same read surface SEES the future row (the
    // employees-see-everything posture).
    let queries = PublicQueryRepository::new(db.pool.clone());
    let (rows, counts) = queries
        .admin_list_posts(AdminState::All, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "admin sees both rows");
    assert_eq!(counts.published, 1, "future-aware: published ≡ visible");
    assert_eq!(
        counts.unpublished, 1,
        "a future-dated published post counts as UNPUBLISHED in the split"
    );
    let (unpublished_rows, _) = queries
        .admin_list_posts(AdminState::Unpublished, None, 100)
        .await
        .unwrap();
    assert_eq!(unpublished_rows.len(), 1);
    assert_eq!(unpublished_rows[0].id, future_id);

    // THE LAZY ARRIVAL: backdate the post (the sanctioned shortcut —
    // scoped SQL standing in for time passing; no cron ran, no writer
    // fired) and the SAME row is publicly visible through every arm.
    backdate_post(&db, future_id).await;
    let answer = public_queries
        .listing(PROBE_HOST, &blog_slug, None, 1, ListingOrder::Recent, None)
        .await
        .unwrap();
    let ListingAnswer::Page(page) = answer else {
        panic!("listing must answer a page");
    };
    assert_eq!(
        page.total, 2,
        "the arrival is read-time, with zero schedulers"
    );
    assert!(
        public_queries
            .detail(PROBE_HOST, &blog_slug, "future-post")
            .await
            .is_ok(),
        "detail serves the row the moment post_date passes"
    );

    db.dispose().await;
}

#[test]
fn zero_cron_census_the_module_arms_no_scheduler_and_spawns_nothing() {
    let manifest = env!("CARGO_MANIFEST_DIR");

    // 1. The hooks index declares NOTHING periodic.
    let hooks =
        std::fs::read_to_string(format!("{manifest}/schema/hooks/index.hook.yaml")).unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&hooks).unwrap();
    let empty_map = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    assert_eq!(
        doc["events"], empty_map,
        "the hooks index must declare events: {{}} (no publications/subscriptions arm work)"
    );
    assert_eq!(
        doc["scheduled_jobs"], empty_map,
        "the hooks index must declare scheduled_jobs: {{}} (the BLOG-LAZY register row: ZERO crons)"
    );

    // 2. No source file self-arms a background task.
    let src = std::path::Path::new(manifest).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("census cannot read {}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.contains("spawn_") {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "no src/ file may arm a background task (found spawn_ in {offenders:?})"
    );
}
