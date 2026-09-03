-- Down: drop blog.tags table
DROP TABLE IF EXISTS blog.tags CASCADE;
DROP FUNCTION IF EXISTS blog.tags_audit_timestamp() CASCADE;
