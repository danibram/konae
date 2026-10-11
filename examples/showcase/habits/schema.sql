CREATE TABLE IF NOT EXISTS habits (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 60),
  created_at INTEGER NOT NULL,
  archived_at INTEGER
);
CREATE INDEX IF NOT EXISTS habits_by_user ON habits(user_id, archived_at);
CREATE TABLE IF NOT EXISTS checkins (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  habit_id INTEGER NOT NULL REFERENCES habits(id),
  user_id TEXT NOT NULL,
  day TEXT NOT NULL CHECK (day GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]' AND date(day) = day),
  device TEXT NOT NULL CHECK (length(device) BETWEEN 1 AND 20),
  created_at INTEGER NOT NULL,
  UNIQUE (habit_id, day)
);
CREATE INDEX IF NOT EXISTS checkins_by_user ON checkins(user_id, day);
CREATE TABLE IF NOT EXISTS days (
  user_id TEXT NOT NULL,
  day TEXT NOT NULL CHECK (day GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]' AND date(day) = day),
  habits INTEGER NOT NULL,
  done INTEGER NOT NULL,
  closed_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, day)
);
