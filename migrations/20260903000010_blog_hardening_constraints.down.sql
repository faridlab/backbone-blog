-- Down: remove the blog module's hand DDL hardening (reverse of
-- 20260903000010, in reverse dependency order).

-- H7 shape CHECKs
ALTER TABLE blog.post_view_receipts DROP CONSTRAINT IF EXISTS ck_post_view_receipts_window_start_day;
ALTER TABLE blog.tags DROP CONSTRAINT IF EXISTS ck_tags_slug_shape;
ALTER TABLE blog.posts DROP CONSTRAINT IF EXISTS ck_posts_slug_shape;

-- H6 slug reservation
DROP INDEX IF EXISTS blog.uq_posts_blog_slug;

-- H5 receipt wall
DROP INDEX IF EXISTS blog.uq_post_view_receipts_wall;

-- H4 join wall
DROP INDEX IF EXISTS blog.uq_post_tags_pair;

-- H3 company-grain tag uniques
DROP INDEX IF EXISTS blog.uq_tags_company_slug;
DROP INDEX IF EXISTS blog.uq_tags_company_name;

-- H2 public-tier fences
DROP POLICY IF EXISTS blogs_public_tier_visibility ON blog.blogs;
DROP POLICY IF EXISTS posts_public_tier_visibility ON blog.posts;

-- H1 website invariant
DROP TRIGGER IF EXISTS blogs_refuse_website_move ON blog.blogs;
DROP FUNCTION IF EXISTS blog.refuse_blog_website_move();
DROP TRIGGER IF EXISTS posts_website_matches_blog ON blog.posts;
DROP FUNCTION IF EXISTS blog.assert_post_website_matches_blog();

-- Stranded-FK repair
ALTER TABLE blog.post_tags DROP CONSTRAINT IF EXISTS fk_post_tags_tag_id;
