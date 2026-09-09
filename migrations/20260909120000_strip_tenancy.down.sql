-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- and restores the company-grain tag walls and the company isolation policy shape, but
-- restores NO data — rows written after the strip (or after the decorator re-keyed them)
-- carry org_unit_id only, and with company_id NULL the recreated walls (NULLs are distinct
-- in Postgres unique indexes) and the equality policies fence nothing. The composing
-- service's tenancy decorator remains the live fence; treat this down as a schema-shape
-- sketch for archaeology, not a usable rollback.

ALTER TABLE blog.blogs              ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.blog_audit_log     ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.post_tags          ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.post_view_receipts ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.posts              ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.tag_categories     ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE blog.tags               ADD COLUMN IF NOT EXISTS company_id uuid;

-- The company-grain tag walls (hardening H3) return in shape only; the per-unit twins
-- the composing service's decorator declared on these tables are NOT dropped here —
-- removing those is the composing service's rollback, not this module's.
CREATE UNIQUE INDEX IF NOT EXISTS uq_tags_company_name ON blog.tags (company_id, lower(name));
CREATE UNIQUE INDEX IF NOT EXISTS uq_tags_company_slug ON blog.tags (company_id, slug);

CREATE POLICY blogs_company_isolation ON blog.blogs
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY blog_audit_log_company_isolation ON blog.blog_audit_log
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY post_tags_company_isolation ON blog.post_tags
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY post_view_receipts_company_isolation ON blog.post_view_receipts
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY posts_company_isolation ON blog.posts
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY tag_categories_company_isolation ON blog.tag_categories
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY tags_company_isolation ON blog.tags
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
