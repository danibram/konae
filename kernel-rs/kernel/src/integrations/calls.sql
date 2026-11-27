PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS calls (
  id TEXT PRIMARY KEY,
  deployment TEXT NOT NULL,
  action TEXT NOT NULL,
  input TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'failed')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  leased_until INTEGER,
  method TEXT,
  url TEXT,
  status INTEGER,
  error TEXT,
  answer TEXT,
  then_action TEXT,
  then_state TEXT,
  then_message TEXT,
  execution_mode TEXT NOT NULL DEFAULT 'live' CHECK (execution_mode IN ('live', 'test')),
  request_body TEXT,
  request_headers TEXT,
  response_body BLOB,
  traceparent TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS calls_due ON calls(state, next_attempt_at);
CREATE INDEX IF NOT EXISTS calls_deployment ON calls(deployment, created_at);
CREATE TABLE IF NOT EXISTS calls_dispatcher (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  last_deployment TEXT
);
INSERT OR IGNORE INTO calls_dispatcher (singleton, last_deployment) VALUES (1, NULL);
