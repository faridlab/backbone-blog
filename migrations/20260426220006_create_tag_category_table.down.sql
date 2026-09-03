-- Down: drop blog.tag_categories table
DROP TABLE IF EXISTS blog.tag_categories CASCADE;
DROP FUNCTION IF EXISTS blog.tag_categories_audit_timestamp() CASCADE;
