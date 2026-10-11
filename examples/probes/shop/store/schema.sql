CREATE TABLE IF NOT EXISTS items (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  stock INTEGER NOT NULL CHECK (stock >= 0),
  hold_ms INTEGER NOT NULL DEFAULT 900000,
  price TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS orders (
  id INTEGER PRIMARY KEY,
  item_id INTEGER NOT NULL REFERENCES items (id),
  user_id TEXT NOT NULL,
  qty INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'held' CHECK (status IN ('held', 'paid')),
  session_id INTEGER,
  payment_id TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS orders_item ON orders (item_id, status, created_at);
