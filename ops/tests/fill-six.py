"""Fill a newly seeded, stopped synthetic installation to the pilot size."""
import hashlib
import json
from pathlib import Path
import sys
import uuid


def fill(root):
    apps = json.loads((root / "apps.json").read_bytes())
    assert [a["projectId"] for a in apps] == [f"fixture_app_{n}" for n in range(6)]
    live = root / "live"
    oracle = root / "expected.json"
    if oracle.exists():
        raise ValueError("Expected a fresh six-app fixture")
    initial = sum(p.stat().st_size for p in live.rglob("*") if p.is_file())
    size, remainder = divmod(291_988_147 - initial, 249)
    assert size > 0
    files = []
    generated = 0
    for n, app in enumerate(apps):
        folder = live / "apps" / app["id"] / "data/files"
        index = folder / "index.json"
        original, = json.loads(index.read_bytes())
        assert original["id"] == app["fileId"] and original["ownerId"] and original["visibility"] == "private"
        entries = [original]
        raw = (folder / (original["id"] + ".blob")).read_bytes()
        files.append(dict(app=app["id"], id=original["id"], size=len(raw), sha256=hashlib.sha256(raw).hexdigest()))
        for i in range(42 if n < 3 else 41):
            seed = f"konae-synthetic-pilot:{n}:{i}"
            key = str(uuid.uuid5(uuid.NAMESPACE_URL, seed))
            payload = hashlib.shake_256(seed.encode()).digest(size + int(generated < remainder))
            digest = hashlib.sha256(payload).hexdigest()
            with (folder / (key + ".blob")).open("xb") as blob:
                blob.write(payload)
            entries.append(dict(original, id=key, size=len(payload), sha256=digest, type="application/octet-stream",
                                name=f"sample-{i}.bin"))
            files.append(dict(app=app["id"], id=key, size=len(payload), sha256=digest))
            generated += 1
        pending = index.with_suffix(".new")
        pending.write_text(json.dumps(entries, separators=(",", ":")))
        pending.replace(index)
    assert len(files) == 255
    with oracle.open("x") as output:
        json.dump(files, output)
    print(json.dumps(dict(apps=len(apps), privateFiles=len(files), appDatabases=len(list(live.glob("apps/*/data/*.sqlite"))),
                          liveBytes=sum(p.stat().st_size for p in live.rglob("*") if p.is_file()),
                          payloadBytes=sum(f["size"] for f in files))))


if __name__ == "__main__":
    fill(Path(sys.argv[1]).resolve(strict=True))
