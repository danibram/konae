"""Exercise the Kernel executable on a disposable directory and a loopback port."""
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="konae-38-process-") as tmp:
    root = Path(tmp)
    live = root / "live"
    copies = root / "copies"
    files = live / "apps" / str(uuid.uuid4()) / "data" / "files"
    files.mkdir(parents=True)
    file_id = str(uuid.uuid4())
    body = b"sixteen-fixtures"
    blob = files / (file_id + ".blob")
    blob.write_bytes(body)
    (files / "index.json").write_text(json.dumps([{"id": file_id, "size": len(body), "sha256": hashlib.sha256(body).hexdigest()}]))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KONAE_", "AWS_", "VITE_"))}
    env.update(KONAE_DATA_DIR=str(live), KONAE_COPY_DIR=str(copies), KONAE_KERNEL_PORT=str(port),
               KONAE_COPY_HEALTH_DIR=str(root / "status"), KONAE_COPY_INTERVAL_SECONDS="3600",
               KONAE_BIND="127.0.0.1", KONAE_PUBLIC_ORIGIN="http://localhost", KONAE_HOST_SUFFIX="localhost")
    url = f"http://127.0.0.1:{port}/_konae/health"

    def observe(proc, condition):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and proc.poll() is None:
            try:
                with urllib.request.urlopen(url, timeout=1) as reply:
                    report = json.load(reply)["copies"]["targets"][0]
                    if condition(report):
                        return report
            except (OSError, IndexError):
                pass
            time.sleep(.05)
        raise RuntimeError("Kernel did not report the expected result. " + (root / "log").read_text())

    with (root / "log").open("w") as log:
        def start():
            return subprocess.Popen([binary], env=env, stdout=log, stderr=log)
        proc = start()
        try:
            initial = observe(proc, lambda r: r["lastCopyId"] and not r["inProgress"])
            first = initial["lastCopyId"]
            assert initial["lastCutAt"] <= initial["lastSuccessAt"]
            assert initial["lastVerifiedRestoreAt"] is None
            restored = root / "restored"
            subprocess.run([binary, "restore", "--from", str(copies), "--copy", first], check=True,
                           env=dict(env, KONAE_DATA_DIR=str(restored)), stdout=log, stderr=log)
            assert (restored / blob.relative_to(live)).read_bytes() == body
            observe(proc, lambda r: r["lastVerifiedRestoreCopyId"] == first)
            proc.kill()
            proc.wait()
            blob.write_bytes(b"x" * len(body))
            proc = start()
            failed = observe(proc, lambda r: r["lastError"] and file_id in r["lastError"])
            for field in ["lastCutAt", "lastSuccessAt", "lastCopyId"]:
                assert failed[field] == initial[field]
            assert failed["lastVerifiedRestoreCopyId"] == first
            assert failed["lastAttemptAt"] > initial["lastAttemptAt"]
            proc.kill()
            proc.wait()
            blob.write_bytes(body)
            proc = start()
            recovered = observe(proc, lambda r: r["lastCopyId"] != first and not r["inProgress"])
            assert recovered["lastVerifiedRestoreCopyId"] == first
            assert recovered["alert"] is None
            print("HTTP health observed a verified CLI restore, two process restarts, a refused same-size substitution and a new complete cut.")
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=10)
