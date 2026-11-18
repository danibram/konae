"""An independent synthetic fixture for the sensitive SQLite capture rule."""
import argparse
from pathlib import Path
import sqlite3

marker = 'DELETED-VALUE-FIXTURE-42-WITHOUT-REAL-DATA'
parser = argparse.ArgumentParser()
parser.add_argument('mode', choices=['seed', 'verify'])
parser.add_argument('file', type=Path)
args = parser.parse_args()
if args.mode == 'seed':
    # The caller supplies a fresh lab path. Keep the previous value in free pages on purpose.
    args.file.parent.mkdir(parents=True, exist_ok=True)
    with args.file.open('xb'):
        pass
    with sqlite3.connect(args.file) as db:
        db.execute('PRAGMA page_size = 4096')
        db.execute('PRAGMA secure_delete = OFF')
        db.execute("CREATE TABLE secrets(name TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL, use TEXT NOT NULL)")
        db.executemany("INSERT INTO secrets VALUES(?,?,1,'header')",
                       [('retained', 'LIVE-FIXTURE-42'), ('removed', marker * 6000)])
        db.commit()
        db.execute("DELETE FROM secrets WHERE name='removed'")
    db.close()
    assert marker.encode() in args.file.read_bytes()
    print('Synthetic input has one live value and deleted bytes in free pages.')
else:
    with sqlite3.connect(args.file.resolve(strict=True).as_uri()+'?mode=ro', uri=True) as db:
        assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
        assert db.execute('SELECT name,value FROM secrets').fetchall() == [('retained','LIVE-FIXTURE-42')]
    db.close()
    assert marker.encode() not in args.file.read_bytes(), 'The recovered image contains deleted bytes'
    print('Sensitive image passed integrity and retained its one live value with zero deleted markers.')
