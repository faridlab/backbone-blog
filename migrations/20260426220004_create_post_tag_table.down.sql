-- Down: drop blog.post_tags table
DROP TABLE IF EXISTS blog.post_tags CASCADE;
DROP FUNCTION IF EXISTS blog.post_tags_audit_timestamp() CASCADE;
