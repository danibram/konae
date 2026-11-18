"""Measure one v3 capture or cold restore per process on macOS."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import shutil
import sqlite3
import subprocess
import tempfile
import time
import uuid

CASES = ("initial", "unchanged", "one-row", "growth", "truncate", "migration", "scattered")
STAMP = 1_700_000_000_000


def change(db, case, rng):
    def append(n):
        db.executemany("INSERT INTO samples(body) VALUES (?)", ((rng.randbytes(1250),) for _ in range(n)))
    if case == "initial":
        db.execute("PRAGMA page_size=4096")
        db.execute("CREATE TABLE samples(id INTEGER PRIMARY KEY, body BLOB)")
        append(1200)
    elif case == "one-row":
        db.execute("UPDATE samples SET body=? WHERE id=600", (rng.randbytes(1250),))
    elif case == "growth":
        append(300)
    elif case == "truncate":
        db.execute("DELETE FROM samples WHERE id>850")
        db.commit()
        db.execute("VACUUM")
    elif case == "migration":
        db.execute("CREATE INDEX by_body ON samples(substr(body,1,24))")
    elif case == "scattered":
        db.executemany("UPDATE samples SET body=? WHERE id=?", ((rng.randbytes(1250), i) for i in range(4, 850, 47)))
    db.commit()


def measure(command, meter):
    command = ["/usr/bin/time", "-lp", *map(str, command)]
    if meter:
        command.insert(0, str(meter))
    started = time.monotonic()
    out = subprocess.run(command, text=True, capture_output=True, timeout=90)
    if out.returncode:
        raise RuntimeError(out.stderr)
    lines = out.stdout.strip().splitlines()
    rss = int(re.search(r"(\d+)\s+maximum resident set size", out.stderr)[1])
    metrics = dict(json.loads(lines[0]), peakRssBytes=rss)
    if meter:
        metrics["s3ServerDeltas"] = json.loads(lines[1])["s3ServerDeltas"]
        metrics["seconds"] = json.loads(lines[1])["seconds"]
    else:
        metrics["seconds"] = round(time.monotonic() - started, 4)
    return metrics


def run(args, root, pages, backend):
    tree = root / "stage"
    db_file = tree / "apps/demo/data/database.sqlite"
    db_file.parent.mkdir(parents=True)
    repository = uuid.uuid4().hex
    dest = "s3" if backend == "s3" else root / "repository"
    meter = args.meter if backend == "s3" else None
    expected = []
    rng = random.Random(42)
    for i, case in enumerate(CASES):
        with sqlite3.connect(db_file) as db:
            change(db, case, rng)
            rows = db.execute("SELECT count(*) FROM samples").fetchone()
        db.close()
        expected.append((hashlib.sha256(db_file.read_bytes()).digest(), rows))
        metrics = measure([args.driver, "publish", dest, repository, tree, STAMP+i, pages], meter)
        if case == "unchanged":
            assert metrics["publication"]["newObjects"] == 0
        print(json.dumps(dict(phase="publish", case=case, pages=pages, backend=backend, **metrics)), flush=True)
    shutil.rmtree(tree)
    for i, case in enumerate(CASES):
        metrics = measure([args.driver, "restore", dest, repository, tree, STAMP+i, pages], meter)
        digest, rows = expected[i]
        assert hashlib.sha256(db_file.read_bytes()).digest() == digest
        with sqlite3.connect(db_file.resolve().as_uri()+"?mode=ro", uri=True) as db:
            assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
            assert db.execute("SELECT count(*) FROM samples").fetchone() == rows
        db.close()
        shutil.rmtree(tree)
        print(json.dumps(dict(phase="restore", case=case, pages=pages, backend=backend, **metrics)), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--driver", required=True, type=Path)
    parser.add_argument("--meter", type=Path, help="minio-measure inside with-minio")
    parser.add_argument("--require-s3", action="store_true")
    parser.add_argument("--pages", nargs="+", default=[4, 16, 64], type=int)
    args = parser.parse_args()
    backends = ["directory"]
    if args.meter and "KONAE_TEST_S3_ENDPOINT" in os.environ:
        backends.append("s3")
    else:
        print("SKIP S3 destination (run with-minio and supply --meter)", flush=True)
        if args.require_s3:
            parser.error("S3 destination required")
    for pages in args.pages:
        for backend in backends:
            with tempfile.TemporaryDirectory(prefix="konae-format-") as tmp:
                run(args, Path(tmp), pages, backend)


if __name__ == "__main__":
    main()
