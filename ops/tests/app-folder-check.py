#!/usr/bin/env python3
"""Check one offline app folder on an encrypted volume. Emits counts only."""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import re
import resource
import runpy
import shutil
import sqlite3
import stat
import sys
import time

drill = runpy.run_path(str(Path(__file__).resolve().parents[1] / "recovery-drill.py"))
require = drill["require"]


def members(folder):
    paths = [folder / "konae.json", folder / ".konae/deployment.json"]
    for subdir in ["app", "dist", "data/files"]:
        tree = folder / subdir
        if subdir == "data/files" and not tree.exists():
            continue
        require(tree.is_dir() and not tree.is_symlink(), "missing-app-directory")
        for path in tree.rglob("*"):
            require(not path.is_symlink(), "unexpected-link")
            if not path.is_dir():
                paths.append(path)
    for db in ("database.sqlite", "identity.sqlite", "usage.sqlite"):
        for suffix in ("", "-wal", "-journal"):
            path = folder / "data" / (db + suffix)
            if path.exists():
                paths.append(path)
    for path in paths:
        require(path.resolve() == path and stat.S_ISREG(path.lstat().st_mode), "unexpected-file-type")
    return sorted(paths)


def check(folder, binary, expected_sha, report):
    require(binary.name == "kernel", "need-kernel-binary")
    report["binarySha256"] = drill["digest"](binary)
    require(report["binarySha256"] == expected_sha, "binary-digest-mismatch")
    with drill["volume"](report, 2) as root:
        paths = members(folder)
        ident = json.loads((folder / "konae.json").read_bytes())["id"]
        require(re.fullmatch(r"[a-z0-9_-]+", ident), "invalid-app-id")
        app = root / "apps" / ident
        for source in paths:
            target = app / source.relative_to(folder)
            target.parent.mkdir(exist_ok=True, parents=True)
            shutil.copyfile(source, target)
        require(paths == members(folder) and all(drill["digest"](p) == drill["digest"](app / p.relative_to(folder))
                                                for p in paths), "source-changed")
        total = sum((app / p.relative_to(folder)).stat().st_size for p in paths)
        dbs = sorted((app / "data").glob("*.sqlite"))
        for db in dbs:
            with closing(sqlite3.connect(db.as_uri() + "?mode=ro", uri=True)) as connection:
                require(connection.execute("PRAGMA integrity_check").fetchall() == [("ok",)], "sqlite-integrity")
        index_path = app / "data/files/index.json"
        index = json.loads(index_path.read_bytes()) if index_path.exists() else []
        for item in index:
            require(re.fullmatch(r"[0-9a-fA-F-]{36}", item["id"]), "invalid-file-id")
            blob = index_path.parent / (item["id"] + ".blob")
            require(blob.is_file() and blob.stat().st_size == item["size"], "file-index-mismatch")
        report.update(verifiedFiles=len(paths), verifiedBytes=total,
                      checkedDatabases=len(dbs), indexedAssets=len(index))
        env = {"KONAE_DATA_DIR": str(root), "KONAE_EXECUTION_MODE": "test", "TMPDIR": str(root),
               "SQLITE_TMPDIR": str(root)}
        result = drill["restricted"](binary, root, ["drill-check"], env)
        require(result.returncode == 0, "workerless-health-failed")
        loaded = json.loads(result.stdout)
        require(loaded["loadedDeployments"] == 1, "app-did-not-load")
        report.update(workerlessHealth=True, version=loaded["version"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--folder", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--binary-sha256", required=True)
    args = parser.parse_args()
    os.umask(0o077)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    start = time.monotonic()
    report = {"scope": "local-app-folder", "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    try:
        require(sys.platform == "darwin" and os.geteuid() != 0, "need-unprivileged-macos")
        check(args.folder.resolve(strict=True), args.binary.resolve(strict=True), args.binary_sha256, report)
        report["result"] = "passed"
    except Exception as error:
        report.update(result="failed", reason=str(error) if isinstance(error, drill["DrillFailure"]) else type(error).__name__)
    report["durationSeconds"] = round(time.monotonic() - start, 3)
    print(json.dumps(report))
    sys.exit(int(report["result"] != "passed"))
