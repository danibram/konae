"""Check custody of the kernel operator's mail key on a Konae volume."""
import argparse
import json
import os
from pathlib import Path
import random
import stat
import subprocess
import tempfile

from pilot_fixture import cut, small_profile, year


def inspect(konae, env, key_count):
    report = json.loads(year["cli"](konae, env, "inventory"))
    counts = report["components"]
    assert counts["apps/{app}"] == 4
    assert counts["private/deployment.json"] == 3
    assert counts.get("root/mail-key", 0) == key_count
    assert (Path(env["KONAE_DATA_DIR"]) / "mail-key").exists() == bool(key_count)


def run(konae, root):
    live = root / "live"
    env = dict(cut["setting"](live), PATH=os.defpath, TOKIO_WORKER_THREADS="2")
    model = small_profile(konae, live, env)
    inspect(konae, env, 1)

    copies = root / "copies"
    with cut["server"](konae, dict(env, KONAE_COPY_DIR=str(copies))) as (port, _):
        reports = cut["wait"](lambda: cut["complete"](port), "no completed directory copy")
        point = reports[0]["lastCopyId"]
    manifest = json.loads((copies / point / "manifest.json").read_bytes())
    assert "mail-key" not in {entry["path"] for entry in manifest["files"]}
    assert not (copies / point / "mail-key").exists()

    restored = root / "restored"
    env["KONAE_DATA_DIR"] = str(restored)
    year["cli"](konae, env, "restore", "--from", copies, "--copy", point)
    inspect(konae, env, 0)

    year["import_version"](konae.with_name("kernel"), env, restored, model, random.Random(36), 2)
    key = restored / "mail-key"
    assert stat.S_IMODE(key.stat().st_mode) == 0o600
    inspect(konae, env, 1)

    (restored / "unregistered-store").touch()
    rejected = subprocess.run([konae, "inventory"], env=env, capture_output=True, timeout=15)
    assert rejected.returncode == 1 and not rejected.stdout
    assert b'No inventory policy for "unregistered-store"' in rejected.stderr


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=Path("kernel-rs/target/debug"))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        run(args.bin_dir.resolve() / "konae", Path(tmp))
    print("Three deployments and one draft. The copy omitted mail-key, kernel import recreated it "
          "with mode 0600, and konae inventory rejected the unknown store.")
