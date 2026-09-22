CREATE TABLE IF NOT EXISTS studio (user_id TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS photos (id INTEGER PRIMARY KEY AUTOINCREMENT, file_id TEXT NOT NULL, title TEXT NOT NULL, caption TEXT, status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')), created_at INTEGER NOT NULL, published_at INTEGER);
CREATE INDEX IF NOT EXISTS photos_by_status ON photos(status, published_at);
CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, email TEXT NOT NULL, topic TEXT NOT NULL CHECK (topic IN ('commission', 'print', 'other')), body TEXT NOT NULL, received_at INTEGER NOT NULL);
