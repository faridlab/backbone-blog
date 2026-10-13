-- Down: drop the declared foreign keys for blog module
ALTER TABLE IF EXISTS blog.post_view_receipts DROP CONSTRAINT IF EXISTS fk_post_view_receipts_post_id;
ALTER TABLE IF EXISTS blog.post_tags DROP CONSTRAINT IF EXISTS fk_post_tags_tag_id;
ALTER TABLE IF EXISTS blog.post_tags DROP CONSTRAINT IF EXISTS fk_post_tags_post_id;
ALTER TABLE IF EXISTS blog.posts DROP CONSTRAINT IF EXISTS fk_posts_blog_id;
