-- Chores a member assigns to another member by account ID. The reminder run
-- stamps reminded_at, so a chore is mentioned once.
CREATE TABLE IF NOT EXISTS chores (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  assignee_id TEXT NOT NULL,
  due_at INTEGER NOT NULL,
  created_by TEXT NOT NULL,
  reminded_at INTEGER
);
