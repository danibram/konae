import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

export function openDatabases(directory) {
  mkdirSync(directory, { recursive: true });
  return new Map(['a', 'b'].map((app) => {
    const db = new DatabaseSync(resolve(directory, `${app}.sqlite`));
    db.exec(`
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      PRAGMA busy_timeout = 5000;
      CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 10));
      CREATE TABLE IF NOT EXISTS sessions (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS audit (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, actor_id TEXT NOT NULL, quantity INTEGER NOT NULL);
      INSERT OR IGNORE INTO orders VALUES (1, 'alice', 2), (2, 'bob', 4);
    `);
    const seed = db.prepare('INSERT OR IGNORE INTO sessions VALUES (?, ?, ?)');
    seed.run(`${app}-alice`, 'alice', 4102444800);
    seed.run(`${app}-bob`, 'bob', 4102444800);
    seed.run(`${app}-expired`, 'alice', 1);
    const session = db.prepare('SELECT user_id FROM sessions WHERE token = ? AND expires_at > ? LIMIT 1');
    const read = db.prepare('SELECT id, owner_id AS ownerId, quantity FROM orders WHERE id = ? AND owner_id = ? LIMIT 1');
    const update = db.prepare('UPDATE orders SET quantity = ? WHERE id = ? AND owner_id = ? RETURNING id, owner_id AS ownerId, quantity');
    const audit = db.prepare('INSERT INTO audit (order_id, actor_id, quantity) VALUES (?, ?, ?)');
    return [app, {
      db,
      authenticate(token) {
        return session.get(token, Math.floor(Date.now() / 1000))?.user_id ?? null;
      },
      forActor(userId) {
        return {
          readOrder(id) { return read.get(id, userId) ?? null; },
          updateOrder(id, quantity) {
            db.exec('BEGIN IMMEDIATE');
            try {
              const row = update.get(quantity, id, userId);
              if (row) audit.run(id, userId, quantity);
              db.exec('COMMIT');
              return row ?? null;
            } catch (error) {
              db.exec('ROLLBACK');
              throw error;
            }
          },
        };
      },
    }];
  }));
}
