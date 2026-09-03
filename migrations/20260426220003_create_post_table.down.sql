-- Down: drop blog.posts table
DROP TABLE IF EXISTS blog.posts CASCADE;
DROP FUNCTION IF EXISTS blog.posts_audit_timestamp() CASCADE;
