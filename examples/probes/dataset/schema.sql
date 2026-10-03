CREATE TABLE IF NOT EXISTS stations (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  region TEXT NOT NULL,
  level REAL NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS stations_region ON stations (region, id);
