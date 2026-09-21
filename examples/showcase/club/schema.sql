CREATE TABLE IF NOT EXISTS members (user_id TEXT PRIMARY KEY, display TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'organizer')), joined_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS books (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, author TEXT NOT NULL, cover_file_id TEXT, proposed_by TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'proposed' CHECK (status IN ('proposed', 'current', 'finished')), meets_on TEXT, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, book_id INTEGER NOT NULL, file_id TEXT NOT NULL, name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS notes_by_user ON notes(user_id, created_at);
