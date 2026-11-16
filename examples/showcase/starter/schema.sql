CREATE TABLE IF NOT EXISTS profiles (
  user_id TEXT PRIMARY KEY NOT NULL,
  display_name TEXT NOT NULL CHECK (length(trim(display_name)) BETWEEN 1 AND 40),
  role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'admin', 'owner')),
  avatar_file_id TEXT,
  joined_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS profiles_by_role ON profiles(role, display_name);

CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
  body TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_by_user ON notes(user_id, id);

CREATE TABLE IF NOT EXISTS team_posts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(trim(body)) BETWEEN 1 AND 2000),
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS updates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
  body TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL CHECK (source IN ('team', 'program')),
  author_id TEXT NOT NULL,
  published_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS status (
  component TEXT PRIMARY KEY NOT NULL CHECK (length(component) BETWEEN 1 AND 40),
  state TEXT NOT NULL CHECK (state IN ('operational', 'degraded', 'down')),
  detail TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS escalations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  component TEXT NOT NULL,
  changed_at INTEGER NOT NULL,
  status INTEGER,
  filed INTEGER,
  error TEXT,
  answered_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS attachments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  file_id TEXT NOT NULL,
  name TEXT NOT NULL,
  size INTEGER NOT NULL CHECK (size >= 0),
  public INTEGER NOT NULL DEFAULT 0 CHECK (public IN (0, 1)),
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS attachments_file_id_unique ON attachments(file_id);
CREATE INDEX IF NOT EXISTS attachments_by_user ON attachments(user_id, id);
CREATE INDEX IF NOT EXISTS attachments_public ON attachments(public, id);
