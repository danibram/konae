CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source TEXT NOT NULL,
  external_id TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  received_at INTEGER NOT NULL,
  acknowledged_by TEXT,
  acknowledged_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS events_source_external ON events(source, external_id);
CREATE INDEX IF NOT EXISTS events_received ON events(received_at);
CREATE TABLE IF NOT EXISTS members (
  user_id TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  seen_at INTEGER NOT NULL
);
