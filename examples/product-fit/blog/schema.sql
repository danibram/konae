CREATE TABLE IF NOT EXISTS posts (
  slug TEXT PRIMARY KEY,
  title TEXT,
  summary TEXT NOT NULL,
  cover TEXT NOT NULL,
  body TEXT NOT NULL,
  published INTEGER NOT NULL,
  updated INTEGER NOT NULL
);
