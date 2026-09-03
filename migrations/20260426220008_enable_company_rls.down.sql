-- Down: remove the company RLS fence for blog module

-- Reverse the company RLS fence for blog.blogs
DROP POLICY IF EXISTS blogs_company_isolation ON blog.blogs;
ALTER TABLE blog.blogs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.blogs DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.blog_audit_log
DROP POLICY IF EXISTS blog_audit_log_company_isolation ON blog.blog_audit_log;
ALTER TABLE blog.blog_audit_log NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.blog_audit_log DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.posts
DROP POLICY IF EXISTS posts_company_isolation ON blog.posts;
ALTER TABLE blog.posts NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.posts DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.post_tags
DROP POLICY IF EXISTS post_tags_company_isolation ON blog.post_tags;
ALTER TABLE blog.post_tags NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.post_tags DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.post_view_receipts
DROP POLICY IF EXISTS post_view_receipts_company_isolation ON blog.post_view_receipts;
ALTER TABLE blog.post_view_receipts NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.post_view_receipts DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.tag_categories
DROP POLICY IF EXISTS tag_categories_company_isolation ON blog.tag_categories;
ALTER TABLE blog.tag_categories NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.tag_categories DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for blog.tags
DROP POLICY IF EXISTS tags_company_isolation ON blog.tags;
ALTER TABLE blog.tags NO FORCE ROW LEVEL SECURITY;
ALTER TABLE blog.tags DISABLE ROW LEVEL SECURITY;

