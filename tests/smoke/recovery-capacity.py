"""Fill a bounded test volume during v3 publication and restore both sides of the failure."""
import argparse
import errno
import json
import os
from pathlib import Path
import runpy
import shutil
import tempfile
import uuid

year = runpy.run_path(str(Path(__file__).with_name("recovery-year.py")))
cut = year["cut"]


def exercise(binary, root, repository, volume):
    live = root / "live"
    cut["fixture"](live)
    asset = Path("apps/probe/app/sample.bin")
    expected = (live / asset).read_bytes()
    kind = "directory-v3"
    env = dict(cut["setting"](live), TOKIO_WORKER_THREADS="2", KONAE_COPY_V3_DIR=str(repository),
               KONAE_COPY_SECRET=year["FIRST_KEY"], AWS_EC2_METADATA_DISABLED="true")
    first = year["capture"](binary, env, None, {kind})
    replacement = expected * 8
    (live / asset).write_bytes(replacement)
    filler = repository / "fill"
    try:
        with filler.open("xb", buffering=0) as stream:
            for _ in range(145):
                stream.write(bytes(1024**2))
            os.fsync(stream.fileno())
    except OSError as error:
        if error.errno != errno.ENOSPC:
            raise
    else:
        raise RuntimeError("The bounded volume did not fill")
    with year["server"](binary, env) as (port, _):
        def failed():
            report, = cut["targets"](port)
            return report if report["lastError"] and not report["inProgress"] else None

        report = cut["wait"](failed, "No failed capture on the full volume")
        assert report["kind"] == kind and report["lastCopyId"] == first, "previous success"
        assert "os error 28" in report["lastError"] or "No space left" in report["lastError"]
    assert {p.name for p in (repository / "points").iterdir()} == {first}
    filler.unlink()
    second = year["capture"](binary, env, first, {kind})
    for i, (point, body) in enumerate([(first, expected), (second, replacement)]):
        dest = root / str(i)
        year["cli"](binary, dict(env, KONAE_DATA_DIR=str(dest)), "restore", "--from", repository, "--copy", point)
        year["equal"]((dest / asset).read_bytes(), body, "restored asset")
    print(json.dumps(dict(binary=binary.name, volumeBytes=volume, error="ENOSPC", failedCaptures=1,
                          completePoints=2, verifiedRestores=2, skipped=["s3-v3"])))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    mount = Path(os.environ["LAB_SMALL_VOLUME"]).resolve(strict=True)
    size = shutil.disk_usage(mount).total
    if not (0 < size <= 144 * 1024**2) or not any(os.path.ismount(p) and p != Path("/") for p in (mount, *mount.parents)):
        parser.error("LAB_SMALL_VOLUME must be on a disposable mounted volume of at most 144 MiB")
    print("SKIP s3-v3 destination (directory ENOSPC)", flush=True)
    with tempfile.TemporaryDirectory(dir=mount) as repo, tempfile.TemporaryDirectory() as scratch:
        exercise(args.binary.resolve(strict=True), Path(scratch), Path(repo) / "copies-v3" / uuid.uuid4().hex, size)
