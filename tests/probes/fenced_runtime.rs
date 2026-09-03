//! The fenced-runtime probe: the repositories must pass the company
//! RLS fence — AND the public-tier fence — when the pool connects as a
//! NON-superuser role.
//!
//! Every other probe runs on the scratch SUPERUSER, which bypasses
//! row-level security entirely (superusers bypass even FORCE ROW LEVEL
//! SECURITY) — the exact blind spot that once let unscoped
//! repositories ship green while the fenced production runtime failed
//! (writes 42501, reads silently empty). This probe recreates the
//! production posture on the scratch database: a LOGIN role with
//! plain DML grants and NO BYPASSRLS, the migrations' FORCE ROW LEVEL
//! SECURITY policies active, and every repository call driven through
//! `backbone_orm::company_scope::with_company_scope`. If a repository
//! ever regresses to a raw unscoped statement, the scoped write below
//! fails the policy's WITH CHECK and this probe goes red.
//!
//! Claims, in order (SPEC section 13.7):
//! (1) an unscoped write naming a company → `BlogError::Database`
//!     containing "row-level security";
//! (2) a forged cross-tenant write refused;
//! (3) the scoped write passes;
//! (4) the scoped list sees only its own rows;
//! (5) a cross-company find → `blog_not_found`;
//! (6) NO scope → zero rows, never the table;
//! (7) the other company's row intact;
//! (8) THE TIER FENCE: under the SAME fenced role, a public-tier
//!     binding makes unpublished/future/archived posts invisible
//!     through EVERY public repository method — and through a raw
//!     predicate-free SELECT (the policy itself holds the line) —
//!     while the plain admin binding on the same role sees all four;
//! (9) NO TIER LEAKAGE: after a public-tier transaction commits, an
//!     admin read on the SAME pooled connection sees the unpublished
//!     row (`app.blog_tier` is transaction-local and reverts).

use std::sync::Arc;

use backbone_blog::application::service::blog_error::BlogError;
use backbone_blog::application::service::notifier_port::RecordingNotifier;
use backbone_blog::application::service::post_service::PostCommandService;
use backbone_blog::application::service::site_scope::bind_public_scope;
use backbone_blog::infrastructure::persistence::blog_command_repository::{
    BlogCommandRepository, CreateBlogInput,
};
use backbone_blog::infrastructure::persistence::post_command_repository::{
    PatchPostInput, PostCommandRepository,
};
use backbone_blog::infrastructure::persistence::public_query_repository::{
    ListingQuery, PublicQueryRepository,
};
use backbone_orm::company_scope::with_company_scope;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use super::common::TestDb;

/// The non-privileged role every fenced connection authenticates as.
const FENCED_ROLE: &str = "blog_probe_app";
const FENCED_PASSWORD: &str = "blog_probe_app";

/// Mint the production-shaped app role on the scratch cluster: LOGIN,
/// DML on the module's tables, NO BYPASSRLS. Cluster-wide, so the
/// name is unique to this probe. A role from an aborted run can hold
/// grants in a leaked scratch database (DROP ROLE refuses while any
/// depend on it), so stale fenced-marker databases go first — the
/// role then drops clean.
async fn fence_role(db: &TestDb) {
    let stale: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database \
         WHERE datname LIKE 'blog\\_probe\\_fenced\\_%' AND datname <> current_database()",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    for name in stale {
        sqlx::query(&format!(r#"DROP DATABASE "{name}" WITH (FORCE)"#))
            .execute(&db.pool)
            .await
            .unwrap_or_else(|e| panic!("stale scratch {name} drop: {e}"));
    }
    sqlx::raw_sql(&format!(
        "DROP ROLE IF EXISTS {FENCED_ROLE}; \
         CREATE ROLE {FENCED_ROLE} LOGIN PASSWORD '{FENCED_PASSWORD}' NOSUPERUSER NOBYPASSRLS; \
         GRANT USAGE ON SCHEMA blog TO {FENCED_ROLE}; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA blog TO {FENCED_ROLE}; \
         GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA blog TO {FENCED_ROLE};"
    ))
    .execute(&db.pool)
    .await
    .unwrap();
}

/// A pool for the SAME scratch database authenticating as the fenced
/// role — the posture the production runtime pool runs under.
async fn fenced_pool(db: &TestDb) -> PgPool {
    let opts = db.pool.connect_options();
    let dsn = format!(
        "postgres://{FENCED_ROLE}:{FENCED_PASSWORD}@{}:{}/{}",
        opts.get_host(),
        opts.get_port(),
        opts.get_database().unwrap(),
    );
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&dsn)
        .await
        .unwrap()
}

fn blog_input(name: &str, company: Uuid) -> CreateBlogInput {
    CreateBlogInput {
        company_id: company,
        website_id: Uuid::new_v4(),
        name: name.into(),
        subtitle: None,
        description: None,
    }
}

#[tokio::test]
async fn repositories_pass_the_fence_under_a_scoped_company() {
    let db = TestDb::new("fenced").await;
    fence_role(&db).await;
    let fenced = fenced_pool(&db).await;

    let company_a = Uuid::new_v4();
    let company_b = Uuid::new_v4();

    // Seed the OTHER company's blog over the owner pool (the setup
    // path migrations and seeders legitimately use), inside the
    // owner-side scope so the row lands company-scoped.
    let owner = BlogCommandRepository::new(db.pool.clone());
    let seeded = with_company_scope(
        Some(company_b),
        owner.create(&blog_input("company b journal", company_b), None),
    )
    .await
    .unwrap();

    let repo = BlogCommandRepository::new(fenced.clone());

    // (1) THE WALL: a write whose company_id claims a tenant but whose
    //     scope carries none is refused by the policy's WITH CHECK —
    //     the exact failure unscoped repositories produced in the
    //     fenced runtime.
    let refused = match repo.create(&blog_input("unscoped", company_a), None).await {
        Err(BlogError::Database(msg)) => msg,
        other => panic!("unscoped fenced write must fail at the database, got {other:?}"),
    };
    assert!(
        refused.contains("row-level security"),
        "refusal must be the RLS policy violation, got: {refused}"
    );

    // (2) The forgery wall: a scoped write claiming ANOTHER company's
    //     id is refused too — the scope vouches for its own tenant
    //     only.
    let forged = with_company_scope(
        Some(company_a),
        repo.create(&blog_input("forged", company_b), None),
    )
    .await;
    assert!(
        matches!(forged, Err(BlogError::Database(_))),
        "cross-tenant claim must be refused, got {forged:?}"
    );

    // (3) The scoped write passes: the company scope reaches every
    //     statement, so the WITH CHECK admits the row.
    let mine = with_company_scope(
        Some(company_a),
        repo.create(&blog_input("company a journal", company_a), None),
    )
    .await
    .unwrap();

    // (4) Reads stay company-fenced: company A's scope sees ONLY its
    //     own blog — never the other company's rows.
    let visible: Vec<String> = with_company_scope(Some(company_a), repo.list(None, 500))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.name)
        .collect();
    assert_eq!(visible, vec!["company a journal".to_string()]);

    // (5) Cross-company reach is refused: the other company's row is
    //     indistinguishable from a missing one under company A's
    //     scope.
    let cross = with_company_scope(Some(company_a), repo.get(seeded.id)).await;
    assert!(
        matches!(cross, Err(BlogError::NotFound)),
        "cross-company find must not resolve, got {cross:?}"
    );

    // (6) Fail-closed read: with NO company scope the fenced pool
    //     sees zero rows (never the whole table).
    let unscoped = repo.list(None, 500).await.unwrap();
    assert!(
        unscoped.is_empty(),
        "no scope = zero rows under the fence, saw {}",
        unscoped.len()
    );

    // (7) The other company's row survives everything untouched.
    let intact = with_company_scope(Some(company_b), repo.get(seeded.id))
        .await
        .unwrap();
    assert_eq!(intact.id, seeded.id);

    // ── (8) THE TIER FENCE ─────────────────────────────────────────
    // Four posts on company A's blog: one visible, one unpublished
    // draft, one future-dated published, one archived. All created
    // through the FENCED pool under company A's scope (the legitimate
    // admin path).
    let posts = PostCommandService::new(
        PostCommandRepository::new(fenced.clone()),
        Arc::new(RecordingNotifier::new()),
    );
    let website = mine.website_id;
    let mk_post = |slug: &str| {
        (
            slug.to_string(),
            backbone_blog::infrastructure::persistence::post_command_repository::CreatePostInput {
                company_id: company_a,
                blog_id: mine.id,
                title: format!("Post {slug}"),
                slug: slug.to_string(),
                content: Some("<p>fenced</p>".into()),
                teaser: None,
                cover: None,
                author_officer: None,
                author_name: None,
                post_date: chrono::Utc::now(),
                allow_comments: false,
            },
        )
    };
    let (visible_slug, visible_input) = mk_post("fenced-visible");
    let (draft_slug, draft_input) = mk_post("fenced-draft");
    let (future_slug, future_input) = mk_post("fenced-future");
    let (archived_slug, archived_input) = mk_post("fenced-archived");

    let p_visible = with_company_scope(Some(company_a), posts.create(&visible_input, None))
        .await
        .unwrap();
    with_company_scope(Some(company_a), posts.publish(p_visible.id, None))
        .await
        .unwrap();

    let p_draft = with_company_scope(Some(company_a), posts.create(&draft_input, None))
        .await
        .unwrap();

    let p_future = with_company_scope(Some(company_a), posts.create(&future_input, None))
        .await
        .unwrap();
    with_company_scope(
        Some(company_a),
        posts.patch(
            p_future.id,
            &PatchPostInput {
                post_date: Some(chrono::Utc::now() + chrono::Duration::days(1)),
                ..Default::default()
            },
            None,
        ),
    )
    .await
    .unwrap();
    with_company_scope(Some(company_a), posts.publish(p_future.id, None))
        .await
        .unwrap();

    let p_archived = with_company_scope(Some(company_a), posts.create(&archived_input, None))
        .await
        .unwrap();
    with_company_scope(Some(company_a), posts.publish(p_archived.id, None))
        .await
        .unwrap();
    with_company_scope(Some(company_a), posts.archive(p_archived.id, None))
        .await
        .unwrap();

    // The ADMIN binding on the SAME fenced role sees all four.
    for id in [p_visible.id, p_draft.id, p_future.id, p_archived.id] {
        assert!(
            with_company_scope(Some(company_a), posts.get(id))
                .await
                .is_ok(),
            "the admin binding sees post {id}"
        );
    }

    // EVERY public repository method, driven at the public tier,
    // hides the three invisible rows.
    let queries = PublicQueryRepository::new(fenced.clone());
    let page = queries
        .listing(
            company_a,
            website,
            mine.id,
            &ListingQuery {
                page: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.total, 1,
        "the public listing serves exactly the visible post"
    );
    assert_eq!(page.items[0].slug, visible_slug);
    for slug in [&draft_slug, &future_slug, &archived_slug] {
        assert!(
            queries
                .detail(company_a, website, mine.id, slug)
                .await
                .unwrap()
                .is_none(),
            "public detail must hide {slug}"
        );
    }
    assert!(
        queries
            .cloud_public(company_a, website, mine.id, 1)
            .await
            .unwrap()
            .is_empty(),
        "the public cloud counts nothing invisible"
    );

    // The POLICY itself holds the line (not just the query
    // predicates): a raw predicate-free SELECT at the public tier
    // still sees exactly one post of the four.
    let mut tx = fenced.begin().await.unwrap();
    bind_public_scope(&mut tx, company_a).await.unwrap();
    let raw_public: i64 = sqlx::query_scalar("SELECT count(*) FROM blog.posts WHERE blog_id = $1")
        .bind(mine.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        raw_public, 1,
        "the restrictive tier policy hides the invisible rows even from a predicate-free SELECT"
    );
    tx.commit().await.unwrap();

    // ── (9) NO TIER LEAKAGE ────────────────────────────────────────
    // The tier GUC is transaction-local: after the public-tier
    // transaction above committed, an admin-scope read on the SAME
    // pooled connection must see the draft again. (The transaction
    // and this read share one acquired connection on purpose.)
    let mut conn = fenced.acquire().await.unwrap();
    let mut public_tx = conn.begin().await.unwrap();
    bind_public_scope(&mut public_tx, company_a).await.unwrap();
    let during: i64 = sqlx::query_scalar("SELECT count(*) FROM blog.posts WHERE blog_id = $1")
        .bind(mine.id)
        .fetch_one(&mut *public_tx)
        .await
        .unwrap();
    assert_eq!(
        during, 1,
        "inside the public-tier transaction only the visible row"
    );
    public_tx.commit().await.unwrap();

    // The admin read runs the way the runtime runs it: inside the
    // company task-local (the repositories' bind_current_company reads
    // exactly that — a bare call with no task-local binds nothing and
    // RLS correctly hides the world).
    let after: i64 = with_company_scope(Some(company_a), async {
        let mut admin_tx = conn.begin().await.unwrap();
        backbone_orm::company_scope::bind_current_company(&mut admin_tx)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM blog.posts WHERE blog_id = $1")
            .bind(mine.id)
            .fetch_one(&mut *admin_tx)
            .await
            .unwrap();
        admin_tx.commit().await.unwrap();
        n
    })
    .await;
    assert_eq!(
        after, 4,
        "after commit the tier GUC reverted — the admin read sees all four (no leakage)"
    );

    fenced.close().await;
    db.dispose().await;
}
