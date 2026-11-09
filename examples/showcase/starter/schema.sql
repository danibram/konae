-- One row per person who joined. The role lives here, in the app's rows:
-- the Kernel knows who is signed in, and the app decides what each role may
-- do. Everyone joins as member; the Konae owner names the app's owner with
-- SQL in Data: UPDATE profiles SET role = 'owner' WHERE user_id = '<account id>';
CREATE TABLE IF NOT EXISTS profiles (
  user_id TEXT PRIMARY KEY NOT NULL,
  display_name TEXT NOT NULL CHECK (length(trim(display_name)) BETWEEN 1 AND 40),
  role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'admin', 'owner')),
  avatar_file_id TEXT,
  joined_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS profiles_by_role ON profiles(role, display_name);

-- Private to the member who wrote them.
CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
  body TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_by_user ON notes(user_id, id);

-- Read by admins and the owner only.
CREATE TABLE IF NOT EXISTS team_posts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(trim(body)) BETWEEN 1 AND 2000),
  created_at INTEGER NOT NULL
);

-- The public landing and GET /api/updates. author_id is a user ID or an API key ID.
CREATE TABLE IF NOT EXISTS updates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
  body TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL CHECK (source IN ('team', 'program')),
  author_id TEXT NOT NULL,
  published_at INTEGER NOT NULL
);

-- Written only by the signed webhook.
CREATE TABLE IF NOT EXISTS status (
  component TEXT PRIMARY KEY NOT NULL CHECK (length(component) BETWEEN 1 AND 40),
  state TEXT NOT NULL CHECK (state IN ('operational', 'degraded', 'down')),
  detail TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);

-- What the team inbox answered to each status the team told it about. Written
-- only by status.escalated, which the Kernel runs after its call answers.
CREATE TABLE IF NOT EXISTS escalations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  component TEXT NOT NULL,
  changed_at INTEGER NOT NULL,
  status INTEGER,
  filed INTEGER,
  error TEXT,
  answered_at INTEGER NOT NULL
);

-- A member's file, private until they publish it. The bytes live with the Kernel.
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
