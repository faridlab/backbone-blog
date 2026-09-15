-- Retire this module's own audit table.
--
-- Audited facts now land on `auditlog.audit_trails`, the shared trail the
-- record-history and activity-feed surfaces read. Every write site in this
-- module was redirected there, and the module no longer declares the entity,
-- so nothing reads or writes this table.
--
-- No data is moved because there is none: the table was empty in every
-- environment checked. Had it held rows, this migration would have carried
-- them over instead of dropping them.
DROP TABLE IF EXISTS blog.blog_audit_log;

-- The closed event vocabulary went with it. `audit_trails.action` is text, so
-- the same event names survive as values; the type constraint does not.
DROP TYPE IF EXISTS blog_audit_event;
