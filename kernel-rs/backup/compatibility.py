"""Replay the released synthetic copies and compare both operator binaries."""
import argparse
import json
import os
from pathlib import Path
from urllib.parse import urlsplit
import shutil
import socket
import sqlite3
import subprocess
import tarfile
import tempfile
import time
import urllib.request
import uuid

FIXTURES = Path(__file__).resolve().parents[1] / "test-fixtures/copies"
KEY = "isolated-baseline-synthetic-test-only"


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def operator_env(root, extra):
    return {
        "PATH": os.defpath, "AWS_EC2_METADATA_DISABLED": "true", "TOKIO_WORKER_THREADS": "2",
        "KONAE_MAIL_TRANSPORT": "development", "KONAE_DATA_DIR": str(root),
        "KONAE_COPY_HEALTH_DIR": str(root.parent / "health"), **extra,
    }


def compare(binaries, args, destination, extra, success, message, inspect=None):
    previous = None
    for binary in binaries:
        result = subprocess.run([binary, *args], env=operator_env(destination, extra),
                                capture_output=True, text=True, timeout=45)
        output = (result.returncode, result.stdout, result.stderr)
        check((result.returncode == 0) == success and message in result.stdout + result.stderr,
              f"{binary.name} {args[0]} expected {message!r}, got {output}")
        if previous is not None:
            check(output == previous, f"{binary.name} changed its output: {previous!r} -> {output!r}")
        previous = output
        if inspect:
            inspect(destination)
        if not success:
            check(not destination.exists() or all(p.name == "kernel.lock" and p.is_file() and p.stat().st_size == 0 for p in destination.iterdir()), "Failed restore installed files")
        shutil.rmtree(destination, ignore_errors=True)


def inspect_note(root):
    [db] = root.glob("apps/*/data/database.sqlite")
    with sqlite3.connect(f"{db.as_uri()}?mode=ro", uri=True) as conn:
        check(conn.execute("SELECT title FROM notes").fetchall() == [("Before the copy",)], "Missing row")
    [blob] = root.glob("apps/*/data/files/*.blob")
    check(blob.read_bytes() == b"Attachment survives the copy", "Wrong file contents")


def catalog(example, tmp):
    live = tmp / "catalog"
    (live / "assets").mkdir(parents=True)
    asset = bytes(range(256))
    (live / "assets" / "sample").write_bytes(asset)
    with sqlite3.connect(live / "catalog.sqlite") as db:
        db.execute("CREATE TABLE samples(title)")
        db.execute("INSERT INTO samples VALUES ('Plate 1')")
    repo = tmp / "repository"
    def run(*args, ok=True):
        result = subprocess.run([example, *args], capture_output=True, text=True,
                                env={"PATH": os.defpath}, timeout=30)
        check((result.returncode == 0) == ok, result.stderr)
        return result.stdout.strip()
    copy = run("capture", live, repo)
    shutil.rmtree(live)
    run("verify", repo, copy)
    restored = tmp / "album"
    run("restore", repo, copy, restored)
    with sqlite3.connect(f"{(restored / 'catalog.sqlite').as_uri()}?mode=ro", uri=True) as db:
        check(db.execute("SELECT title FROM samples").fetchall() == [("Plate 1",)], "Missing catalog row")
    check((restored / "assets/sample").read_bytes() == asset, "Wrong catalog asset")
    (repo / copy / "assets/sample").write_bytes(asset[::-1])
    run("verify", repo, copy, ok=False)
    run("restore", repo, copy, tmp / "damaged", ok=False)
    check(not (tmp / "damaged").exists(), "Partial catalog restore")
    print("PASS independent catalog capture, verify, restore and damaged asset", flush=True)


def replay(tmp, name, readers, remote):
    suite = tmp / name
    with tarfile.open(FIXTURES / f"{name}.tar.gz") as archive:
        archive.extractall(suite, filter="data")
    objects = suite / "objects/copies"
    latest = sorted(objects.glob("*/manifest.json"))[-1]
    manifest = json.loads(latest.read_bytes())
    copy = latest.parent.name
    version = 1 if name == "v0.9.0" else 2
    check(manifest["version"] == version and manifest.get("mac"), "Wrong S3 fixture format")
    if version == 2:
        older, newer = sorted((suite / "directory").glob("2*"))
        linked = sum((older / entry["path"]).is_file() and
                     (older / entry["path"]).samefile(newer / entry["path"]) for entry in manifest["files"])
        check(linked == 9, f"Expected 9 hardlinks, found {linked}")
        compare(readers["konae"], ["restore", "--from", newer.parent, "--copy", copy],
                tmp / "restore", {}, True, "checksums verified", inspect_note)
        print("PASS directory v1, 9 hardlinks and explicit restore", flush=True)
    if remote is None:
        return
    mc, env, bucket = remote
    prefix = f"compat-{uuid.uuid4().hex}/"
    homes = {f.get("home_copy", copy) for f in manifest["files"]} - {copy}
    check(len(homes) == (3 if version == 2 else 0), "Wrong fixture references")
    for home in homes:
        (objects / home / "manifest.json").unlink()
    target = f"lab/{bucket}/{prefix}"
    subprocess.run([mc, "mirror", objects, target], check=True, stdout=subprocess.DEVNULL)
    args = ["restore", "--from", f"s3://{bucket}/{prefix}", "--copy", copy]
    for profile, binaries in readers.items():
        compare(binaries, args, tmp / "restore", env, profile == "konae",
                "checksums verified" if profile == "konae" else "outside a data directory",
                inspect_note if profile == "konae" else None)
        compare(binaries, args, tmp / "restore", {**env, "KONAE_COPY_SECRET": "x" * 32},
                False, "it is not the secret")
    # Keep all file bytes and change only the MAC field in this disposable prefix.
    del manifest["mac"]
    subprocess.run([mc, "pipe", target + copy + "/manifest.json"], input=json.dumps(manifest).encode(),
                   check=True, stdout=subprocess.DEVNULL)
    for binaries in readers.values():
        compare(binaries, args, tmp / "restore", env, False, "no manifest MAC")
    print(f"PASS S3 v{version}, {len(homes)} expired source manifests, explicit restore, "
          "profile scope, wrong key and missing MAC", flush=True)


def writer(binary, root, remote):
    live, repo = root / "live", root / "copies"
    app = live / "apps/demo"
    (app / "app").mkdir(parents=True)
    (app / "app/sample.txt").write_bytes(b"copy comparison")
    (app / "data").mkdir()
    with sqlite3.connect(app / "data/database.sqlite") as conn:
        conn.execute("CREATE TABLE readings(value)")
        conn.execute("INSERT INTO readings VALUES (23)")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    env = operator_env(live, {
        "KONAE_BIND": "127.0.0.1", "KONAE_KERNEL_PORT": str(port), "KONAE_PUBLIC_ORIGIN": url,
        "KONAE_HOST_SUFFIX": "localhost", "KONAE_SETUP_KEY": "synthetic-writer-setup",
        "KONAE_COPY_DIR": str(repo), "KONAE_COPY_INTERVAL_SECONDS": "1",
        **({**remote[1], "KONAE_COPY_S3": "1"} if remote else {}),
    })
    expected = {"directory", "s3"} if remote else {"directory"}
    with subprocess.Popen([binary], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True) as child:
        try:
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise AssertionError(f"{binary.name} stopped before writing a copy")
                try:
                    with urllib.request.urlopen(url + "/_konae/health", timeout=2) as response:
                        health = json.load(response)["copies"]
                except OSError:
                    time.sleep(.1)
                    continue
                targets = health["targets"]
                check({t["kind"] for t in targets} == expected, "Copy health omitted a target")
                check(all(t["lastError"] is None for t in targets), "Copy health reported failure")
                completed = all(t["lastCopyId"] and not t.get("inProgress")
                                and t["lastSuccessAt"] >= t["lastAttemptAt"] for t in targets)
                if completed and len({t["lastCopyId"] for t in targets}) == 1:
                    check(not health["alert"], "Completed copy still reports an alert")
                    break
                time.sleep(.1)
            else:
                raise AssertionError(f"Timed out waiting for targets {expected}")
        finally:
            child.terminate()
            output, _ = child.communicate(timeout=10)
    copy = targets[0]["lastCopyId"]
    for kind in expected:
        check(f"Copy {copy} sent to the {kind} target" in output, f"Missing {kind} operator message")
    manifest = json.loads((repo / copy / "manifest.json").read_bytes())
    check(manifest["version"] == 1 and manifest["copyId"] == copy, "Writer changed directory format")
    if remote:
        mc, _, bucket = remote
        obj = subprocess.run([mc, "cat", f"lab/{bucket}/copies/{copy}/manifest.json"],
                             capture_output=True, check=True)
        sealed = json.loads(obj.stdout)
        check(sealed["version"] == 2 and sealed["copyId"] == copy and sealed["mac"]
              and sealed["encryption"]["algorithm"] == "AES-256-GCM", "Writer changed S3 format")
        inventory = [{k: v for k, v in f.items() if k != "home_copy"} for f in sealed["files"]]
        check(inventory == manifest["files"], "The two destinations disagree on the captured bytes")
    shutil.rmtree(live)
    return repo, copy


def inspect_reading(root):
    check((root / "apps/demo/app/sample.txt").read_bytes() == b"copy comparison", "Wrong source asset")
    db = root / "apps/demo/data/database.sqlite"
    with sqlite3.connect(f"{db.as_uri()}?mode=ro", uri=True) as conn:
        check(conn.execute("SELECT value FROM readings").fetchall() == [(23,)], "Wrong SQLite row")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", required=True, type=Path)
    parser.add_argument("--baseline-bin-dir", type=Path)
    parser.add_argument("--mc", type=Path)
    parser.add_argument("--require-s3", action="store_true")
    args = parser.parse_args()
    readers = {name: [folder.resolve() / name for folder in [args.baseline_bin_dir, args.bin_dir] if folder]
               for name in ("kernel", "konae")}
    if not args.baseline_bin_dir:
        print("SKIP comparison with af1a897 (pass --baseline-bin-dir)", flush=True)
    remote = None
    if args.mc:
        env = {"KONAE_S3_" + name: os.environ["KONAE_TEST_S3_" + name]
               for name in ("ENDPOINT", "BUCKET", "ACCESS_KEY", "SECRET_KEY")}
        endpoint = urlsplit(env["KONAE_S3_ENDPOINT"])
        alias = urlsplit(os.environ.get("MC_HOST_lab", ""))
        check(endpoint.scheme == alias.scheme == "http" and endpoint.hostname == alias.hostname == "127.0.0.1"
              and endpoint.port == alias.port, "Point mc and the test binaries at the same loopback MinIO")
        remote = args.mc, {**env, "KONAE_COPY_SECRET": KEY}, env["KONAE_S3_BUCKET"]
    else:
        print("SKIP S3 destination (pass --mc inside disposable MinIO)", flush=True)
        if args.require_s3:
            parser.error("S3 destination required")
    with tempfile.TemporaryDirectory(prefix="copy-compat-") as scratch:
        tmp = Path(scratch)
        catalog(args.bin_dir.resolve() / "examples/catalog", tmp)
        for name in ("af1a897", "v0.9.0"):
            replay(tmp, name, readers, remote)
        for profile, binaries in readers.items():
            usage = f"Usage: {profile} restore --from <copy directory | s3://bucket/prefix> [--copy <copy ID>]"
            for binary in binaries:
                suffix = " [--require-revocation <entry SHA-256>]" if binary == binaries[-1] else ""
                compare([binary], ["restore"], tmp / "restore", {}, False, usage + suffix + "\n")
            for index, binary in enumerate(binaries):
                repo, copy = writer(binary, tmp / f"{profile}-{index}", remote)
                sources = [(repo, {})]
                if remote:
                    sources.append((f"s3://{remote[2]}/copies/", remote[1]))
                for source, env in sources:
                    compare(binaries, ["restore", "--from", source, "--copy", copy],
                            tmp / "restore", env, True, "checksums verified", inspect_reading)
                print(f"PASS {profile} writer {index+1}/{len(binaries)}, "
                      f"{len(sources)} targets, {len(binaries)} readers", flush=True)


if __name__ == "__main__":
    main()
