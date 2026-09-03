-- Hand-written DDL hardening for the blog module.
--
-- Carries the walls the schema DSL cannot express (SPEC section 9.3,
-- H1-H7) plus the ONE stranded-FK repair. Idempotent throughout: guards
-- on every object so a re-run is a no-op (the module migration runner
-- tracks applied files; the guards protect the probe suite, which builds
-- the schema fresh and applies this file once).
--
-- Posture notes:
--  - H2's tier policies are RESTRICTIVE: Postgres ANDs them against the
--    PERMISSIVE company-fence policy from 20260426220008. The gate key is
--    the transaction-local GUC `app.blog_tier`; UNSET (or any value other
--    than 'public') means an officer/admin connection, where the tier arm
--    is TRUE and visibility falls back to the company fence alone. The
--    read-time lazy-publish arm (`post_date <= now()`) lives INSIDE the
--    posts policy — the database enforces the lazy schedule on every
--    public read, not just the ones that remember the predicate.
--  - The tier policies cover FOR ALL: the WITH CHECK arm defaults to the
--    USING expression, which is exactly right for the visit verb — an
--    increment may only touch a post that is publicly visible under the
--    fence (an unpublished/future post can never gain visits through the
--    public tier).
--  - The receipt table deliberately carries NO tier policy: receipts are
--    write-only ledger rows inside the company fence, never publicly
--    readable (no SELECT path exists at all — the visit verb is the only
--    writer, INSERT + prune DELETE).

-- ==============================================================================
-- Stranded-FK repair (generator defect, the events-module pattern): the
-- pinned generator numbers table migrations alphabetically by model name
-- and emits FK constraints in the child's own file. post_tags (20260426220004)
-- sorts BEFORE tags (20260426220007), so the generator's FK to blog.tags
-- cannot live in the post_tags file — it lands here, after every table
-- exists.
-- ==============================================================================
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'fk_post_tags_tag_id'
    ) THEN
        ALTER TABLE blog.post_tags
            ADD CONSTRAINT fk_post_tags_tag_id FOREIGN KEY (tag_id) REFERENCES blog.tags (id);
    END IF;
END
$$;

-- ==============================================================================
-- H1 — the (post.website_id = blog.website_id) invariant, both directions.
-- ==============================================================================

-- H1a: a post may never disagree with its blog's website. Constraint
-- trigger (not a plain CHECK) because the invariant spans two rows.
CREATE OR REPLACE FUNCTION blog.assert_post_website_matches_blog() RETURNS trigger AS $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM blog.posts p
          JOIN blog.blogs b ON b.id = p.blog_id
         WHERE p.id = NEW.id
           AND p.website_id IS DISTINCT FROM b.website_id
    ) THEN
        RAISE EXCEPTION 'post website_id must equal its blog website_id'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS posts_website_matches_blog ON blog.posts;
CREATE CONSTRAINT TRIGGER posts_website_matches_blog
    AFTER INSERT OR UPDATE OF blog_id, website_id ON blog.posts
    FOR EACH ROW
    EXECUTE FUNCTION blog.assert_post_website_matches_blog();

-- H1b: a blog's website is service-immutable once posts exist. The guard
-- is a BEFORE trigger on blogs (not a post-service check) so no writer —
-- generated CRUD included — can move a loaded blog.
CREATE OR REPLACE FUNCTION blog.refuse_blog_website_move() RETURNS trigger AS $$
BEGIN
    IF NEW.website_id IS DISTINCT FROM OLD.website_id
       AND EXISTS (SELECT 1 FROM blog.posts WHERE blog_id = NEW.id) THEN
        RAISE EXCEPTION 'blog website_id is immutable while posts exist'
            USING ERRCODE = '23510';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS blogs_refuse_website_move ON blog.blogs;
CREATE TRIGGER blogs_refuse_website_move
    BEFORE UPDATE OF website_id ON blog.blogs
    FOR EACH ROW
    EXECUTE FUNCTION blog.refuse_blog_website_move();

-- ==============================================================================
-- H2 — the public-tier visibility fences (the two group-scoped record rules
-- as restrictive RLS policies; SPEC section 4.6).
-- ==============================================================================

-- Posts: published only, lazy schedule honored, not archived, not
-- soft-deleted (the soft-delete arm is SPEC section 5's uniform-miss
-- rule made DB-level; nothing writes deleted_at at this version, the
-- arm is the belt).
DROP POLICY IF EXISTS posts_public_tier_visibility ON blog.posts;
CREATE POLICY posts_public_tier_visibility ON blog.posts
    AS RESTRICTIVE
    FOR SELECT
    USING (
        COALESCE(current_setting('app.blog_tier', true), '') <> 'public'
        OR (
            is_published
            AND post_date <= now()
            AND archived_at IS NULL
            AND metadata->>'deleted_at' IS NULL
        )
    );

-- Blogs: active only, not soft-deleted.
DROP POLICY IF EXISTS blogs_public_tier_visibility ON blog.blogs;
CREATE POLICY blogs_public_tier_visibility ON blog.blogs
    AS RESTRICTIVE
    FOR SELECT
    USING (
        COALESCE(current_setting('app.blog_tier', true), '') <> 'public'
        OR (
            archived_at IS NULL
            AND metadata->>'deleted_at' IS NULL
        )
    );

-- ==============================================================================
-- H3 — tag uniqueness at the COMPANY grain (the Odoo global unique(name)
-- corrected at the tenancy seam: a global wall would be a cross-tenant
-- existence oracle and a collision blocker).
-- ==============================================================================
CREATE UNIQUE INDEX IF NOT EXISTS uq_tags_company_name
    ON blog.tags (company_id, lower(name))
    WHERE metadata->>'deleted_at' IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_tags_company_slug
    ON blog.tags (company_id, slug)
    WHERE metadata->>'deleted_at' IS NULL;

-- ==============================================================================
-- H4 — the join wall: a tag applies to a post at most once.
-- ==============================================================================
CREATE UNIQUE INDEX IF NOT EXISTS uq_post_tags_pair
    ON blog.post_tags (post_id, tag_id)
    WHERE metadata->>'deleted_at' IS NULL;

-- ==============================================================================
-- H5 — the visit dedup wall: one counted view per (post, token) per UTC
-- day. THE seat of session dedup (no visitor rows exist anywhere).
-- ==============================================================================
CREATE UNIQUE INDEX IF NOT EXISTS uq_post_view_receipts_wall
    ON blog.post_view_receipts (post_id, view_token, window_start);

-- ==============================================================================
-- H6 — the slug reservation: a live post's slug is unique within its blog
-- (soft-deleted rows keep their reservation released).
-- ==============================================================================
CREATE UNIQUE INDEX IF NOT EXISTS uq_posts_blog_slug
    ON blog.posts (blog_id, slug)
    WHERE metadata->>'deleted_at' IS NULL;

-- ==============================================================================
-- H7 — shape CHECKs (visits >= 0 is already inline in the generated posts
-- DDL via @non_negative; restated here for the census).
-- ==============================================================================
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'ck_posts_slug_shape'
    ) THEN
        ALTER TABLE blog.posts
            ADD CONSTRAINT ck_posts_slug_shape
            CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$');
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'ck_tags_slug_shape'
    ) THEN
        ALTER TABLE blog.tags
            ADD CONSTRAINT ck_tags_slug_shape
            CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$');
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'ck_post_view_receipts_window_start_day'
    ) THEN
        ALTER TABLE blog.post_view_receipts
            ADD CONSTRAINT ck_post_view_receipts_window_start_day
            CHECK (window_start = date_trunc('day', window_start));
    END IF;
END
$$;
