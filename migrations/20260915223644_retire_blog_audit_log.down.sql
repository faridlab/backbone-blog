-- Recreate the retired shell so a rollback leaves a schema the old code can
-- start against. It comes back EMPTY, which is what it was — the rows that
-- matter are in `auditlog.audit_trails` and are not moved back, because
-- nothing was moved out.
CREATE TYPE blog_audit_event AS ENUM ('placeholder');

CREATE TABLE IF NOT EXISTS blog.blog_audit_log (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    event        blog_audit_event NOT NULL,
    actor        text,
    subject_type text,
    subject_id   uuid,
    detail       jsonb,
    occurred_at  timestamptz NOT NULL DEFAULT now()
);
