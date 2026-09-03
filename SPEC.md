# backbone-blog — module specification

> Crate `backbone-blog` (version 0.1.0) · Postgres schema `blog` · host route base
> `/api/v1/blog` (schema-name ruling; never `backbone_*`). One crate, one bounded
> context (ADR-0016): blogs, posts, tags, visit counting — the Odoo `website_blog`
> port, deepened. Donor idioms: `backbone-website` v0.1.0 (site-scoped reads, the
> `WebsiteSurface` downstream contract, host binding, publish-contract pattern)
> and `backbone-events` v0.2.1 (typed error surface, RLS LAW discipline, the
> fenced-runtime probe, ports that park loudly, audit rows as durable trace).
>
> Acceptance bar: the five WB-5 register rows — **BLOG-PUBLISH**, **BLOG-LAZY**,
> **BLOG-TAGS**, **BLOG-VISITS**, **BLOG-RULES** (§8 maps each row to its
> mechanism). Cohort source: `docs/odoo/website/blog/` (cycle 20; Odoo 19
> `addons/website_blog`, commit `b9eb72eb`) — prose flags BL-1..BL-14, hook rules
> BL-R1..BL-R9. Everything below that names an upstream behavior cites its flag.

---

## 1. Identity, mount, composition

- **Crate**: `backbone-blog`, `[lib] path = "src/lib.rs"` only — no binary, never
  self-mounts, never self-gates.
- **Schema**: `blog` (crate plural, schema singular — the `backbone-events`/`event`
  relationship). Migrations qualify every table `blog.<table>`; the one enum type
  is created UNQUALIFIED in `public` (the ratified family convention).
- **Host nest** (host-side; the module exports plain routers):

  ```rust
  let blog_gated_admin = blog_admin_routes(state)
      .route_layer(from_fn(seams::blog_compose::blog_actor_bridge))      // innermost-1
      .route_layer(from_fn_with_state(
          middleware::module_write::ModuleWriteGate::new(pool.clone(), "blog"),
          middleware::module_write::module_write_gate))
      .route_layer(from_fn_with_state(verifier.clone(), company_auth)); // OUTSIDE
  modules_router = modules_router.nest("/api/v1/blog",
      blog_gated_admin.merge(blog_public_routes(public_state)));
  ```

  First `route_layer` added = innermost. Public tree merges in **bare** (no
  tenant layer) — the host binding + tier fence + throttle are its only gates.
- **Exports** (`lib.rs`, inside CUSTOM markers where they sit in generated files):
  `BlogModule::builder().with_database(pool).build()`, `all_crud_routes()`
  (unguarded, deprecated `routes()`), `readonly_routes()`,
  `blog_public_routes(BlogPublicState)`, `blog_admin_routes(BlogAdminState)`,
  `BlogActor(pub Uuid)`.
- **Outbox**: none. Hooks index declares `events: {}`; the host's
  `outbox_schemas` carries no `blog` entry. `backbone-messaging` ships only as
  the recorded generator-forced deviation (same note as both donors).
- **Crons/jobs**: **ZERO, module-wide.** No scheduler tables, no `spawn_*`
  functions, nothing registers on the host jobs loop. This is the BLOG-LAZY
  posture made structural (§3, declaration D2) — the seams compose file contains
  no cron spawner and says so in its header.

## 2. Tenancy and the two fences (RLS LAW)

### 2.1 Company fence — `company_fence: strict`

Blog content is unambiguously company-owned: a website belongs to a company, a
blog belongs to a website, a post belongs to a blog. There is no shared master
data and no global/blank arm anywhere in the module. Every table carries
`company_id uuid NOT NULL` (logical ref `organization.companies(id)` —
`@exclude_from_foreign_key_check @indexed @global`, never a cross-schema FK) and
gets the generated strict policy (the events `20260426220018` template minus the
shared-blank NULL arm; emitted by the generator from the declaration):

```sql
ALTER TABLE blog.posts ENABLE ROW LEVEL SECURITY;
ALTER TABLE blog.posts FORCE  ROW LEVEL SECURITY;
CREATE POLICY posts_company_isolation ON blog.posts
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
```

(If the pinned generator's `strict` emission keeps a NULL arm, the hand
hardening migration tightens it to the shape above — the declaration in
`schema/models/index.model.yaml` is the contract either way.)

**Repository LAW** (verbatim events idiom, no exceptions):

- Every transactional write/read: `let mut tx = pool.begin().await?;` then
  `company_scope::bind_current_company(&mut tx).await?;` — first statement after
  `begin()`, every method, every repository.
- Direct-pool statements go through `backbone_orm::company_scope` helpers
  (`fetch_all_scoped`, `fetch_one_scoped`, `fetch_optional_scoped`,
  `fetch_one_scalar_scoped`, `execute_scoped`, …).
- Known-company non-request paths (none exist today — zero crons) would use
  `bind_company_on(&mut conn, company)`.
- Fence claims are proven ONLY under the fenced-runtime probe (§13.7) — never as
  superuser, never on 5432.

### 2.2 Site scope — `website_id` as owned column (BL-1)

Posts keep the donor posture that `website_id` is the scope carrier, but as an
**owned column on both tables** (the blog README faithfulness note 1's second
arm), not a stored-related: `blog.blogs.website_id` and `blog.posts.website_id`
are both `uuid NOT NULL` logical refs to `website.websites(id)` (cross-schema —
indexed, no DB constraint). The `(blog, post)` invariant is enforced by a hand
constraint trigger (§9.3, hardening H1) rather than a related-field compute:

```sql
CREATE CONSTRAINT TRIGGER post_website_matches_blog
AFTER INSERT OR UPDATE OF blog_id, website_id ON blog.posts
DEFERRABLE INITIALLY IMMEDIATE FOR EACH ROW
EXECUTE FUNCTION blog.assert_post_website_matches_blog();
-- raises unless EXISTS (SELECT 1 FROM blog.blogs b
--                      WHERE b.id = NEW.blog_id AND b.website_id = NEW.website_id)
```

Repositories set both columns from the parent blog in one statement; the trigger
is the wall (and the invariant is a fenced-probe claim). `blogs.website_id` is
service-refused for change once any post exists (typed 422 — moving a blog
between websites would silently re-scope its posts, the BL-1 latent behavior;
the port makes that an explicit refusal, not a silent move).

### 2.3 The public-tier fence — BLOG-RULES as a RESTRICTIVE policy

The two Odoo group-scoped `ir.rule`s (public/portal: only `website_published`
posts / `active` blogs — BL-8/BL-R6) port as a **declared, DB-enforced fence**,
not scattered domain predicates. Postgres combines multiple permissive policies
with OR, so a second permissive policy would *widen* visibility — the tier fence
must be **restrictive** (restrictive policies AND against the permissive
result). Hand hardening migration H2:

```sql
CREATE POLICY posts_public_tier_visibility ON blog.posts
    AS RESTRICTIVE FOR SELECT
    USING (
        COALESCE(current_setting('app.blog_tier', true), '') <> 'public'
        OR (is_published AND post_date <= now() AND archived_at IS NULL)
    );

CREATE POLICY blogs_public_tier_visibility ON blog.blogs
    AS RESTRICTIVE FOR SELECT
    USING (
        COALESCE(current_setting('app.blog_tier', true), '') <> 'public'
        OR archived_at IS NULL
    );
```

- The public path binds **both** GUCs transaction-locally (`site_scope.rs`):
  `bind_company_on(&mut tx, website.company_id)` then
  `set_config('app.blog_tier', 'public', true)`. Everything else (admin verbs,
  the host request scope) never sets `app.blog_tier`, so `COALESCE(...,'')`
  yields `'' <> 'public'` → true → the restrictive policy is inert.
- Because the tier is transaction-local (`true` flag), it cannot leak past
  commit on a pooled connection — the probe asserts this.
- Note the fence predicate **includes `post_date <= now()`**: for public-tier
  connections, read_time_lazy (BLOG-LAZY) is DB-enforced even if a repository
  predicate were forgotten. Belt: repositories still compose the predicate
  explicitly (§4.3) so the admin tree can see future rows.
- The post fence does NOT carry the parent-blog liveness check inside the
  policy: posts of an archived blog are already force-archived + unpublished by
  the cascade verb (§4.2), so the row-local predicate is sufficient — the
  invariant is probe-claimed. Blogs' own policy carries `archived_at IS NULL`.
- "Employees see everything" (the Odoo posture choice) stays an explicit
  declaration: the admin tree has **no** visibility predicate — officers of the
  scoped company read all rows, including unpublished, future-dated, and
  archived. It is declared here, probed (§13.7 claim 8), and not an accident.

## 3. Declarations of record

These are the port-time decisions the register demands be explicit. Each is
cited by its acceptance row in §8.

- **D1 — ONE publish event (BLOG-PUBLISH).** The publish coupling is ONE verb,
  ONE transaction, ONE audit row — never three write-override side effects. See
  §4.1.
- **D2 — `read_time_lazy`, ZERO crons (BLOG-LAZY).** A future `post_date` is the
  only scheduling mechanism; nothing ever "fires". Every public read composes
  `post_date <= now()` (repo predicate) AND the tier policy enforces it at the
  DB for public connections. The module owns no scheduler, no job table, no
  `spawn_*`. The events module's self-arming cron posture is explicitly NOT this
  shape. Do not add a scheduler to "fix" lazy publish — that is the one posture
  this module must refuse.
- **D3 — tag uniqueness grain is THE COMPANY (BLOG-TAGS).** Odoo's global
  `unique(name)` is the addon's only 2 SQL constraints and is wrong for
  multi-tenancy: a global unique is both a cross-tenant existence oracle (a
  409 reveals that *someone else's* company has the tag) and a collision
  blocker (two tenants cannot both have "News"). Port grain:
  `UNIQUE (company_id, lower(name))` and `UNIQUE (company_id, slug)` (hardening
  H3). Within one company the vocabulary is shared across that company's blogs
  and websites (Odoo's shared-vocabulary semantics, corrected only at the
  tenancy seam). Reopening trigger (recorded): per-website disjoint vocabularies
  would add `website_id` to both keys — an additive migration, an owner
  decision, not a default.
- **D4 — visits without visitor rows (BLOG-VISITS).** No `blog.visitor`-shaped
  table, no identity attributes, no IP, no user agent, no geoip — the website
  VisitorEngine is NOT consumed (`track_visit` is livechat's seam, WB-8).
  Dedup is a **receipt ledger**: `(post_id, view_token, window_start)` unique,
  one counted view per token per post per UTC day. A receipt is not a visitor
  row: it is a TTL-bounded idempotence ledger in the events-scheduler receipt
  family — no PII, no profile, nothing subject to erasure beyond the token
  itself. The counter is **inflatable by design** (omit the token → a new
  mint → a new count; the register's own finding) — `?order=visits` ("Most
  Viewed") ships knowing this, and the SPEC/README say so.
- **D5 — one-way archive rules (BLOG-PUBLISH).** Archive ⇒ forced unpublish,
  one-way: unarchive restores liveness and NEVER re-publishes. Blog archive
  cascades to posts as **single-statement UPDATEs** inside the blog-archive
  transaction (BL-10's N ORM writes are not ported); the cascade is marker-based
  (`archived_by_blog_id`) so blog-unarchive restores exactly the posts the
  cascade archived — a declared refinement over Odoo's blunt all-posts rewrite.
- **D6 — notify on transition only; reply-downgrade consciously dropped
  (BLOG-PUBLISH).** The parent-blog notification fires only on the `false →
  true` flip (Odoo re-notifies on any redundant write — not ported). The
  BL-R5 reply-downgrade (`mt_note` forcing) is **consciously dropped**: it
  protects a follower inbox that does not exist in backbone; the port's
  notifier is a fixed-recipient, non-blocking port (§7.1) in the website
  intake-notify family. Dropping it is recorded here, not silent.
- **D7 — GET is pure.** Public reads never mutate. Visit counting is an explicit
  `POST /public/posts/{post_slug}/visit` (the SPA fires it), not a side effect
  of the detail read — a declared deviation from Odoo's GET-with-increment that
  keeps reads cacheable and the counter honest about what it counts.
- **D8 — URL parsing never trusts the URL (BLOG-TAGS).** No `_unslug`-style
  trust: tag slugs resolve by scoped lookup (`slug → id` under the bound
  company + website); an unknown slug is the uniform 404, never `browse(None)`
  (BL-5's malformed-slug defect is fixed, declared).
- **D9 — no delete on content.** Posts and blogs have no DELETE verb at v0.1.0:
  retirement is the archive axis (Odoo's own `active` posture — its controllers
  have no content unlink path either). Tags are master data and do carry DELETE
  (untag-everywhere). `metadata.deleted_at` still rides `@audit_metadata` as
  family plumbing; nothing writes it at this version. (Blogs keep a guarded
  empty-blog DELETE, §4.2 — the D9 body is posts.)

### 4.1 BLOG-PUBLISH — the one coupling transaction

`POST /admin/posts/{id}/publish` — the ONLY writer of `is_published` and
`published_date` in the module. One transaction, in order:

1. `begin()` + `bind_current_company`.
2. Guarded flip: `SELECT ... FOR UPDATE` the post; if already published →
   no-op success (200, `{changed: false}`), no stamp, no notify, no audit
   re-emit (D6).
3. `UPDATE blog.posts SET is_published = true,
   published_date = CASE WHEN published_date IS NULL OR published_date <= now()
                        THEN now() ELSE published_date END`
   — **a pre-set future `published_date` survives** (BL-R3's lazy-schedule arm).
   The stamp happens once, here; the patch whitelist refuses both fields
   (`blog_field_not_patchable`, audit `publish_refused`).
4. Audit row `post_published` (detail: stamped `published_date`, whether the
   future value survived, actor). **This audit row IS the `BlogPostPublished`
   event** — the durable trace, one row, not three hooks. BL-R1/R2/R3/R4 do not
   exist as write-interceptors anywhere.
5. Fire `BlogPublishNotifier::post_published(...)` (§7.1) — non-blocking
   posture: a refusal WARNs + audits `notify_parked` and the verb still
   commits (a down mail transport must not unpublish a post).

`POST /admin/posts/{id}/unpublish` sets `is_published = false`, retains
`published_date` (history), audits `post_unpublished`. Idempotent no-op when
already false.

### 4.2 BLOG-PUBLISH — archive verbs (one-way rules)

- `POST /admin/posts/{id}/archive` — one transaction:
  `UPDATE ... SET archived_at = now(), is_published = false WHERE id = $1
   AND archived_at IS NULL` (forced unpublish embedded, D5), audit
  `post_archived` with `detail.forced_unpublish` true iff the flip happened.
- `POST /admin/posts/{id}/unarchive` — `archived_at = NULL` only;
  `is_published` untouched (stays false — the one-way rule), audit
  `post_unarchived` with `detail.republisher = false`.
- `POST /admin/blogs/{id}/archive` — one transaction, single-statement cascade:
  `UPDATE blog.posts SET archived_at = now(), archived_by_blog_id = $blog,
   is_published = false WHERE blog_id = $blog AND archived_at IS NULL`
  (ONE UPDATE — BL-10's 5k-writes shape is not ported), then archive the blog
  row; audit `blog_archived` with `detail.posts_cascade = <rowcount>`.
- `POST /admin/blogs/{id}/unarchive` — restores the blog row and exactly the
  posts whose `archived_by_blog_id` points at it (clearing marker + timestamp);
  posts archived individually before or after stay archived (D5's refinement);
  nothing re-publishes. Audit `blog_unarchived` with restore count.
- `DELETE /admin/blogs/{id}` — hard-delete of an EMPTY blog; any post (archived
  or not) makes the intra-schema RESTRICT FK refuse → typed 409
  `blog_blog_has_posts`.

### 4.3 BLOG-LAZY — read paths and the designer fork

- Public listing/detail/tag-cloud/nav (§5) compose
  `is_published AND post_date <= now() AND archived_at IS NULL` explicitly; the
  tier policy double-enforces it (§2.3). No cron exists to flip anything (D2).
- The designer visibility fork (BL-9) ports as the admin listing's explicit
  `?state=` parameter — **admin-only**; the public tree never sees a state
  split:
  `GET /admin/posts?state=all|published|unpublished&blog_id=` where
  *published* ≡ `is_published AND post_date <= now() AND archived_at IS NULL`
  — a **future-dated published post counts as unpublished in the split**
  (faithful to BL-9's semantics). The response carries live counts
  `{published: n, unpublished: m}` computed future-aware in the same query
  family. Public detail of a future-dated post is the uniform 404 (no oracle).

### 4.4 BLOG-TAGS — grammar and redirects

- Tag create/rename normalize `name` (trim) and derive `slug` (kebab); collisions
  at the company grain are typed 409 `blog_tag_name_taken` (name) /
  `blog_slug_taken` (post or tag slug; (blog, slug) grain for posts).
- Post slug change (PATCH) and tag rename (PATCH) on live content call
  `WebsiteSurface::record_redirect(website_id, from, to, kind)` with the
  website layer's permanent kind — stale-slug 301s live in **WB-3's redirect
  table**, not in blog (the register's own cross-reference). Loop-guard: blog
  never records `from == to` and never records a target that does not resolve.
- Serving: a public read that misses asks `WebsiteSurface::redirect_answer`
  once; an answer → 301 (the webapp follows); no answer → uniform 404.
- Comma-joined tag filter `?tag=slug-a,slug-b` on the listing: canonical set is
  computed from the resolved tags; a stale member resolves via redirect_answer;
  **more than one tag on a GET → 302 to the first tag's canonical URL**
  (BL-R9 faithful). Unknown slug → uniform 404 (D8).
- Tag cloud (BL-6): the raw GROUP BY is **re-expressed as one declared guarded
  query** (never silently): public tier joins the fence, admin joins the full
  set:

  ```sql
  SELECT t.id, t.name, t.slug, count(*) AS post_count
    FROM blog.tags t
    JOIN blog.post_tags pt ON pt.tag_id = t.id
    JOIN blog.posts  p  ON p.id = pt.post_id
   WHERE p.archived_at IS NULL AND p.is_published AND p.post_date <= now()
   GROUP BY t.id HAVING count(*) >= $min_limit
   ORDER BY post_count DESC, t.name;
  ```

  It lives in `public_query_repository.rs` behind a header comment naming BL-6
  and this SPEC section.

### 4.5 BLOG-VISITS — the visit verb

`POST /public/posts/{post_slug}/visit` with optional `{view_token}` — one
transaction after the throttle:

1. Resolve the bound website (host header, `WebsiteSurface`, no fallback) →
   `(website_id, company_id)`; bind public scope (company + tier GUC).
2. Token arm: presented token → verify (Tier A, §6); absent → mint one (random
   nonce in `data`), return it in the response (first view counts — Odoo
   semantics). Bad/expired signature → 422 `blog_invalid_input` (constant-time,
   uniform message) + audit `capability_refused`. Secret unset → 503
   `blog_capability_secret_not_configured` (fail-closed, never mint under an
   empty key).
3. Visibility guard + receipt + increment:

   ```sql
   INSERT INTO blog.post_view_receipts (id, company_id, post_id, view_token, window_start)
   VALUES ($1, $2, $3, $4, date_trunc('day', now()))
   ON CONFLICT (post_id, view_token, window_start) DO NOTHING;
   -- rows == 0  → {counted: false} (commit, done)
   UPDATE blog.posts SET visits = visits + 1
    WHERE id = $3 AND is_published AND post_date <= now() AND archived_at IS NULL;
   -- rows == 0  → uniform blog_not_found (rollback: no receipt oracle)
   ```

   The unique receipt IS the dedup wall. The increment is a **single-statement
   atomic `visits = visits + 1`** — a declared refinement over
   `increment_fields_skiplock` (BL-4/BL-R8): a self-incrementing row UPDATE
   serializes at the row and cannot lose counts, where SKIP LOCKED would skip
   (undercount) under contention; the receipt wall already excludes duplicates.
   The register's "SKIP LOCKED-style concurrency" demand is met by something
   strictly stronger, recorded here.
4. Retention, cronless: the same transaction runs an amortized prune
   (`DELETE ... WHERE window_start < now() - interval '30 days'` gated to ~1%
   of visits by a random predicate) — receipts are bounded without any
   scheduler (D2 holds module-wide).
5. Per-visit audit rows are **deliberately absent** — the counter + receipts
   are the trace (D4); only refusals are audited (`visit_throttled`,
   `capability_refused`).

### 4.6 BLOG-RULES — read fences

Public/portal see exactly the tier policy + the composed predicate
(published-only posts, live-only blogs); officers of the scoped company see
everything (declared §2.3). Comment access (Odoo "read" gate) is moot at this
version — comments are deferred (§15) — and the deferral is recorded so the
gate lands consciously when they do.

## 5. Public read surface (site-scoped, donor posture)

All public reads: resolve host → website (loud typed 404
`blog_website_not_found` on miss, **no fallback to any first website**), bind
public scope, scope every query by `website_id` (the scope carrier) AND the
company fence AND the tier fence. Content misses (unknown slug, wrong blog,
unpublished, future-dated, archived, soft-deleted) are the ONE uniform 404
`blog_not_found` — no oracle, events-style.

- `GET /public/blogs` — live blogs of the website.
- `GET /public/blogs/{blog_slug}/posts` — `?page` (keyset on
  `(post_date, id)`, page size 12 — the Odoo page grain), `?tag=slugs`,
  `?order=recent|visits` (visits = the Most Viewed port, inflatable, D4),
  `?q=` (ILIKE title/teaser/author_name, LIMIT/OFFSET — the Odoo
  limit-page×12-then-slice quirk is NOT ported, declared).
- `GET /public/blogs/{blog_slug}/posts/{post_slug}` — detail + `nav_next`
  (**modulo wrap-around** over the visible set — BL-11's circular tour,
  faithful) + OpenGraph-shaped meta (`article:published_time` = post_date,
  `modified_time` = updated_at, `article:tag` list, og:description = teaser).
  `cover` is read as a structured field (`cover.image_url`) — BL-14's raw
  `[4:-1]` slicing of `url('...')` is NOT ported (declared fix).
- `GET /public/blogs/{blog_slug}/tags` — `?min_limit=` tag cloud (§4.4).
- `POST /public/posts/{post_slug}/visit` — §4.5. Throttled: per-IP fixed window
  `(60, 3600)` AND per-token fixed window `(60, 3600)` — both arms because the
  identity exists; 429 + `Retry-After` + `retry_after_secs` (website Tier B
  `IntakeEngine` book shape, poison-lock fails closed). GETs are unthrottled.

## 6. Tier A capability family + client IP

- `src/application/service/capability.rs` — the events ADR-0018 file copied in
  shape, module-local: context const `CAPABILITY_CONTEXT = "blog-capability-v1"`
  (never events'), token shape `v1.<payload-b64url>.<sig-b64url>`, compact JSON
  claims `{purpose, exp, data}`, HMAC-SHA256 over
  `"<context>\n<purpose>\n<payload>"`, `subtle::ConstantTimeEq` verify,
  hand-rolled b64url no-pad, uniform refusal on every malformed arm, expiry
  after signature, secret never exposed (only `secret_is_configured()`).
- **The one purpose**: `blog-view-token` — mint(secret, nonce, now, 180d),
  `data = [43-char random nonce]`. This is the visit dedup identity (D4). There
  is deliberately **no gated public resource** in blog v0.1.0 (no my-tickets
  analog) — the view token is the honest Tier A surface, and the family ships
  ready for the deferred comments/moderation links.
- Env: `BLOG_CAPABILITY_SECRET` (unset → empty → visit verb answers the typed
  503; boot WARN is the host's; the secret is never printed).
- Client IP: `BLOG_TRUSTED_PROXY` (`true|1|yes|on`, any case, arms the proxy
  posture; unset/other = direct). When armed read the RIGHTMOST
  `X-Forwarded-For` hop; else the socket address; bare IP never `ip:port`;
  fallback `"unknown"`. IP feeds the visit throttle only — never authorization.

## 7. Ports and host uptake

### 7.1 Module ports (process-local, park loudly)

- `notifier_port.rs` — `trait BlogPublishNotifier` +
  `RefusingPublishNotifier` (→ typed `NotifyRefused`). **Non-blocking posture**
  (website notifier family, chosen explicitly): the publish verb commits; a
  refused notify WARNs + audits `notify_parked` and is never auto-retried (the
  audit trail is the retry surface for the officer). Contrast with events'
  template port — that one blocks; this one must not, because publishing is not
  deliverable-contingent.
- **WebsiteSurface consumption** — the ONE sibling module edge:
  `backbone-website = { git = "...", tag = "v0.1.0" }` (tag-equal with the host
  pin, single-resolve). `BlogPublicState` holds `Arc<dyn WebsiteSurface>`; the
  HOST constructs `PgWebsiteSurface::new(pool, pepper)` in the seam (the pepper
  is website's env, not blog's). Used for `resolve_website_by_host`,
  `record_redirect`, `redirect_answer` — exactly the methods the trait docs
  name blog for. `track_visit`/`sweep_visitors` are NOT called by blog
  (livechat's seam, D4).
- Siblings NOT consumed (recorded as Cargo comments): `backbone-mail` (the
  notifier adapter is host-side), `backbone-portal`, `backbone-events`,
  `backbone-livechat`, sapiens/party (logical refs only).

### 7.2 Host side (orchestrator-serialized; this spec's boundary)

- `apps/serpa-service/src/infrastructure/seams/blog_compose.rs`:
  `NotifierAdapter` over backbone-mail; surface construction;
  `blog_actor_bridge` (`CompanyContext` → `Uuid::parse_str(tenant.user_id)` →
  insert `BlogActor`; unresolvable = typed 403 `blog_actor_unresolved`);
  `public_router(pool)` / `admin_router(pool)` with warn-if-unset boot notes;
  **no cron spawners — the header comment says so** (D2). The seams glob is
  already `user_owned`.
- `main.rs` compose + the §1 nest under `/api/v1/blog`.
- Env declarations (compose fail-fast, no silent defaults):
  `BLOG_CAPABILITY_SECRET`, `BLOG_TRUSTED_PROXY` in the matching
  `.env.*.example` templates.
- After dev migrations: run `apps/serpa-service/scripts/rls_app_role.sql` as
  owner or the new `blog` schema locks out `sherpa_app` (the DIT #250 lesson);
  prod twin `deployment/scripts/migrate-with-grants.sh`.
- Cargo pin at uptake: `backbone-blog = { git = "...", tag = "v0.1.0" }`
  (tag pins only, no `[patch]`); workspace `metaphor.yaml` entry + serpa-service
  `depends_on` append; `metaphor sync --update` regenerates the lock.

## 8. Acceptance matrix — register row → mechanism

| Row | Mechanism (this spec) | Probe |
|---|---|---|
| **BLOG-PUBLISH** | §4.1 one-transaction verb (flip + stamp + ONE `post_published` audit + notifier port); §4.2 one-way archive verbs; D5 marker cascade as single UPDATEs; D6 transition-only notify; BL-R5 dropped (D6) | publish_coupling, archive_rules |
| **BLOG-LAZY** | §4.3 predicate in every public read + §2.3 DB-enforced tier fence carrying `post_date <= now()`; D2 zero crons (no scheduler anywhere); admin-only `?state=` fork, future-aware counts | lazy_visibility, fenced_runtime |
| **BLOG-TAGS** | D3 company-grain uniques (H3); §4.4 redirects through WB-3 (`record_redirect`/`redirect_answer`); D8 lookup-not-trust; multi-tag 302; declared guarded cloud query | tag_grammar |
| **BLOG-VISITS** | §4.5 receipt ledger + atomic increment (declared stronger-than-SKIP-LOCKED); D4 no visitor rows; throttle both arms; inflatable-by-design declared; cronless amortized prune | visits_dedup |
| **BLOG-RULES** | §2.3 restrictive public-tier policies (the ir.rule port, DB-enforced); §2.1 strict company RLS + repo LAW; officers-see-all declared; host binding loud 404 | fenced_runtime (claims 1–9) |

## 9. Schema (source of truth) and migrations

### 9.1 `schema/models/index.model.yaml`

`module: blog`, `version: 1`, `schema: blog`, `company_fence: strict` (posture
map in the header comment: every table strict; audit log included — a deliberate
tightening over events' shared audit table, consistent with the strict module
posture), `config:` postgresql / `soft_delete: true` / `audit: true` /
`default_timestamps: true` / generators disabled: graphql, grpc, proto.
`shared_types:` Timestamps/Actors/Metadata exactly as events (actors are plain
nullable logical uuids — no sapiens edge). Imports list mirroring §16's files.
Header carries the enum census duty: `blog_audit_event` is the module's ONLY
enum, unqualified in `public`; census-check every `blog_*`/`*_blog_*` stem
against the tree before the enums migration lands (events already owns
`event_audit_event`; backbone-calendar owns `event_*` families — no collision,
re-verify at codegen).

### 9.2 Models (field-level)

**`blog.model.yaml` — `Blog` (collection `blogs`, `read_only: true`)**

| field | type / attrs | notes |
|---|---|---|
| id | uuid `@id @default(uuid)` | |
| company_id | uuid `@required @indexed @exclude_from_foreign_key_check` | the strict fence column. MUST NOT carry `@global`: in the pinned generator `@global` on the fence column means the table is UNFENCED (no policy emitted at all), silently defeating §2.1. Post-generation correction — this row originally carried `@global` and the emitted RLS proved it wrong. |
| website_id | uuid `@required @indexed @exclude_from_foreign_key_check` | logical ref `website.websites(id)`; service-immutable once posts exist (§2.2) |
| name | string `@required @max(120)` | |
| subtitle | string? `@max(500)` | doubles as description meta (BL-14 subtitle arm) |
| description | string? | free text |
| archived_at | datetime? | the Odoo `active` axis, declared as a timestamp |
| metadata | Metadata `@audit_metadata` | |

**`post.model.yaml` — `Post` (collection `posts`, `read_only: true`)** — the
aggregate's content grain. Fields: `id`; `company_id @required @indexed
@exclude_from_foreign_key_check` (NO `@global` — the attribute unfences the
table; post-generation correction); `blog_id uuid @required
@foreign_key(Blog.blogs) @indexed` (real intra-schema FK, RESTRICT);
`website_id uuid @required @indexed @exclude_from_foreign_key_check` (owned
column + H1 invariant trigger); `title string @required @max(200)`; `slug
string @required @max(120) @indexed` (kebab, service-normalized; mutable via
PATCH which records the redirect); `content string?` (HTML, stored verbatim —
sanitization is the rendering layer's concern, the website `robots_txt`
posture); `teaser string? @max(500)` (og:description); `cover json?`
(structured `{image_url}` — BL-14 fix); `author_officer uuid?
@exclude_from_foreign_key_check` (logical ref); `author_name string? @max(120)`
(plain editable column — the stored-related oddity not ported as a related);
`is_published bool @required @default(false)` + `published_date datetime?` —
**the fence pair: publish/unpublish verbs only, PATCH refusal typed**;
`post_date datetime @required @default(now) @indexed` — the lazy schedule
carrier (BL-3); `visits integer @required @default(0)` (readonly-in-API,
written only by §4.5); `allow_comments bool @required @default(false)` (the
deferred-comments placeholder, §15); `archived_at datetime?`;
`archived_by_blog_id uuid? @exclude_from_foreign_key_check` (the D5 cascade
marker, logical self-ref); `metadata Metadata @audit_metadata`. Indexes:
`(blog_id, post_date desc)`, `(website_id, post_date desc)` (public listing),
`visits` (Most Viewed).

**`tag.model.yaml`** — `TagCategory` (collection `tag_categories`): `id`,
`company_id @required`, `name @required @max(120)`, metadata. `Tag` (collection
`tags`): `id`, `company_id @required @indexed`, `name @required @max(120)` (D3
grain; wall in H3), `slug @required @max(120)` (derived, service-normalized;
wall in H3), `category_id uuid? @foreign_key(TagCategory.tag_categories)
@indexed` (RESTRICT), metadata. No color machinery (Odoo random-color not in
the blog cohort).

**`post_tag.model.yaml` — `PostTag`** (explicit join model, collection
`post_tags`): `post_id @required @foreign_key(Post.posts)`; `tag_id @required
@foreign_key(Tag.tags)`; `company_id @required` (the fence rides the join);
metadata. Unique `(post_id, tag_id)` (H4).

**`post_view_receipt.model.yaml` — `PostViewReceipt`** (collection
`post_view_receipts`, `read_only: true`): `id`, `company_id @required`,
`post_id @required @foreign_key(Post.posts) @indexed`, `view_token string
@required @max(64)`, `window_start datetime @required` (`date_trunc('day')`),
`occurred_at datetime @required @default(now)`. Unique
`(post_id, view_token, window_start)` (H5) — the dedup wall. NOT a visitor row
(D4 — header comment says so).

**`blog_audit_log.model.yaml` — `BlogAuditLog`** (collection `blog_audit_log`,
`read_only: true`, append-only): the events audit shape + `company_id
@required`: `event BlogAuditEvent @required @indexed`, `actor uuid?` logical,
`subject_type string? @max(120)`, `subject_id uuid?`, `detail json?`,
`occurred_at @required @default(now)`; index `(event, occurred_at)`.

**`enums:`** (in `blog_audit_log.model.yaml`) — `BlogAuditEvent`, closed
vocabulary: `blog_created, blog_updated, blog_archived, blog_unarchived,
blog_delete_refused, post_created, post_updated, publish_refused (PATCH carried
the fence pair), post_published (THE one event), notify_parked,
post_unpublished, post_archived, post_unarchived, tag_created, tag_updated
(redirect recorded in detail), tag_deleted, visit_throttled,
capability_refused`. Deliberately absent: `post_visited` (D4 — the counter +
receipts are the trace) and per-miss audit on uniform 404s (reads are not
refused writes).

### 9.3 Migrations (the module's own sequence)

| file | contents |
|---|---|
| `20260426220000_create_enums.{up,down}.sql` | `blog_audit_event` enum (public, census-checked) — generated |
| `20260426220001_create_blog_table.{up,down}.sql` | `blog.blogs` — generated |
| `20260426220002_create_blog_audit_log_table.{up,down}.sql` | generated |
| `20260426220003_create_post_table.{up,down}.sql` | `blog.posts` — generated |
| `20260426220004_create_post_tag_table.{up,down}.sql` | HAND (user-owned slot, generator-shape): the stranded `post_tags -> tags` FK deferred to the hardening migration (generator numbers alphabetically by MODEL name; `tags` sorts last) |
| `20260426220005_create_post_view_receipt_table.{up,down}.sql` | HAND (user-owned slot, generator-shape): no metadata column, no audit-timestamp triggers; FK to posts inline (posts sorts earlier) |
| `20260426220006_create_tag_category_table.{up,down}.sql` | generated |
| `20260426220007_create_tag_table.{up,down}.sql` | generated |
| `20260426220008_enable_company_rls.{up,down}.sql` | strict policies, all 7 tables — generated from `company_fence: strict` |
| `20260426220010_add_audit_triggers.up.sql` | generated (up-only, events/website shape) — the generator emits its own audit-timestamp triggers; NO hand file exists |
| `20260903000010_blog_hardening_constraints.{up,down}.sql` | HAND (user_owned `migrations/*hardening*`): **H1** the `(post.website_id = blog.website_id)` constraint trigger + the blog-move refusal trigger; **H2** the two RESTRICTIVE public-tier policies (§2.3); **H3** `UNIQUE (company_id, lower(name))` + `UNIQUE (company_id, slug)` on tags; **H4** `UNIQUE (post_id, tag_id)`; **H5** receipt wall `UNIQUE (post_id, view_token, window_start)`; **H6** `UNIQUE (blog_id, slug)` on posts `WHERE metadata->>'deleted_at' IS NULL` (slug reserved across archive — restore must not collide); **H7** slug-shape CHECKs (`^[a-z0-9]+(-[a-z0-9]+)*$`, non-empty), `visits >= 0` (already inline in the generated posts DDL via `@non_negative`), `window_start = date_trunc('day', window_start)`. Plus the stranded-FK repair (`post_tags -> tags`). Idempotent (`IF NOT EXISTS`, `DO $$` conname guards); no generator header. |

> Numbering note (post-generation correction): the pinned generator numbers
> first-generation table migrations alphabetically by MODEL name
> (Blog, BlogAuditLog, Post, PostTag, PostViewReceipt, TagCategory, Tag),
> not by the pre-generation guess this table originally carried. The rows
> above state the files as actually emitted.

Later passes: new date prefix, suffix continues past the module's last number,
gaps allowed (the donors' tail pattern).

## 10. Route surface (exhaustive)

**Public** (`blog_public_routes`, mounted bare; doc-commented allowlist — the
negative-enumeration probe target): the five paths of §5, exactly: `GET
/public/blogs`, `GET /public/blogs/{blog_slug}/posts`, `GET
/public/blogs/{blog_slug}/posts/{post_slug}`, `GET
/public/blogs/{blog_slug}/tags`, `POST /public/posts/{post_slug}/visit`.

**Admin** (`blog_admin_routes`; doc header in the events table format):

```
- GET/POST   /admin/blogs                       list (?website_id=) / create
- GET/PATCH  /admin/blogs/{id}                  read / typed patch (website_id
                                               refused while posts exist)
- DELETE     /admin/blogs/{id}                  empty-blog delete (409 while posts)
- POST       /admin/blogs/{id}/archive|unarchive  the cascade verbs (D5)
- GET        /admin/blogs/{id}/tags             admin tag cloud (full set)
- GET/POST   /admin/posts                       list (?state=all|published|unpublished,
                                               ?blog_id=, counts future-aware) / create
- GET/PATCH  /admin/posts/{id}                  read / typed patch (FENCE:
                                               is_published, published_date refused)
- POST       /admin/posts/{id}/publish|unpublish|archive|unarchive
- PUT        /admin/posts/{id}/tags             set the tag id list (resolved, scoped)
- GET/POST   /admin/tags                        list / create (company grain)
- PATCH/DELETE /admin/tags/{id}                 rename (redirect recorded) / untag-everywhere
- GET/POST   /admin/tag-categories              (+ PATCH/DELETE /{id})
```

Patch whitelists: posts — title, slug (records redirect), content, teaser,
cover, post_date (the schedule is officer data), allow_comments,
author_officer, author_name; blogs — name, subtitle, description. Actor via
`BlogActor` extension; the admin verbs never fall back to a public principal.

## 11. Typed error vocabulary (ONE enum)

`src/application/service/blog_error.rs` — one `#[derive(thiserror::Error)] pub
enum BlogError`, stable `code()` wire strings, private `status()`, one
`IntoResponse` (`{"error":{"code","message"}}` + `Retry-After` on throttle),
`From<sqlx::Error> -> Database(String)`, `From<anyhow::Error> ->
Internal(String)`, `pub type BlogResult<T>`. Internal shapes never leak text
(tracing + generic body). Status classes copied from events: 404 not-found
family; 422 domain refusals/validation; 429 throttle; 503 unconfigured secret;
409 busy/conflict; 500 infra.

| code | status | when |
|---|---|---|
| `blog_not_found` | 404 | THE uniform content family: unknown blog/post/slug/tag, wrong blog, unpublished, future-dated (public), archived, cross-tenant — one answer, no oracle |
| `blog_website_not_found` | 404 | host binding miss (loud, no fallback) |
| `blog_invalid_input` | 422 | malformed body/dates/tag list; bad or expired view-token signature (constant-time, uniform message) |
| `blog_field_not_patchable` | 422 | PATCH carried the fence pair (`is_published`/`published_date`) or a refused field; audit `publish_refused` |
| `blog_throttled` | 429 | visit windows exhausted (+ `Retry-After`, `retry_after_secs`); audit `visit_throttled` |
| `blog_capability_secret_not_configured` | 503 | mint/verify under empty `BLOG_CAPABILITY_SECRET` — fail-closed |
| `blog_slug_taken` | 409 | `(blog_id, slug)` wall (posts) or `(company_id, slug)` (tags) |
| `blog_tag_name_taken` | 409 | `(company_id, lower(name))` wall (D3) |
| `blog_blog_has_posts` | 409 | blog delete with any post (the RESTRICT FK, mapped) |
| `blog_database` / `blog_internal` | 500 | infra (never leaking detail) |

## 12. Repository layer

Five hand repositories (`*_command_repository.rs` names distinct from the
generated `*_repository.rs` newtypes; services hold no raw sqlx — the DDD
boundary). Every method opens §2.1's bind; public-query methods open §2.3's
public scope:

- `blog_command_repository.rs` — create/patch/archive cascade/unarchive
  (single-UPDATE + marker)/guarded delete.
- `post_command_repository.rs` — create/patch/publish coupling (§4.1; FOR
  UPDATE guard + CASE stamp)/unpublish/archive pair/tag set.
- `tag_command_repository.rs` — grain-guarded create/rename/delete (untag
  everywhere in one statement), slug derivation support.
- `visit_command_repository.rs` — the receipt/increment/prune SQL of §4.5 (the
  ONE owner of the visit transaction).
- `public_query_repository.rs` — the fence-composed reads: blog list, listing
  (keyset + tag filter + visits order + search), detail + nav wrap, tag cloud,
  state counts (admin family). Every query header-comments its fence arms.

## 13. Probe suite (fail-hard, scratch 5433, NEVER 5432)

`tests/blog_probes.rs` (doc header naming every gate) + `tests/probes/`:
`common/mod.rs` — the events TestDb harness copied: one disposable
`blog_<marker>_<hex>` scratch DB per probe on `SCRATCH_ADMIN_URL =
postgres://postgres:postgres@127.0.0.1:5433/postgres` (env override
`BLOG_TEST_ADMIN_URL`), pre-drop + CREATE, sorted `.up.sql` runner from
`CARGO_MANIFEST_DIR/migrations`, explicit `dispose()` + Drop leak-guard re-drop
`WITH (FORCE)`; `skipped(reason)` PANICS — vacuous skip is a failure;
unreachable scratch panics the suite. Stubs: `RecordingNotifier`,
`RefusingNotifier`, `StubSurface` (records `record_redirect` calls, answers
canned `redirect_answer`s), fixtures `make_blog` /
`make_post{visible,future,archived,draft}`.

1. **`publish_coupling.rs`** — draft→publish: flag flips, `published_date`
   stamped now, ONE `post_published` audit row, notifier called ONCE; pre-set
   future `published_date` survives the stamp; PATCH carrying the fence pair →
   422 + `publish_refused` audit; re-publish → no-op, no re-notify, no
   re-stamp; unpublish retains the date; publish under `RefusingNotifier` →
   commits + `notify_parked`.
2. **`lazy_visibility.rs`** — future-dated published post: excluded from every
   public read (listing, detail = uniform 404, cloud, nav); visible in admin;
   counts as unpublished in the `?state=` split; same row visible publicly after
   `post_date` passes (test shortcut: backdate in a scoped tx); the zero-cron
   claim: module exposes no `spawn_*` and the hooks index declares `events: {}`
   (source census noted in the probe doc).
3. **`tag_grammar.rs`** — duplicate name (case-folded) same company → 409; same
   name other company → OK (D3); rename → slug recompute + ONE
   `record_redirect` via the stub; stale slug resolves through the canned
   answer (301), unknown slug → uniform 404; multi-tag GET → 302-first; cloud
   counts exclude invisible posts.
4. **`visits_dedup.rs`** — same token/day → one count (`counted:false` on the
   second); new token → counts; 25 concurrent first-visits one token → `visits
   == 1`; N distinct tokens → exactly N; invisible post → uniform 404 AND no
   receipt row; schema census: NO table in `blog` matches `%visitor%` (D4);
   throttle burst → 429 + Retry-After.
5. **`archive_rules.rs`** — post archive forces unpublish; unarchive restores
   liveness, `is_published` stays false; blog archive cascades in ONE
   transaction (marker set, counts audited); blog unarchive restores exactly
   marker rows (an individually-archived post stays archived); delete blog with
   posts → 409.
6. **`public_fences.rs`** (mount introspection + allowlist) — the exported
   public router answers exactly the five paths (negative enumeration);
   host-binding miss → `blog_website_not_found`, no fallback; secret unset →
   503 on visit, GETs unaffected.
7. **`fenced_runtime.rs`** — the events pattern copied verbatim in shape: mint
   cluster-unique `blog_probe_app` LOGIN `NOSUPERUSER NOBYPASSRLS` (drop stale
   `blog\_fenced\_%` DBs FIRST), `GRANT USAGE ON SCHEMA blog` + DML all tables +
   sequences; `fenced_pool()`; then, every repository driven through
   `backbone_orm::company_scope::with_company_scope`, assert in order: (1)
   unscoped write naming a company → `BlogError::Database` containing
   "row-level security"; (2) forged cross-tenant write refused; (3) scoped
   write passes; (4) scoped list sees only own rows; (5) cross-company find →
   `blog_not_found`; (6) NO scope → zero rows, never the table; (7) the other
   company's row intact; **(8) the tier fence**: same fenced role, public-tier
   binding → unpublished/future/archived rows invisible through EVERY public
   repo method while the admin binding on the SAME role sees them; **(9) no
   tier leakage**: after a public-tier transaction commits, an admin read on
   the same pooled connection sees the unpublished row (`app.blog_tier`
   reverted with the transaction).

Inline `#[cfg(test)]` unit tests for pure logic: capability mint/verify
(including wrong-purpose, tampered payload, expiry), slug normalization, window
keying.

## 14. Quality gates

- Module suite green on scratch 5433, fail-hard (probes + unit).
- `cargo clippy --all-targets -- -D clippy::expect_used` EXIT=0, run INSIDE
  `modules/backbone-blog/` (never from the metaphora root).
- Schema regen DRIFT=0 (`metaphor schema generate` then clean diff).
- `metaphor.codegen.yaml` `user_owned` declared BEFORE files land:
  `src/application/service/{blog_error,capability,blog_service,post_service,
  tag_service,visit_service,public_query_service,site_scope,notifier_port}.rs`;
  `src/infrastructure/persistence/{blog,post,tag,visit,public_query}_command_repository.rs`;
  `src/presentation/http/{public_routes,admin_routes}.rs`;
  `migrations/*hardening*`; plus the user-owned first-generation table slots
  `migrations/*_create_post_tag_table.{up,down}.sql` and
  `migrations/*_create_post_view_receipt_table.{up,down}.sql` (generator-shape
  hand files; see §9.3); `tests/**`; `README.md`, `SPEC.md`, `docs/**`.
  CUSTOM-marker re-exports in
  `lib.rs` / `application/service/mod.rs` / `exports/services.rs`.

## 15. Explicitly NOT ported (each a recorded decision, never silence)

- BL-R1's N ORM cascade writes → single-UPDATE cascade (D5). BL-R2's
  write-interceptor → declared verb (D5). BL-R3's compute-inverse pair → the
  fence verb's CASE stamp (§4.1). BL-R4's `_check_for_publication` → the ONE
  audit event + port (D1). BL-R5's reply downgrade → dropped (D6).
- BL-5's `_unslug` URL trust → scoped lookup (D8); pre-v14 301s, blog/post
  mismatch 301, single-blog `/blog` 302, default-lang sitemap (BL-11) — webapp
  routing/rendering concerns, outside the module API (the module serves
  slug-addressed reads + the WB-3 redirect seam).
- BL-6 raw-SQL cloud → the declared guarded query (§4.4). BL-7 comments →
  **deferred**; the declared home is website's `IntakeDeclaration` ("blog
  comments" is named in `intake_engine.rs` as a downstream declarer) mounting
  through `execute_intake` — never a second engine; `allow_comments` ships as
  the placeholder. BL-10 → one UPDATE. BL-13 configurator menus → no
  configurator exists in backbone; blog creation is the admin verb. BL-14's
  og:image string-slicing → structured `cover.image_url`. The `author_name`
  stored-related oddity → plain editable column. The `post_date`/`published_date`
  compute-inverse confusion → two declared columns with one contract (§4.1).
  Odoo's GET-with-visit-side-effect → D7 (explicit POST).
- Real-time transport: N/A — blog has no realtime surface; the REST+polling
  preference in the pass context binds livechat, not this module.

## 16. File tree (exact)

```
modules/backbone-blog/
├── Cargo.toml                      # v0.1.0; deps §7.1/§1; framework v2.7.11 tags;
│                                   #   sibling edge backbone-website tag v0.1.0;
│                                   #   NOT-consumed siblings recorded as comments
├── CLAUDE.md                       # the module-type orientation (family template)
├── README.md                       # operator-facing: env vars, mount, declarations
├── SPEC.md                         # this file
├── buf.yaml                        # scaffold parity with events (generator config)
├── config/                         # module-local config scaffold
├── metaphor.codegen.yaml           # user_owned (§14) — declared before files land
├── schema/models/
│   ├── index.model.yaml            # module/schema/strict fence/config/imports
│   ├── blog.model.yaml             # Blog
│   ├── post.model.yaml             # Post (the fence pair, lazy carrier, visits)
│   ├── tag.model.yaml              # TagCategory, Tag (company grain, D3)
│   ├── post_tag.model.yaml         # PostTag join (fenced)
│   ├── post_view_receipt.model.yaml# PostViewReceipt (the dedup wall, not a visitor)
│   └── blog_audit_log.model.yaml   # BlogAuditLog + BlogAuditEvent enum
├── migrations/
│   ├── 20260426220000_create_enums.{up,down}.sql
│   ├── 20260426220001_create_blog_table.{up,down}.sql
│   ├── 20260426220002_create_post_table.{up,down}.sql
│   ├── 20260426220003_create_tag_category_table.{up,down}.sql
│   ├── 20260426220004_create_tag_table.{up,down}.sql
│   ├── 20260426220005_create_post_tag_table.{up,down}.sql
│   ├── 20260426220006_create_post_view_receipt_table.{up,down}.sql
│   ├── 20260426220007_create_blog_audit_log_table.{up,down}.sql
│   ├── 20260426220008_enable_company_rls.{up,down}.sql
│   ├── 20260903000010_blog_hardening_constraints.{up,down}.sql   # HAND H1–H7
│   ├── 20260903000020_add_audit_triggers.up.sql                  # HAND, up-only
│   └── seeds/
├── src/
│   ├── lib.rs                      # BlogModule builder, all_crud_routes,
│   │                               #   readonly_routes + CUSTOM re-exports
│   ├── domain/…  application/dto/… # generated trees
│   ├── application/service/
│   │   ├── blog_error.rs           # ONE typed enum (§11)
│   │   ├── capability.rs           # Tier A, "blog-capability-v1" (§6)
│   │   ├── blog_service.rs         # blog verbs incl. cascade + marker unarchive
│   │   ├── post_service.rs         # create/patch whitelist/publish coupling/rename
│   │   ├── tag_service.rs          # grain-guarded CRUD + redirect recording
│   │   ├── visit_service.rs        # throttle windows + token + receipt flow
│   │   ├── public_query_service.rs # fence-composed reads + state counts
│   │   ├── site_scope.rs           # bind_public_scope (company + app.blog_tier)
│   │   └── notifier_port.rs        # BlogPublishNotifier + Refusing (park loudly)
│   ├── infrastructure/persistence/
│   │   ├── (generated *_repository.rs newtypes)
│   │   ├── blog_command_repository.rs
│   │   ├── post_command_repository.rs
│   │   ├── tag_command_repository.rs
│   │   ├── visit_command_repository.rs
│   │   └── public_query_repository.rs
│   ├── presentation/http/
│   │   ├── public_routes.rs        # the five-path allowlist (§5)
│   │   └── admin_routes.rs         # the officer tree (§10, events table format)
│   └── exports/services.rs         # CUSTOM block re-exports
└── tests/
    ├── blog_probes.rs              # doc header naming every gate
    └── probes/
        ├── mod.rs
        ├── common/mod.rs           # TestDb blog_* on 5433 + stubs + fixtures
        ├── publish_coupling.rs     # BLOG-PUBLISH
        ├── lazy_visibility.rs      # BLOG-LAZY (+ state fork, zero-cron claim)
        ├── tag_grammar.rs          # BLOG-TAGS
        ├── visits_dedup.rs         # BLOG-VISITS
        ├── archive_rules.rs        # the one-way + cascade rules
        ├── public_fences.rs        # allowlist + host binding + secret posture
        └── fenced_runtime.rs       # RLS LAW claims 1–9 under blog_probe_app
```

## 17. Open items for the owner (none block the pin)

- **Tag grain reopening** (D3): per-website vocabularies = additive migration
  adding `website_id` to the two tag walls. Decided company-grain for v0.1.0.
- **Comments** (BL-7): land as a blog `IntakeDeclaration` through website's
  engine when the funnels arm; the gate ("read" access) must be declared then.
- **Generator `strict` emission**: confirm the pinned generator's strict-policy
  SQL drops the NULL arm; if not, the hardening migration tightens it (§2.1
  note). Likewise if the pinned generator numbers an empty enums file
  differently, keep the generator's numbering — the SPEC's table renumbers with
  it; only the date-prefixed hand tails are fixed names.
