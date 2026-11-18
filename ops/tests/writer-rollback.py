"""Run the reader rollout, writer switch and historical rollback on disposable repositories."""
import argparse
from collections import Counter
import copy
import json
import os
from pathlib import Path
import runpy
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

from cutover_lab import Minio, digest, sandbox, sign_receipt, wrap
from pilot_fixture import cut, year
from pilot_observation import Traffic

OPS = Path(__file__).resolve().parents[1]
GATE = runpy.run_path(str(OPS / "writer-cutover.py"))
REVISIONS = ("158b7bfdba779a60c2dca96bbeb99c8d6067df64", "af1a897175ff93a7ce43cb0ffe29fd30a122f541")
TARGETS = ("directory", "s3")


def now():
    return time.time_ns() // 1_000_000


cut_ms = GATE["point_ms"]


def seed(binary, env, live):
    year["fixture"](live)
    model = year["Model"]()
    if binary.name == "konae":
        with cut["server"](binary.with_name("kernel"), env):
            pass
    with cut["server"](binary, env) as (port, _):
        if binary.name == "konae":
            owner, _ = year["owner_session"](port, env, signup=True)
            model.projects.add("probe")
            model.published.add("probe")
            year["check_projects"](port, owner, model)
        for email in year["MEMBERS"]:
            model.users[email], _ = year["account"](port, email, "signup")
    fid, email = year["FILE"], year["MEMBERS"][0]
    body = b"Private rollback fixture"
    model.files[fid] = (email, "private", body)
    folder = live / "apps/probe/data/files"
    folder.mkdir(exist_ok=True)
    (folder / (fid + ".blob")).write_bytes(body)
    (folder / "index.json").write_text(json.dumps([dict(id=fid, ownerId=model.users[email],
        visibility="private", name="probe", type="application/octet-stream", size=len(body))]))
    return model


def mutate(binary, env, model, day):
    email = year["MEMBERS"][0]
    title = "write-" + str(day)
    with cut["server"](binary, env) as (port, _):
        _, cookie = year["account"](port, email, "login")
        year["action"](port, cookie, "notes.add", {"title": title})
        model.rows[title] = email
        year["check_rows"](port, cookie, email, model)


def check_failed_cut(binary, env, point, kinds):
    start = now()
    with cut["server"](binary, dict(env, KONAE_COPY_READERS="0")) as (port, _):
        def failed():
            statuses = cut["targets"](port)
            assert {s["kind"] for s in statuses} == kinds
            return statuses if all((s["lastAttemptAt"] or 0) >= start and not s["inProgress"]
                                   and s["lastError"] for s in statuses) else None
        statuses = cut["wait"](failed, "common cut did not fail on all targets")
    for s in statuses:
        assert "reader limit" in s["lastError"] and s["lastCopyId"] == point and s["lastCutAt"] == (cut_ms(point) if point else None)
    return {s["kind"]: False for s in statuses}


def historical_prune(old, writable, legacy, remote, store, secret):
    ids = [f"2000-01-01T00-00-00.{i:03}Z" for i in range(25)]
    for i, point in enumerate(ids):
        folder = legacy / point
        folder.mkdir(exist_ok=True)
        manifest = json.dumps(dict(copyId=point, createdAt=946684800000+i, version=1, files=[])).encode()
        (folder / "manifest.json").write_bytes(manifest)
        store.call("pipe", "lab/konae-test/copies/" + point + "/manifest.json", data=manifest)
    env = dict(cut["setting"](writable / "empty-installation"), **remote, KONAE_COPY_DIR=str(legacy),
               KONAE_COPY_S3="1", KONAE_COPY_SECRET=secret, TOKIO_WORKER_THREADS="2")
    with cut["server"](old, env) as (port, _):
        def finished():
            statuses = cut["targets"](port)
            assert {s["kind"] for s in statuses} == set(TARGETS)
            for s in statuses:
                assert not s["lastError"], s["lastError"]
            return all(s["lastCopyId"] for s in statuses)
        cut["wait"](finished, "historical cleaner did not finish", 90)
    local = {p.name for p in legacy.iterdir()}
    remote_ids = {p.split("/")[0] for p in store.files("copies/")}
    removed = set(ids) - local
    assert removed and set(ids) - remote_ids == removed
    return len(removed)


def exercise(args, root, report, traffic, remote):
    actual = args.binary.resolve(strict=True)
    commit = args.build_commit
    subprocess.run(["git", "diff", "--quiet", commit, "--", "kernel-rs"], check=True)
    report.update(binary=actual.name, build="release", candidateCommit=commit, candidateSha256=digest(actual))
    live, legacy = root / "live", root / "legacy"
    local = root / "copies-v3" / uuid.uuid4().hex
    rid = uuid.uuid4().hex
    prefix = f"copies-v3/{rid}/"
    store = Minio(args.mc, root, remote)
    current = root / "current"
    command = sandbox(current, [root])
    binary = wrap(actual, current, command)
    if actual.name == "konae":
        wrap(actual.with_name("kernel"), current, command)
    base = dict(cut["setting"](live), PATH=os.defpath, AWS_EC2_METADATA_DISABLED="true", TOKIO_WORKER_THREADS="2")
    secret = secrets.token_hex(32)
    if actual.name == "konae":
        report["fixtureKernelSha256"] = digest(actual.with_name("kernel"))
    model = seed(binary, base, live)
    _, primary = store.user(store.bucket, "", "")
    env = dict(base, **primary, KONAE_COPY_SECRET=secret)
    snapshots = {}
    legacy_inventory = {t: {} for t in TARGETS}
    suspended = set()
    results = []
    counts = Counter()
    legacy_cfg = dict(KONAE_COPY_DIR=str(legacy), KONAE_COPY_S3="1")
    v3_cfg = dict(KONAE_COPY_V3_DIR=str(local), KONAE_COPY_V3_S3=rid)
    legacy_src = [legacy, "s3://konae-test/copies/"]
    v3_src = [local, "s3://konae-test/" + prefix]
    repositories = dict(zip(GATE["FORMATS"], map(str, legacy_src + v3_src)))
    builds = {binary: (commit, report["candidateSha256"])}

    def phase(name):
        report["phase"] = name
        traffic.boundary(name)

    def restore(kind, index, source, point, reader=binary, settings=env, scratch=root):
        start = time.monotonic()
        with tempfile.TemporaryDirectory(dir=scratch) as tmp:
            dest = Path(tmp) / "restored"
            out = year["cli"](reader, dict(settings, KONAE_DATA_DIR=str(dest)), "restore", "--from", source, "--copy", point)
            assert b"Applied the revocation record" in out
            year["verify"](reader, base, dest, snapshots[point], suspended, counts)
        legacy_format = kind in ("reader-first", "rollback", "compatibility")
        version = legacy_inventory[TARGETS[index]][point] if legacy_format else 3
        repository = repositories["vault"] if kind == "independent" else str(source)
        rev, sha = builds[reader]
        results.append(dict(commit=rev, sha256=sha, format=version, repository=repository, kind=kind, target=TARGETS[index], atMs=now(), copy=point, result="passed",
                            elapsedMs=round((time.monotonic() - start) * 1000)))
        return results[-1]

    def inspect_legacy(p):
        local_header = json.loads((legacy / p / "manifest.json").read_bytes())
        remote_header = json.loads(store.call("cat", "lab/konae-test/copies/" + p + "/manifest.json"))
        for target, header, expected in zip(TARGETS, (local_header, remote_header), (1, 2)):
            assert header["copyId"] == p and header["version"] == expected
            legacy_inventory[target][p] = header["version"]

    phase("load-file-store")
    year["cli"](binary, env, "files-migrate", "--to", "s3")
    phase("usual-writer-no-point")
    failed_legacy = check_failed_cut(binary, env | legacy_cfg, None, set(TARGETS))
    assert not store.files("copies/") and not list(legacy.glob("*/manifest.json"))
    mutate(binary, env, model, 0)
    report["usualWriterNoPoint"] = True
    phase("legacy-writer")
    first = year["capture"](binary, env | legacy_cfg, None, set(TARGETS))
    snapshots[first] = copy.deepcopy(model)
    inspect_legacy(first)
    for i, source in enumerate(legacy_src):
        phase("reader-first-" + TARGETS[i])
        restore("reader-first", i, source, first)
    verified = now()
    phase("shadow")
    enabled = now()
    shadow = year["capture"](binary, env | legacy_cfg | v3_cfg, first, set(GATE["FORMATS"]))
    snapshots[shadow] = copy.deepcopy(model)
    inspect_legacy(shadow)
    assert verified <= cut_ms(shadow)
    for i, source in enumerate(v3_src):
        phase("before-prune-" + TARGETS[i])
        restore("before-prune", i, source, shadow)
    phase("common-cut-refusal")
    failed_cut = check_failed_cut(binary, env | legacy_cfg | v3_cfg, shadow, set(GATE["FORMATS"]))
    phase("record-revocation")
    victim = year["MEMBERS"][2]
    year["cli"](binary.with_name("kernel"), env | legacy_cfg | v3_cfg, "suspend", model.users[victim], "--app", "probe")
    suspended.add(victim)
    point = shadow
    published = [shadow]
    for day in range(1, 31):
        phase("mutate")
        mutate(binary, env, model, day)
        phase("v3-writer")
        point = year["capture"](binary, env | v3_cfg, point, {"directory-v3", "s3-v3"})
        snapshots[point] = copy.deepcopy(model)
        published.append(point)
        i = (day - 1) % 2
        phase("daily-" + TARGETS[i])
        restore("daily", i, v3_src[i], point)
        if day % 7 == 0:
            i = (day // 7 - 1) % 2
            phase("weekly-" + TARGETS[i])
            listed = sorted(p.name for p in (local / "points").iterdir()) if i == 0 else store.files(prefix + "points/")
            restore("weekly", i, v3_src[i], listed[0])["oldestRetained"] = listed[0]
    phase("post-prune")
    kept = sorted(p.name for p in (local / "points").iterdir())
    assert kept == store.files(prefix + "points/") and len(kept) < len(published) and kept[-1] == point
    final_prune = now()
    report.update(prunedPerDestination=len(published)-len(kept), retainedPerDestination=len(kept))
    phase("remove-source")
    shutil.rmtree(live)
    store.call("rm", "--recursive", "--force", "lab/konae-test/files/")
    assert not store.files("files/")
    phase("lease-permissions")
    _, lease_settings = store.user(store.bucket, prefix, prefix + "users/")
    keys = ["points/" + point, "objects/" + store.files(prefix + "objects/")[0]]
    store.deny_changes(lease_settings, store.bucket, prefix, keys)
    for i, source in enumerate(v3_src):
        settings = base | {"KONAE_COPY_SECRET": secret}
        if i:
            settings.update(lease_settings)
        for kind, p in [("after-oldest", kept[0]), ("after-latest", point)]:
            phase(kind + "-" + TARGETS[i])
            restore(kind, i, source, p, settings=settings)
    phase("vault-replication")
    vault = "vault-" + uuid.uuid4().hex
    repositories["vault"] = "s3://" + vault + "/" + prefix
    store.call("mb", "lab/" + vault)
    store.call("mirror", "lab/konae-test/" + prefix, f"lab/{vault}/" + prefix)
    phase("vault-permissions")
    emergency, vault_settings = store.user(vault, prefix, None)
    _, legacy_settings = store.user(store.bucket, "copies/", "copies/")
    store.deny_changes(primary, vault, prefix, keys)
    store.deny_changes(legacy_settings, vault, prefix, keys)
    store.deny_changes(vault_settings, vault, prefix, keys)
    store.call("cat", f"user/konae-test/{prefix}{keys[0]}", env=emergency, denied=True)
    phase("emergency-mirror")
    mirror = root / "recovery" / "copies-v3" / rid
    mirror_start = time.monotonic()
    store.call("mirror", f"user/{vault}/" + prefix, mirror, env=emergency)
    mirror_ms = report["emergencyMirrorMs"] = round((time.monotonic() - mirror_start) * 1000)
    phase("offline")
    for end in [kept[0], point]:
        d = restore("independent", 1, mirror, end, settings=dict(base, KONAE_COPY_SECRET=secret))
        d.update(emergencyCredentials=True, elapsedMs=mirror_ms + d["elapsedMs"])

    phase("select-compatible")
    legacy_ids = sorted(p.name for p in legacy.iterdir() if (p / "manifest.json").exists())
    s3_ids = sorted(p.split("/")[0] for p in store.files("copies/") if p.endswith("/manifest.json"))
    assert legacy_ids == s3_ids == [first, shadow]
    selected = legacy_ids[-1]
    delta = cut_ms(point) - cut_ms(selected)
    assert delta > 0
    incident = now()
    report.update(incidentAgeMs=incident - cut_ms(selected), cutDifferenceMs=delta,
                  omittedFixtureWrites=len(snapshots[point].rows) - len(snapshots[selected].rows))
    phase("legacy-permissions")
    store.deny_changes(legacy_settings, store.bucket, prefix, keys)
    rollbacks = report["historical"] = {}
    cleaners = []
    for rev in REVISIONS:
        phase("rollback-" + rev[:7])
        artifact = args.baseline / rev / binary.name
        expected = dict(line.split()[::-1] for line in (artifact.parent / "binaries.sha256").read_text().splitlines())
        sha = digest(artifact)
        assert expected[binary.name] == sha
        private = root / ("rollback-" + rev)
        private.mkdir()
        isolation = sandbox(root / rev, [private, legacy], [local.parent])
        for group in ["points", "objects"]:
            existing = next((local / group).iterdir())
            for probe in (["/usr/bin/touch", existing], ["/bin/rm", existing]):
                denied = subprocess.run([*isolation, *probe], capture_output=True)
                assert denied.returncode and b"Operation not permitted" in denied.stderr
        old = wrap(artifact.resolve(), root / rev, isolation)
        builds[old] = (rev, sha)
        record = rollbacks[rev] = dict(sha256=sha, restoreMs={}, localWriteDenials=4)
        for i, source in enumerate(legacy_src):
            phase("restore-" + rev[:7] + "-" + TARGETS[i])
            settings = dict(base, KONAE_COPY_SECRET=secret)
            if i:
                settings.update(legacy_settings)
            kind = "rollback" if rev == REVISIONS[0] else "compatibility"
            d = restore(kind, i, source, selected, old, settings, private)
            record["restoreMs"][TARGETS[i]] = d["elapsedMs"]
        cleaners.append((rev, old, private))
    for rev, old, private in cleaners:
        phase("prune-" + rev[:7])
        rollbacks[rev]["expiredPerTarget"] = historical_prune(old, private, legacy, legacy_settings, store, secret)
    for i, source in enumerate(v3_src):
        phase("verify-after-cleaner-" + TARGETS[i])
        restore("after-cleaner", i, source, point)

    receipt = {
        "material": "synthetic", "commit": commit, "sha256": report["candidateSha256"],
        "pilot": {"result": args.pilot, "commit": commit}, "completeCut": dict.fromkeys(GATE["FORMATS"], True),
        "limits": {"rpoMs": 3_900_000, "rtoMs": 1_800_000},
        "unclassified": [], "integrityAlerts": [], "readerFirst": True, "historicalCustody": False,
        "remoteCopy": point, "remoteCutMs": cut_ms(point),
        "readerVerifiedMs": verified, "writerEnabledMs": enabled, "firstV3WriteMs": cut_ms(shadow), "windowStartMs": cut_ms(shadow),
        "rollback": dict(commit=REVISIONS[0], sha256=rollbacks[REVISIONS[0]]["sha256"], cleanerDenied=True,
                         copy=selected, incidentMs=incident, compatibleCutMs=cut_ms(selected), latestCutMs=cut_ms(point)),
        "repositories": repositories, "drills": results, "legacyInventory": legacy_inventory,
        "publicationJournal": {t: published for t in TARGETS},
        "finalPrune": {t: dict(atMs=final_prune, oldest=kept[0], latest=point, points=kept,
                               repository=repositories[t + "-v3"]) for t in TARGETS},
    }
    path, public = sign_receipt(root, receipt)
    for decision in ("promote", "retire"):
        holds = GATE["review"](path, public, decision)
        expected_holds = {"receipt material = synthetic", "operations signature authority = lab",
                          "maintenance signature authority = lab",
                          "reader rollout or historical custody unverified"}
        if args.pilot != "passed":
            expected_holds.add("B11 pilot missing or failed")
        if decision == "retire":
            expected_holds.add("30 real days have not elapsed")
            expected_holds.update(t + " final prune preceded the retention window" for t in TARGETS)
        assert set(holds) == expected_holds, holds
        report[decision + "Holds"] = holds
    for case, changes in {
        "sharedCutFailure": dict(completeCut=failed_cut, remoteCopy=shadow, remoteCutMs=cut_ms(shadow)),
        "usualWriterWithoutPoint": dict(completeCut=failed_legacy, remoteCopy=None, remoteCutMs=None),
    }.items():
        directory = root / case
        directory.mkdir()
        path, public = sign_receipt(directory, receipt | changes)
        holds = GATE["review"](path, public, "promote")
        assert "usual writer cut incomplete" in holds and "v3 writer cut incomplete" in holds
        if case == "usualWriterWithoutPoint":
            assert "no identified complete remote cut" in holds
        report[case] = holds
    report["remoteCutAgeAtReviewMs"] = now() - cut_ms(point)
    report["b11"] = args.pilot
    report["s3WriteDenials"] = dict(legacyOnActive=4, legacyOnVault=4, primaryOnVault=4, activeReader=4, vaultReader=4)
    report["verifiedAccess"] = dict(counts)
    summary = report["drills"] = {}
    for drill in results:
        by_target = summary.setdefault(drill["kind"], {})
        stats = by_target.setdefault(drill["target"], dict(count=0, maxMs=0))
        stats["count"] += 1
        stats["maxMs"] = max(stats["maxMs"], drill["elapsedMs"])
    phase("complete")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--build-commit", required=True, help="full checkout commit used to build --binary")
    parser.add_argument("--baseline", type=Path, required=True, help="release artifacts by full commit")
    parser.add_argument("--mc", type=Path, required=True)
    parser.add_argument("--pilot", choices=("pending", "failed", "passed"), default="pending",
                        help="B11 disposition supplied by the release operator for this hold exercise")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.binary.name not in ("kernel", "konae"):
        parser.error("choose a kernel or konae binary")
    os.umask(0o077)
    if not __debug__:
        parser.error("Python assertions must be enabled")
    if args.report.exists():
        parser.error("choose a new report")
    report = dict(material="synthetic", omitted=[], result="failed")
    try:
        if not os.environ.get("KONAE_TEST_S3_ENDPOINT"):
            report["omitted"] = ["directory", "s3"]
            sys.exit("SKIP directory and S3: rollback requires both destinations inside with-minio")
        remote = year["remote_settings"](args.mc)
        with tempfile.TemporaryDirectory(prefix="writer-rollback-") as tmp, Traffic(args.mc) as traffic:
            try:
                exercise(args, Path(tmp).resolve(), report, traffic, remote)
            finally:
                report["s3TrafficByPhase"] = traffic.stats
        report["result"] = "passed"
    finally:
        args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        print(report["result"], report.get("phase", "setup"), flush=True)


if __name__ == "__main__":
    main()
