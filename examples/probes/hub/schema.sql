CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  ref TEXT NOT NULL,
  detail TEXT,
  received_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS orders (
  id TEXT PRIMARY KEY,
  item TEXT NOT NULL,
  payment_id TEXT,
  paid_at INTEGER,
  created_at INTEGER NOT NULL
);
