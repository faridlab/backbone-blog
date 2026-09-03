# backbone-blog

Company-grain blog module for the Metaphor family: blogs, posts, tags,
visit counting, and the publication rules around them. Schema-first
(generated CRUD scaffolding under `metaphor schema generate`), with the
verb layer hand-written on top. `SPEC.md` in this directory is the
authoritative design.

## What the module guarantees

- **Publish is one transaction.** `POST /admin/posts/:id/publish` flips
  `is_published`, stamps `published_date` (a pre-set FUTURE date
  survives the stamp — the lazy-schedule arm), writes exactly ONE
  `post_published` audit row, and fires the publish notifier once. The
  pair `is_published`/`published_date` is refused inside PATCH bodies
  (`blog_field_not_patchable` + a `publish_refused` audit fact) — there
  is exactly one door to the flip.
- **Lazy visibility, zero schedulers.** Every public read composes
  `is_published AND post_date <= now() AND archived_at IS NULL AND not
  soft-deleted` in the query, double-enforced by RESTRICTIVE
  `FOR SELECT` RLS policies keyed on a transaction-local `app.blog_tier`
  GUC. The module declares `read_time_lazy` and arms no cron, no
  scheduler, no background task: a future-dated post becomes visible
  when its moment passes, with nothing running.
- **Tags are company-grain.** Name/slug uniqueness is per company
  (case-folded). Renaming a tag recomputes its slug and records ONE
  redirect through the website surface (WB-3's SEO redirect layer in
  `backbone-website`); unknown slugs resolve through the surface's
  answer seam before refusing.
- **Visits dedupe tokens, not people.** `POST
  /public/posts/:post_slug/visit` mints/verifies an HMAC view token
  (the only identity), dedupes on a unique `(post_id, view_token,
  window_start)` receipt, and increments atomically — concurrent first
  views with one token move the counter exactly once. There is NO
  visitor-profile table anywhere in the schema. Fixed-window throttle:
  60/hour per IP AND per token (429 `blog_throttled` + `Retry-After`).
- **Archive is one-way per object.** Archiving a post forces unpublish;
  unarchiving restores liveness, never re-publishes. Archiving a blog
  cascades marker rows to its LIVE posts only; unarchive restores
  exactly the marker rows. Deleting a blog that still has posts is the
  typed refusal `blog_blog_has_posts`.

## Environment

| Variable | Required | Meaning |
|---|---|---|
| `BLOG_CAPABILITY_SECRET` | yes (visit verb) | HMAC key for view tokens. Empty/unset → the visit verb answers 503 `blog_capability_secret_not_configured` (fail-closed, never mints under an empty key). Public GETs are unaffected. |
| `BLOG_TRUSTED_PROXY` | no | Set to `true` ONLY behind a proxy whose `X-Forwarded-For` is trustworthy. When trusted, the visit verb reads the RIGHTMOST hop; untrusted, it ignores the header and uses the socket address. |

Both must be declared in the host's `.env.*.example` when consumed.

**Gate-order note (deliberate, do not "harmonize"):** every public
handler resolves the Host FIRST and answers the typed 404
`blog_website_not_found` for an unbound host — even while
`BLOG_CAPABILITY_SECRET` is unset (public GETs stay live; only the visit
verb fail-closes on the secret). This is the website-donor idiom and it
fits because the public surface serves READS on bound hosts, so a
binding miss must look like a miss everywhere. backbone-livechat
deliberately orders the other way (uniform pre-resolution 503 while its
secret is unset) because its session-open verb is the anonymous WRITE /
mint entry — see its README. The divergence is the design.

## Mounting

The module does not self-mount. From the host service:

```rust
let blog = BlogModule::builder()
    .with_database(pool.clone())
    .with_publish_notifier(mailing_adapter)      // optional; refusing default parks a WARN + audit row
    .with_website_surface(Some(website_surface)) // optional; slug-redirect recording parks WARNs without it
    .build()?;

// Public tree: mount BARE (no company_auth) — the host binding + the
// tier fence are the gates.
app.merge(blog_public_routes(BlogPublicState::with_secret(
    pool.clone(), website_surface, secret,
)));

// Officer tree: nest behind company_auth + the module-write gate keyed "blog".
app.nest("/api/v1/blog", blog.admin_routes());
```

### Public routes (5 paths, host-header bound)

| Method | Path |
|---|---|
| GET | `/public/blogs` |
| GET | `/public/blogs/:blog_slug/posts` (`?page=&order=recent|visits&tag=&search=`) |
| GET | `/public/blogs/:blog_slug/posts/:post_slug` |
| GET | `/public/blogs/:blog_slug/tags` |
| POST | `/public/posts/:post_slug/visit` |

An unbound Host answers `blog_website_not_found` on all five — no
fallback website.

### Admin routes (behind company_auth)

| Method | Path |
|---|---|
| GET, POST | `/admin/blogs` |
| GET, PATCH, DELETE | `/admin/blogs/:id` |
| POST | `/admin/blogs/:id/archive` · `/admin/blogs/:id/unarchive` |
| GET | `/admin/blogs/:id/tags` |
| GET, POST | `/admin/posts` |
| GET, PATCH | `/admin/posts/:id` |
| POST | `/admin/posts/:id/publish` · `/unpublish` · `/archive` · `/unarchive` |
| PUT | `/admin/posts/:id/tags` |
| GET, POST | `/admin/tags` |
| PATCH, DELETE | `/admin/tags/:id` |
| GET, POST | `/admin/tag-categories` |
| PATCH, DELETE | `/admin/tag-categories/:id` |

## Typed error codes

`blog_not_found`, `blog_website_not_found`, `blog_invalid_input`,
`blog_field_not_patchable`, `blog_throttled`,
`blog_capability_secret_not_configured`, `blog_slug_taken`,
`blog_tag_name_taken`, `blog_blog_has_posts`, `blog_database`,
`blog_internal`.

## Migrations

Run in filename order (the probe suite applies them the same way):

1. `20260426220000` enums
2. `20260426220001` blog
3. `20260426220002` blog_audit_log
4. `20260426220003` post
5. `20260426220004` post_tag (hand; FK repair — see freeze notes)
6. `20260426220005` post_view_receipt (hand; FK repair)
7. `20260426220006` tag_category
8. `20260426220007` tag
9. `20260426220008` enable_company_rls (FORCE + strict policies)
10. `20260426220010` add_audit_triggers (up-only)
11. `20260903000010` blog_hardening_constraints (hand DDL hardening + down)

## Probes

`cargo test` from this directory runs the fail-hard suite: every probe
mints a disposable database on the scratch Postgres (`127.0.0.1:5433`,
`postgres`/`postgres`; override with `BLOG_TEST_ADMIN_URL`), applies
the migrations, and drops it. The suite NEVER touches a live database.
`fenced_runtime` additionally mints a `NOSUPERUSER NOBYPASSRLS` role
and proves the RLS fence binds for real — a superuser run is RLS-blind
by construction.

A missing scratch Postgres PANICS the suite; there are no vacuous
skips.

## Regen-safety notes

See `metaphor.codegen.yaml`. Hand-written files under generator trees
are listed in `user_owned` (declared before landing). Two entries are
GENERATOR-DEFECT FREEZES with unlist conditions documented inline:
`src/lib.rs` (unreliable CUSTOM-marker merge inside the builder
constructor) and the two FK-repaired table migrations. Schema regen is
idempotent: consecutive `metaphor schema generate --force` runs leave
the tree byte-identical (DRIFT=0).
