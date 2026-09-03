//! TAG GRAMMAR (the BLOG-TAGS register row): the company-grain unique
//! walls, the rename-redirect flow over the website seam, the tag
//! filter's comma grammar, and the cloud's visibility fence — SPEC
//! sections 4.4 and 13.3.
//!
//! Claims:
//! 1. duplicate name (CASE-FOLDED) in one company → 409
//!    `blog_tag_name_taken` (D3: the wall is `lower(name)`).
//! 2. the SAME name in ANOTHER company → OK (company grain, D3).
//! 3. rename → the slug recomputes and the stale slug is recorded
//!    through the seam exactly ONCE (`/tags/{old}` → `/tags/{new}`,
//!    moved_301).
//! 4. a stale tag slug in the listing filter resolves through the
//!    seam's canned 301 to the canonical tag.
//! 5. an unknown tag slug (no seam answer) → the uniform 404.
//! 6. more than one tag on a GET → 302 to the first tag's canonical
//!    listing URL.
//! 7. the cloud counts only VISIBLE posts (an archived post's tag
//!    disappears from the count).

use std::sync::Arc;

use backbone_blog::application::service::public_query_service::{
    record_tag_slug_redirect, ListingAnswer, PublicQueryService,
};
use backbone_blog::application::service::tag_service::TagCommandService;
use backbone_blog::infrastructure::persistence::tag_command_repository::normalize_slug;
use backbone_blog::infrastructure::persistence::tag_command_repository::TagCommandRepository;
use backbone_orm::company_scope::with_company_scope;

use super::common::{
    make_blog, make_post_archived, make_post_visible, make_tag, probe_tenancy, tag_post,
    StubSurface, TestDb, PROBE_HOST,
};

#[tokio::test]
async fn tag_name_walls_are_company_grain_and_case_folded() {
    let db = TestDb::new("taggrain").await;
    let (company, _view) = probe_tenancy();

    let service = TagCommandService::new(TagCommandRepository::new(db.pool.clone()));

    // The name exists once per company, case-folded.
    let first = with_company_scope(Some(company), service.create(company, "News", None, None))
        .await
        .unwrap();
    let dup = with_company_scope(Some(company), service.create(company, "NEWS", None, None)).await;
    assert_eq!(dup.unwrap_err().code(), "blog_tag_name_taken");

    // The SAME name in ANOTHER company is fine (D3).
    let other_company = uuid::Uuid::new_v4();
    let theirs = with_company_scope(
        Some(other_company),
        service.create(other_company, "News", None, None),
    )
    .await
    .unwrap();
    assert_ne!(theirs.id, first.id);

    db.dispose().await;
}

#[tokio::test]
async fn rename_recomputes_the_slug_and_records_one_redirect() {
    let db = TestDb::new("tagren").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    make_blog(&db, company, view.id, "Probe Journal").await;

    let service = TagCommandService::new(TagCommandRepository::new(db.pool.clone()));
    let tag = with_company_scope(
        Some(company),
        service.create(company, "Old Label", None, None),
    )
    .await
    .unwrap();
    assert_eq!(tag.slug, normalize_slug("Old Label"));

    let (row, prior) = with_company_scope(
        Some(company),
        service.rename(tag.id, Some("New Label"), None, None),
    )
    .await
    .unwrap();
    let prior = prior.unwrap();
    assert_eq!(prior, "old-label");
    assert_eq!(row.slug, "new-label");

    // The CALLER-side recording (the route's arm, here the helper the
    // routes share): exactly ONE seam call, module-relative, 301.
    record_tag_slug_redirect(surface.as_ref(), view.id, &prior, &row.slug).await;
    let recorded = surface.recorded();
    assert_eq!(recorded.len(), 1, "exactly one record_redirect");
    assert_eq!(
        recorded[0],
        (
            view.id,
            "/tags/old-label".to_string(),
            "/tags/new-label".to_string(),
            "moved_301".to_string()
        ),
        "the redirect is module-relative and permanent"
    );

    // The loop guard: recording a no-move records nothing.
    record_tag_slug_redirect(surface.as_ref(), view.id, &row.slug, &row.slug).await;
    assert_eq!(surface.recorded().len(), 1, "from == to records nothing");

    db.dispose().await;
}

#[tokio::test]
async fn the_listing_tag_filter_grammar_resolves_stale_unknown_and_multi() {
    let db = TestDb::new("tagfilt").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let blog_slug = normalize_slug(&blog.name);

    let post_id = make_post_visible(&db, company, blog.id, "tagged-post").await;
    let canonical = make_tag(&db, company, "Canonical Tag").await;
    let second = make_tag(&db, company, "Second Tag").await;
    tag_post(&db, company, post_id, &[canonical.id]).await;

    let public_queries = PublicQueryService::new(db.pool.clone(), surface.clone());

    // The canonical slug filters directly.
    let ListingAnswer::Page(page) = public_queries
        .listing(
            PROBE_HOST,
            &blog_slug,
            Some("canonical-tag"),
            1,
            Default::default(),
            None,
        )
        .await
        .unwrap()
    else {
        panic!("the canonical tag slug must filter");
    };
    assert_eq!(page.total, 1);

    // A STALE slug: the seam answers 301 → the canonical listing.
    surface.answer("/tags/stale-tag", "moved_301", Some("/tags/canonical-tag"));
    let ListingAnswer::Page(page) = public_queries
        .listing(
            PROBE_HOST,
            &blog_slug,
            Some("stale-tag"),
            1,
            Default::default(),
            None,
        )
        .await
        .unwrap()
    else {
        panic!("the stale slug must resolve through the seam");
    };
    assert_eq!(page.total, 1, "the stale slug reached the canonical tag");

    // UNKNOWN slug (no seam answer): the uniform 404 — never a trust.
    let miss = public_queries
        .listing(
            PROBE_HOST,
            &blog_slug,
            Some("never-heard-of"),
            1,
            Default::default(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(miss.code(), "blog_not_found");

    // MULTI-tag GET: 302 to the FIRST tag's canonical URL.
    let answer = public_queries
        .listing(
            PROBE_HOST,
            &blog_slug,
            Some("canonical-tag,second-tag"),
            1,
            Default::default(),
            None,
        )
        .await
        .unwrap();
    match answer {
        ListingAnswer::RedirectTo(location) => assert_eq!(
            location,
            format!("/public/blogs/{blog_slug}/posts?tag=canonical-tag"),
            "more than one tag on a GET redirects to the first canonical"
        ),
        ListingAnswer::Page(_) => panic!("a multi-tag GET must answer the 302, not a page"),
    }
    let _ = second; // both members must resolve for the 302 arm

    db.dispose().await;
}

#[tokio::test]
async fn the_cloud_counts_only_visible_posts() {
    let db = TestDb::new("tagcloud").await;
    let (company, view) = probe_tenancy();
    let surface = Arc::new(StubSurface::binding(view.clone()));
    let blog = make_blog(&db, company, view.id, "Probe Journal").await;
    let blog_slug = normalize_slug(&blog.name);

    let visible_id = make_post_visible(&db, company, blog.id, "cloud-visible").await;
    let archived_id = make_post_archived(&db, company, blog.id, "cloud-archived").await;
    let visible_tag = make_tag(&db, company, "Live Topic").await;
    let archived_tag = make_tag(&db, company, "Dead Topic").await;
    tag_post(&db, company, visible_id, &[visible_tag.id]).await;
    tag_post(&db, company, archived_id, &[archived_tag.id]).await;

    let public_queries = PublicQueryService::new(db.pool.clone(), surface);
    let cloud = public_queries
        .cloud(PROBE_HOST, &blog_slug, 1)
        .await
        .unwrap();
    let slugs: Vec<&str> = cloud.iter().map(|entry| entry.slug.as_str()).collect();
    assert!(
        slugs.contains(&"live-topic"),
        "the visible post's tag is counted: {slugs:?}"
    );
    assert!(
        !slugs.contains(&"dead-topic"),
        "the archived post's tag is NOT counted: {slugs:?}"
    );

    db.dispose().await;
}
