CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY,
  user_id TEXT NOT NULL,
  body TEXT NOT NULL,
  remind_at INTEGER NOT NULL,
  surfaced_at INTEGER
);
CREATE INDEX IF NOT EXISTS notes_due ON notes(surfaced_at, remind_at);
