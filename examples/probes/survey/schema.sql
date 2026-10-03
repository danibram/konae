CREATE TABLE IF NOT EXISTS answers (
  response_key TEXT NOT NULL,
  question TEXT NOT NULL,
  answer TEXT NOT NULL,
  submitted_at INTEGER NOT NULL,
  PRIMARY KEY (response_key, question)
);
CREATE TABLE IF NOT EXISTS ballots (
  user_id TEXT PRIMARY KEY,
  choice TEXT NOT NULL CHECK (choice IN ('tea', 'coffee'))
);
