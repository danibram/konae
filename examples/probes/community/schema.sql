CREATE TABLE IF NOT EXISTS members (
  user_id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'reader' CHECK (role IN ('reader', 'publisher', 'editor'))
);
CREATE TABLE IF NOT EXISTS posts (
  id INTEGER PRIMARY KEY,
  author_id TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft', 'published')),
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS follows (
  user_id TEXT NOT NULL,
  author_id TEXT NOT NULL,
  PRIMARY KEY (user_id, author_id)
);
