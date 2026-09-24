CREATE TABLE IF NOT EXISTS rooms (
  id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 40),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
  seats INTEGER NOT NULL CHECK (seats > 0)
);
CREATE TABLE IF NOT EXISTS staff (
  user_id TEXT PRIMARY KEY,
  display TEXT NOT NULL CHECK (length(display) BETWEEN 1 AND 60)
);
CREATE TABLE IF NOT EXISTS bookings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  booking_ref TEXT NOT NULL,
  room_id TEXT NOT NULL,
  day TEXT NOT NULL CHECK (date(day) IS day),
  slot INTEGER NOT NULL CHECK (typeof(slot) = 'integer' AND slot BETWEEN 8 AND 17),
  user_id TEXT NOT NULL,
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 80),
  created_at INTEGER NOT NULL,
  cancelled_at INTEGER,
  checked_in_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS bookings_one_holder ON bookings(room_id, day, slot) WHERE cancelled_at IS NULL;
CREATE INDEX IF NOT EXISTS bookings_by_ref ON bookings(booking_ref);
CREATE INDEX IF NOT EXISTS bookings_by_day ON bookings(day, room_id, slot);
