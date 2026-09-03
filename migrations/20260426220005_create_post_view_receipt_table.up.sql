-- Migration: Create post_view_receipts table
-- Hand-written (user-owned slot; see metaphor.codegen.yaml), mirroring the
-- generator's shape for this table. The referenced table blog.posts sorts
-- EARLIER (20260426220003), so its FK is inline like the generator would
-- emit it. This table carries NO metadata column (it is a ledger, not a
-- CRUD row) — no audit-timestamp triggers, matching the generator's
-- behavior for tables without audit metadata fields (blog_audit_log).

CREATE SCHEMA IF NOT EXISTS blog;

CREATE TABLE IF NOT EXISTS blog.post_view_receipts (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    company_id UUID NOT NULL,
    post_id UUID NOT NULL,
    view_token TEXT NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_post_view_receipts_post_id ON blog.post_view_receipts (post_id);

-- The dedup-wall probe index: the visit verb's INSERT .. ON CONFLICT DO
-- NOTHING (post_id, view_token, window_start) and the amortized prune
-- (window_start < now - retention) both walk this shape (the wall itself
-- is the hardening migration's partial-unique index).
CREATE INDEX IF NOT EXISTS idx_post_view_receipts_window_start ON blog.post_view_receipts (window_start);

-- Inline foreign key constraints (forward + self refs)
ALTER TABLE blog.post_view_receipts ADD CONSTRAINT fk_post_view_receipts_post_id FOREIGN KEY (post_id) REFERENCES blog.posts (id);
