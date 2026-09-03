-- Down: drop blog.blogs table
DROP TABLE IF EXISTS blog.blogs CASCADE;
DROP FUNCTION IF EXISTS blog.blogs_audit_timestamp() CASCADE;
