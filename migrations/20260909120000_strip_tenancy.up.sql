-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the blog tables (ADR-0029): the module is
-- tenant-agnostic; org scoping is installed by the COMPOSING service's tenancy decorator,
-- never by the module. Dropped here, per table: the company-leading indexes, the
-- <table>_company_isolation RLS policy, and the company_id column itself.
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. A table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.
--
-- The tag uniqueness walls (uq_tags_company_name / uq_tags_company_slug, H3) are
-- company-leading and drop here; a composing service that org-scopes these tables
-- re-declares their per-unit twins in its tenancy decorator. The non-company walls of
-- the hardening migration (uq_post_tags_pair, uq_post_view_receipts_wall,
-- uq_posts_blog_slug, the H7 CHECKs, the H1 triggers) involve no tenancy column and
-- stay untouched.

DO $$
DECLARE
    t text;
    has_org boolean;
    org_nulls bigint;
    total bigint;
    offenders text := '';
BEGIN
    FOREACH t IN ARRAY ARRAY['blogs', 'blog_audit_log', 'post_tags', 'post_view_receipts', 'posts', 'tag_categories', 'tags']
    LOOP
        IF to_regclass(format('blog.%I', t)) IS NULL THEN
            CONTINUE; -- chain not fully applied on this database; nothing to strip
        END IF;

        SELECT EXISTS (
                   SELECT 1 FROM information_schema.columns
                   WHERE table_schema = 'blog' AND table_name = t AND column_name = 'org_unit_id'
               )
        INTO has_org;

        EXECUTE format('SELECT count(*) FROM blog.%I', t) INTO total;

        IF has_org THEN
            EXECUTE format(
                'SELECT count(*) FROM blog.%I WHERE org_unit_id IS NULL', t)
            INTO org_nulls;
        ELSE
            org_nulls := total; -- no org column: every row's only tenancy key is company_id
        END IF;

        IF has_org AND org_nulls = 0 THEN
            CONTINUE; -- decorator backfilled: safe
        END IF;
        IF total = 0 THEN
            CONTINUE; -- empty table (fresh database): safe
        END IF;
        offenders := offenders || format(' blog.%s (%s rows, %s rows not covered by org_unit_id);', t, total, org_nulls);
    END LOOP;

    IF offenders <> '' THEN
        RAISE EXCEPTION 'refusing to strip company_id — these tables are not yet covered by the tenancy decorator:%. Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', offenders;
    END IF;
END $$;

-- ── blogs ──────────────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS blogs_company_isolation ON blog.blogs;
ALTER TABLE blog.blogs DROP COLUMN IF EXISTS company_id;

-- ── blog_audit_log ─────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS blog_audit_log_company_isolation ON blog.blog_audit_log;
ALTER TABLE blog.blog_audit_log DROP COLUMN IF EXISTS company_id;

-- ── post_tags ──────────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS post_tags_company_isolation ON blog.post_tags;
ALTER TABLE blog.post_tags DROP COLUMN IF EXISTS company_id;

-- ── post_view_receipts ─────────────────────────────────────────────────────────
DROP POLICY IF EXISTS post_view_receipts_company_isolation ON blog.post_view_receipts;
ALTER TABLE blog.post_view_receipts DROP COLUMN IF EXISTS company_id;

-- ── posts ──────────────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS posts_company_isolation ON blog.posts;
ALTER TABLE blog.posts DROP COLUMN IF EXISTS company_id;

-- ── tag_categories ─────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS tag_categories_company_isolation ON blog.tag_categories;
ALTER TABLE blog.tag_categories DROP COLUMN IF EXISTS company_id;

-- ── tags ───────────────────────────────────────────────────────────────────────
-- The company-grain tag walls (hardening H3) lead on company_id; the composing
-- service's tenancy decorator re-declares their per-unit twins.
DROP INDEX IF EXISTS blog.uq_tags_company_name;
DROP INDEX IF EXISTS blog.uq_tags_company_slug;
DROP POLICY IF EXISTS tags_company_isolation ON blog.tags;
ALTER TABLE blog.tags DROP COLUMN IF EXISTS company_id;
