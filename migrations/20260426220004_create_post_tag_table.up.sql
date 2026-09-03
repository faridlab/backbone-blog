-- Migration: Create post_tags table
-- Hand-written (user-owned slot; see metaphor.codegen.yaml): identical to
-- the generator's shape for this table EXCEPT the stranded FK to
-- blog.tags, which this file's sort position runs BEFORE (tags lands at
-- 20260426220007). The generator numbers table migrations alphabetically
-- by model name and emits FK constraints in the child's own file, so a
-- referenced table sorting later leaves the constraint stranded. The FK
-- is added by the hardening migration (20260903000010), which runs after
-- every table exists — the same repair the events module freezes for its
-- four stranded FKs.

CREATE SCHEMA IF NOT EXISTS blog;

CREATE TABLE IF NOT EXISTS blog.post_tags (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    company_id UUID NOT NULL,
    post_id UUID NOT NULL,
    tag_id UUID NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_post_tags_post_id ON blog.post_tags (post_id);
CREATE INDEX IF NOT EXISTS idx_post_tags_tag_id ON blog.post_tags (tag_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_post_tags_metadata_gin ON blog.post_tags USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_post_tags_metadata_deleted_at ON blog.post_tags ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_post_tags_metadata_created_at ON blog.post_tags ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_post_tags_metadata_updated_at ON blog.post_tags ((metadata->>'updated_at'));

-- Triggers for automatic metadata timestamp management
-- Automatically sets created_at on INSERT and updated_at on UPDATE

-- Function to set metadata->'created_at' on INSERT
CREATE OR REPLACE FUNCTION blog.post_tags_audit_timestamp() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{created_at}', to_jsonb(NOW()));
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    ELSIF TG_OP = 'UPDATE' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger to set timestamps on INSERT
DROP TRIGGER IF EXISTS post_tags_insert_audit ON blog.post_tags;
CREATE TRIGGER post_tags_insert_audit BEFORE INSERT ON blog.post_tags
    FOR EACH ROW EXECUTE FUNCTION blog.post_tags_audit_timestamp();

-- Trigger to set updated_at on UPDATE
DROP TRIGGER IF EXISTS post_tags_update_audit ON blog.post_tags;
CREATE TRIGGER post_tags_update_audit BEFORE UPDATE ON blog.post_tags
    FOR EACH ROW EXECUTE FUNCTION blog.post_tags_audit_timestamp();

-- Inline foreign key constraints (forward + self refs).
-- blog.posts exists at 20260426220003 (sorts earlier) — inline is safe.
-- The FK to blog.tags is added by the hardening migration (see header).
ALTER TABLE blog.post_tags ADD CONSTRAINT fk_post_tags_post_id FOREIGN KEY (post_id) REFERENCES blog.posts (id);
