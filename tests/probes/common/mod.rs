//! Shared harness: one DISPOSABLE scratch database per probe,
//! FAIL-HARD (the events-module suite's contract, verbatim in shape).
//!
//! The suite never runs against a shared database (and NEVER against
//! the live dev database on 5432): each probe mints
//! `blog_probe_<marker>_<hex>` on the local scratch Postgres
//! (127.0.0.1:5433 — the pinned scratch container), applies this
//! module's migrations with a raw SQL file runner, runs, and drops
//! the database.
//!
//! FAIL-HARD CONTRACT: a probe that cannot reach its scratch
//! database PANICS — [`TestDb::new`] refuses to return `None`, and
//! [`skipped`] panics on principle. A green suite means the behaviors
//! were exercised, not that they were unreachable.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// The scratch Postgres every probe database is born on and dropped
/// from. 127.0.0.1:5433 — the pinned scratch container, NEVER a live
/// service database.
pub const SCRATCH_ADMIN_URL: &str = "postgres://postgres:postgres@127.0.0.1:5433/postgres";

/// The probe capability secret (explicit, never from the environment
/// — probes must not depend on host configuration).
pub const PROBE_SECRET: &str = "blog-probe-capability-secret";

/// The probe host the StubSurface binds.
pub const PROBE_HOST: &str = "probe.example.test";

fn admin_url() -> String {
    std::env::var("BLOG_TEST_ADMIN_URL").unwrap_or_else(|_| SCRATCH_ADMIN_URL.into())
}

/// The fail-hard skip: reaching this is a FAILURE, never a green
/// tick.
pub fn skipped(reason: &str) -> ! {
    panic!("VACUOUS SKIP IS A FAILURE: {reason}");
}

/// One disposable scratch database, migrations applied. Panics
/// (never returns `None`) when the scratch Postgres is unreachable.
pub struct TestDb {
    pub pool: PgPool,
    name: String,
    admin: PgPool,
}

impl TestDb {
    pub async fn new(marker: &str) -> Self {
        let url = admin_url();
        let admin = match PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: admin connect to {url} failed: {e}");
                skipped(&format!("scratch Postgres unreachable: {e}"));
            }
        };
        let suffix: String = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
        let name = format!("blog_probe_{marker}_{suffix}");
        // Disposable by construction: a stale DB of the same name goes
        // first.
        if let Err(e) = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#))
            .execute(&admin)
            .await
        {
            eprintln!("PROBE-FAIL: {marker}: pre-drop of {name} failed: {e}");
            skipped(&format!("scratch pre-drop failed: {e}"));
        }
        if let Err(e) = sqlx::query(&format!(r#"CREATE DATABASE "{name}""#))
            .execute(&admin)
            .await
        {
            eprintln!("PROBE-FAIL: {marker}: create database {name} failed: {e}");
            skipped(&format!("scratch create failed: {e}"));
        }
        let db_url = match url.rfind('/') {
            Some(i) => format!("{}{}", &url[..=i], name),
            None => url.clone(),
        };
        let pool = match PgPoolOptions::new()
            .max_connections(12)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db_url)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: connect to {db_url} failed: {e}");
                skipped(&format!("scratch connect failed: {e}"));
            }
        };
        if let Err(what) = apply_module_migrations(&pool, marker).await {
            skipped(&what);
        }
        Self { pool, name, admin }
    }

    /// Explicit teardown: drop the scratch database entirely.
    pub async fn dispose(self) {
        self.drop_db().await;
    }

    async fn drop_db(&self) {
        // FORCE: the connected probe pool may still hold an idle
        // session.
        let _ = sqlx::query(&format!(
            r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#,
            self.name
        ))
        .execute(&self.admin)
        .await;
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let name = self.name.clone();
        let url = admin_url();
        // Leak-guard teardown for panicking probes; dispose() is the
        // happy path.
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    if let Ok(admin) = sqlx::PgPool::connect(&url).await {
                        let _ = sqlx::query(&format!(
                            r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#
                        ))
                        .execute(&admin)
                        .await;
                    }
                });
            }
        });
    }
}

/// Apply this module's migrations with a raw SQL file runner (sorted
/// `.up.sql` order — the module's files are self-contained).
async fn apply_module_migrations(pool: &PgPool, marker: &str) -> Result<(), String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{manifest}/migrations");
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".up.sql"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(e) => return Err(format!("PROBE-FAIL: {marker}: cannot read {dir}: {e}")),
    };
    files.sort();
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("PROBE-FAIL: {marker}: cannot acquire pool conn: {e}"))?;
    for file in files {
        let sql = std::fs::read_to_string(&file)
            .map_err(|e| format!("PROBE-FAIL: {marker}: cannot read {}: {e}", file.display()))?;
        if let Err(e) = sqlx::raw_sql(&sql).execute(&mut *conn).await {
            return Err(format!(
                "PROBE-FAIL: {marker}: migration {} failed: {e}",
                file.display()
            ));
        }
    }
    Ok(())
}

// ── shared stubs ───────────────────────────────────────────────────────────

use async_trait::async_trait;
use std::sync::Mutex;

use backbone_blog::application::service::blog_service::BlogCommandService;
use backbone_blog::application::service::post_service::PostCommandService;
use backbone_blog::infrastructure::persistence::blog_command_repository::{
    BlogCommandRepository, BlogRow, CreateBlogInput,
};
use backbone_blog::infrastructure::persistence::post_command_repository::{
    CreatePostInput, PostCommandRepository,
};
use backbone_orm::company_scope::with_company_scope;

pub use backbone_blog::application::service::notifier_port::RecordingNotifier;

/// The stub website surface: binds ONE host to ONE website view,
/// records every `record_redirect` call, and answers `redirect_answer`
/// from a canned list the probe programs.
pub struct StubSurface {
    pub view: backbone_website::exports::WebsiteView,
    /// Every recorded redirect: (website_id, url_from, url_to, kind).
    pub redirects: Mutex<Vec<(Uuid, String, String, String)>>,
    /// Canned answers keyed by the asked url.
    pub answers: Mutex<Vec<(String, RedirectAnswer)>>,
}

impl StubSurface {
    /// A surface binding `PROBE_HOST` to the given website view.
    pub fn binding(view: backbone_website::exports::WebsiteView) -> Self {
        Self {
            view,
            redirects: Mutex::new(Vec::new()),
            answers: Mutex::new(Vec::new()),
        }
    }

    /// Program one canned answer.
    pub fn answer(&self, url: &str, redirect_type: &str, url_to: Option<&str>) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push((
                url.to_string(),
                RedirectAnswer {
                    redirect_type: redirect_type.to_string(),
                    url_to: url_to.map(str::to_string),
                },
            ));
        }
    }

    /// The recorded redirect calls, in order.
    pub fn recorded(&self) -> Vec<(Uuid, String, String, String)> {
        self.redirects.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

/// The redirect answer shape (not part of the module's export list;
/// the lang_matcher path is public and canonical).
pub use backbone_website::application::service::lang_matcher::RedirectAnswer;

use backbone_website::exports::{
    MenuNode, PublicPage, SessionFacts, SweepSummary, WebsiteError, WebsitePrincipal,
    WebsiteResult, WebsiteView,
};

#[async_trait]
impl backbone_website::exports::WebsiteSurface for StubSurface {
    async fn resolve_website_by_host(&self, host: &str) -> WebsiteResult<WebsiteView> {
        if host.eq_ignore_ascii_case(PROBE_HOST) {
            Ok(self.view.clone())
        } else {
            Err(WebsiteError::WebsiteNotResolved)
        }
    }

    async fn visible_page(
        &self,
        _website_id: Uuid,
        _url: &str,
        _principal: Option<&WebsitePrincipal>,
    ) -> WebsiteResult<Option<PublicPage>> {
        Ok(None)
    }

    async fn menu_tree_visible(
        &self,
        _website_id: Uuid,
        _principal: Option<&WebsitePrincipal>,
    ) -> WebsiteResult<Vec<MenuNode>> {
        Ok(Vec::new())
    }

    async fn redirect_answer(
        &self,
        _website_id: Uuid,
        url: &str,
    ) -> Option<backbone_website::application::service::lang_matcher::RedirectAnswer> {
        self.answers.lock().ok().and_then(|answers| {
            answers
                .iter()
                .find(|(asked, _)| asked == url)
                .map(|(_, answer)| answer.clone())
        })
    }

    async fn record_redirect(
        &self,
        website_id: Uuid,
        url_from: &str,
        url_to: &str,
        kind: backbone_website::exports::RedirectKind,
    ) -> WebsiteResult<()> {
        if let Ok(mut redirects) = self.redirects.lock() {
            redirects.push((
                website_id,
                url_from.to_string(),
                url_to.to_string(),
                kind.to_string(),
            ));
        }
        Ok(())
    }

    async fn company_allowlist(
        &self,
        _principal: &WebsitePrincipal,
        _website_id: Uuid,
    ) -> Vec<Uuid> {
        Vec::new()
    }

    async fn track_visit(
        &self,
        _website_id: Uuid,
        _session: &SessionFacts<'_>,
        _url: &str,
        _page_key: Option<&str>,
    ) -> WebsiteResult<()> {
        Ok(())
    }

    async fn sweep_visitors(&self) -> WebsiteResult<SweepSummary> {
        Ok(SweepSummary {
            swept: 0,
            batches: 0,
        })
    }
}

// ── shared fixtures ────────────────────────────────────────────────────────

/// A fresh (company, website) pair plus the probe website view bound
/// to `PROBE_HOST`.
pub fn probe_tenancy() -> (Uuid, backbone_website::exports::WebsiteView) {
    let company = Uuid::new_v4();
    let view = backbone_website::exports::WebsiteView {
        id: Uuid::new_v4(),
        name: "Probe Website".to_string(),
        domain: Some(PROBE_HOST.to_string()),
        company_id: company,
        public_user_id: Uuid::new_v4(),
        default_lang_code: "en".to_string(),
        homepage_url: "/".to_string(),
        robots_txt: None,
        social_links: None,
        contact_recipients: Vec::new(),
        sequence: 1,
    };
    (company, view)
}

/// Create a blog through the verb service (inside the owner-side
/// company scope, the legitimate setup path).
pub async fn make_blog(db: &TestDb, company: Uuid, website_id: Uuid, name: &str) -> BlogRow {
    let service = BlogCommandService::new(BlogCommandRepository::new(db.pool.clone()));
    with_company_scope(
        Some(company),
        service.create(
            &CreateBlogInput {
                company_id: company,
                website_id,
                name: name.to_string(),
                subtitle: None,
                description: None,
            },
            None,
        ),
    )
    .await
    .unwrap_or_else(|e| panic!("probe fixture: blog create failed: {e:?}"))
}

/// The post verb service over the probe pool with the recording
/// notifier.
pub fn posts_with(
    db: &TestDb,
    notifier: std::sync::Arc<
        dyn backbone_blog::application::service::notifier_port::BlogPublishNotifier,
    >,
) -> PostCommandService {
    PostCommandService::new(PostCommandRepository::new(db.pool.clone()), notifier)
}

/// A plain post-create input (draft, post_date now).
pub fn post_input(company: Uuid, blog_id: Uuid, title: &str, slug: &str) -> CreatePostInput {
    CreatePostInput {
        company_id: company,
        blog_id,
        title: title.to_string(),
        slug: slug.to_string(),
        content: Some("<p>probe body</p>".to_string()),
        teaser: Some("probe teaser".to_string()),
        cover: None,
        author_officer: None,
        author_name: Some("Probe Officer".to_string()),
        post_date: chrono::Utc::now(),
        allow_comments: false,
    }
}

/// A DRAFT post (created, never published).
pub async fn make_post_draft(db: &TestDb, company: Uuid, blog_id: Uuid, slug: &str) -> uuid::Uuid {
    let service = posts_with(db, std::sync::Arc::new(RecordingNotifier::new()));
    let row = with_company_scope(
        Some(company),
        service.create(&post_input(company, blog_id, "Probe Post", slug), None),
    )
    .await
    .unwrap_or_else(|e| panic!("probe fixture: post create failed: {e:?}"));
    row.id
}

/// A VISIBLE post (created + published, post_date now).
pub async fn make_post_visible(
    db: &TestDb,
    company: Uuid,
    blog_id: Uuid,
    slug: &str,
) -> uuid::Uuid {
    let service = posts_with(db, std::sync::Arc::new(RecordingNotifier::new()));
    let row = with_company_scope(
        Some(company),
        service.create(&post_input(company, blog_id, "Probe Post", slug), None),
    )
    .await
    .unwrap();
    with_company_scope(Some(company), service.publish(row.id, None))
        .await
        .unwrap_or_else(|e| panic!("probe fixture: publish failed: {e:?}"));
    row.id
}

/// A FUTURE post (published now, post_date a day out — the lazy arm:
/// is_published is true, the row is invisible until post_date passes).
pub async fn make_post_future(db: &TestDb, company: Uuid, blog_id: Uuid, slug: &str) -> uuid::Uuid {
    let service = posts_with(db, std::sync::Arc::new(RecordingNotifier::new()));
    let mut input = post_input(company, blog_id, "Probe Future Post", slug);
    input.post_date = chrono::Utc::now() + chrono::Duration::days(1);
    let row = with_company_scope(Some(company), service.create(&input, None))
        .await
        .unwrap();
    with_company_scope(Some(company), service.publish(row.id, None))
        .await
        .unwrap();
    row.id
}

/// An ARCHIVED post (created, published, then archived — the forced
/// unpublish).
pub async fn make_post_archived(
    db: &TestDb,
    company: Uuid,
    blog_id: Uuid,
    slug: &str,
) -> uuid::Uuid {
    let service = posts_with(db, std::sync::Arc::new(RecordingNotifier::new()));
    let row = with_company_scope(
        Some(company),
        service.create(
            &post_input(company, blog_id, "Probe Archived Post", slug),
            None,
        ),
    )
    .await
    .unwrap();
    with_company_scope(Some(company), service.publish(row.id, None))
        .await
        .unwrap();
    with_company_scope(Some(company), service.archive(row.id, None))
        .await
        .unwrap();
    row.id
}

/// Create a tag through the verb service (company grain).
pub async fn make_tag(
    db: &TestDb,
    company: Uuid,
    name: &str,
) -> backbone_blog::infrastructure::persistence::tag_command_repository::TagRow {
    use backbone_blog::application::service::tag_service::TagCommandService;
    use backbone_blog::infrastructure::persistence::tag_command_repository::TagCommandRepository;
    let service = TagCommandService::new(TagCommandRepository::new(db.pool.clone()));
    with_company_scope(Some(company), service.create(company, name, None, None))
        .await
        .unwrap_or_else(|e| panic!("probe fixture: tag create failed: {e:?}"))
}

/// Attach tags to a post through the set-tags verb (scoped ids).
pub async fn tag_post(db: &TestDb, company: Uuid, post_id: Uuid, tag_ids: &[uuid::Uuid]) {
    let service = posts_with(db, std::sync::Arc::new(RecordingNotifier::new()));
    with_company_scope(Some(company), service.set_tags(post_id, tag_ids, None))
        .await
        .unwrap_or_else(|e| panic!("probe fixture: set_tags failed: {e:?}"));
}

/// Count audit rows for one event (the census the coupling probes
/// assert against).
pub async fn audit_count(db: &TestDb, event: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM blog.blog_audit_log WHERE event = $1::blog_audit_event",
    )
    .bind(event)
    .fetch_one(&db.pool)
    .await
    .unwrap_or_else(|e| panic!("probe fixture: audit census failed: {e}"))
}

/// A company-scoped setup write: the probe's own setup SQL, run with
/// the scope bound first (the same discipline the repositories hold;
/// the superuser probe pool bypasses nothing worth trusting — every
/// setup write binds the tenant it writes for).
pub async fn scoped_exec(db: &TestDb, company: Uuid, sql: &str) {
    let mut tx = db
        .pool
        .begin()
        .await
        .unwrap_or_else(|e| panic!("probe fixture: scoped_exec begin: {e}"));
    backbone_orm::company_scope::bind_company_on(&mut tx, company)
        .await
        .unwrap_or_else(|e| panic!("probe fixture: scoped_exec bind: {e}"));
    sqlx::query(sql)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("probe fixture: scoped_exec run: {e}"));
    tx.commit()
        .await
        .unwrap_or_else(|e| panic!("probe fixture: scoped_exec commit: {e}"));
}

/// Pre-set a FUTURE `published_date` on a post (the lazy-schedule arm
/// the publish stamp must PRESERVE, not overwrite).
pub async fn preset_future_published_date(db: &TestDb, company: Uuid, post_id: Uuid) {
    scoped_exec(
        db,
        company,
        &format!(
            "UPDATE blog.posts SET published_date = now() + interval '1 day' \
             WHERE id = '{post_id}'"
        ),
    )
    .await;
}

/// The lazy-visibility test shortcut: backdate one post's `post_date`
/// past now inside a scoped transaction (SPEC section 4.3's sanctioned
/// shortcut — the row becomes publicly visible with NO cron).
pub async fn backdate_post(db: &TestDb, company: Uuid, post_id: Uuid) {
    scoped_exec(
        db,
        company,
        &format!(
            "UPDATE blog.posts SET post_date = now() - interval '1 second' \
             WHERE id = '{post_id}'"
        ),
    )
    .await;
}
