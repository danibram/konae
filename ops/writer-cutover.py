#!/usr/bin/env python3
"""Review a restricted installation-copy receipt. Exit 1 means hold the writer."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import time

DAY = 86_400_000
TARGETS = ("directory", "s3")
FORMATS = ("directory", "s3", "directory-v3", "s3-v3")


def point(p):
    return isinstance(p, str) and re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d\.\d{3}Z", p)


def point_ms(p):
    if not point(p):
        raise ValueError("invalid copy ID")
    stamp = datetime.strptime(p, "%Y-%m-%dT%H-%M-%S.%fZ").replace(tzinfo=timezone.utc)
    return round(stamp.timestamp() * 1000)


def retained_at(journal, prune, reviewed):
    if len(journal) != len(set(journal)):
        raise ValueError("duplicate publication")
    dated = sorted((point_ms(p), p) for p in journal)
    if not dated or dated[-1][0] > reviewed:
        raise ValueError("empty or future publication journal")
    cuts = [p for at, p in dated if at <= prune]
    if not cuts:
        raise ValueError("no confirmed cut before prune")
    daily = {}
    for copy in cuts:
        at = point_ms(copy)
        if prune - at <= 30 * DAY:
            daily[at // DAY] = copy
    return sorted(set(cuts[-24:]) | set(daily.values()))


def check_receipt(r, request, now):
    failures = []

    def require(ok, message):
        if not ok:
            failures.append(message)

    def number(n):
        return type(n) in (int, float) and math.isfinite(n) and n >= 0

    def day(ms):
        if not number(ms) or ms > now:
            raise ValueError("invalid evidence time")
        return datetime.fromtimestamp(ms / 1000, timezone.utc).date().toordinal()

    require(r["material"] == "installation", "receipt material = " + str(r["material"]))
    for role in ("operations", "maintenance"):
        require(r["signers"][role]["authority"] == "operational",
                role + " signature authority = " + str(r["signers"][role]["authority"]))
    require(r["pilot"]["result"] == "passed" and r["pilot"]["commit"] == r["commit"], "B11 pilot missing or failed")
    require(all(r["completeCut"].get(t) is True for t in FORMATS[:2]), "usual writer cut incomplete")
    require(all(r["completeCut"].get(t) is True for t in FORMATS[2:]), "v3 writer cut incomplete")
    require(r["unclassified"] == [] and r["integrityAlerts"] == [], "inventory or integrity alert")
    require(r["readerFirst"] is True and r["historicalCustody"] is True, "reader rollout or historical custody unverified")
    require(re.fullmatch("[0-9a-f]{40}", r["commit"]) and re.fullmatch("[0-9a-f]{64}", r["sha256"]),
            "candidate build receipt missing")
    for budget in ("rpoMs", "rtoMs"):
        require(number(r["limits"][budget]) and r["limits"][budget] > 0, "invalid " + budget)
    cut = r["remoteCutMs"]
    require(point(r["remoteCopy"]) and point_ms(r["remoteCopy"]) == cut, "no identified complete remote cut")
    require(number(cut) and 0 <= now - cut <= r["limits"]["rpoMs"], "remote cut outside RPO")
    repos = r["repositories"]
    require(all(isinstance(repos[t], str) and repos[t] for t in (*FORMATS, "vault")) and
            len(set(repos.values())) == len(repos), "repository identities missing or repeated")
    remote = r["publicationJournal"]["s3"]
    require(remote and r["remoteCopy"] == max(remote), "latest remote point differs from publication journal")
    drills = r["drills"]
    for d in drills:
        require(d["result"] == "passed" and point(d["copy"]) and number(d["elapsedMs"]) and
                d["elapsedMs"] <= r["limits"]["rtoMs"], "restore failed, lacks a point, or exceeded RTO")
        target = d["target"]
        if target not in TARGETS:
            raise ValueError("unknown drill destination")
        legacy = d["kind"] in ("reader-first", "rollback", "compatibility")
        version = r["rollback"] if d["kind"] == "rollback" else r
        kind = target if legacy else target + "-v3"
        repo = repos["vault"] if d["kind"] == "independent" else repos[kind]
        expected_format = r["legacyInventory"][target].get(d["copy"]) if legacy else 3
        require(d["repository"] == repo and d["format"] == expected_format and
                expected_format in ((1, 2) if legacy else (3,)), "restore format or repository mismatch")
        require(re.fullmatch("[0-9a-f]{40}", d["commit"]) and re.fullmatch("[0-9a-f]{64}", d["sha256"]) and
                (d["kind"] == "compatibility" or (d["commit"], d["sha256"]) == (version["commit"], version["sha256"])),
                "restore reader build mismatch")
        if not legacy:
            require(d["copy"] in r["publicationJournal"][target], "restored v3 point absent from publication journal")
        day(d["atMs"])
        require(point(d["copy"]) and point_ms(d["copy"]) <= d["atMs"], "restore predates its cut")
        if d["kind"] == "weekly":
            require(d["copy"] == d["oldestRetained"], "weekly restore missed the oldest retained point")
        if d["kind"] == "daily":
            require(point(d["copy"]) and d["atMs"] - point_ms(d["copy"]) <= r["limits"]["rpoMs"],
                    "daily restore used a stale cut")
    for kind in ("reader-first", "before-prune", "after-oldest", "after-latest", "rollback"):
        require({d["target"] for d in drills if d["kind"] == kind} == set(TARGETS), "missing targets for " + kind)
    require(any(d["kind"] == "independent" and d["emergencyCredentials"] is True for d in drills),
            "independent restore missing")
    rollback = r["rollback"]
    require(re.fullmatch("[0-9a-f]{40}", rollback["commit"]) and re.fullmatch("[0-9a-f]{64}", rollback["sha256"])
            and rollback["cleanerDenied"] is True and point(rollback["copy"]), "rollback build or isolation missing")
    require(number(rollback["compatibleCutMs"]) and number(rollback["latestCutMs"]) and
            0 <= rollback["latestCutMs"] - rollback["compatibleCutMs"] <= r["limits"]["rpoMs"],
            "rollback cut difference outside RPO")
    require(point(rollback["copy"]) and point_ms(rollback["copy"]) == rollback["compatibleCutMs"] and
            rollback["latestCutMs"] <= rollback["incidentMs"] <= now and
            rollback["incidentMs"] - rollback["compatibleCutMs"] <= r["limits"]["rpoMs"],
            "compatible point outside incident RPO")
    for target in TARGETS:
        require(any(d["kind"] == "rollback" and d["target"] == target and d["copy"] == rollback["copy"]
                    and d["commit"] == rollback["commit"] and d["sha256"] == rollback["sha256"] for d in drills),
                "missing exact historical restore on " + target)
    day(r["readerVerifiedMs"])
    day(r["writerEnabledMs"])
    day(r["firstV3WriteMs"])
    require(r["readerVerifiedMs"] <= r["writerEnabledMs"] <= r["firstV3WriteMs"], "reader verification followed writer enablement")
    start = r["windowStartMs"]
    today = day(now)
    start_day = day(start)
    require(start >= r["firstV3WriteMs"], "window started before v3")
    if request == "retire":
        require(now - start >= 30 * DAY, "30 real days have not elapsed")
    for kind, interval in (("daily", 1), ("weekly", 7)):
        entries = sorted((d for d in drills if d["kind"] == kind and d["atMs"] >= start),
                         key=lambda d: d["atMs"])
        dates = [day(d["atMs"]) for d in entries]
        require(dates and dates[0] - start_day < interval and today - dates[-1] <= interval
                and all(b - a <= interval for a, b in zip(dates, dates[1:])), kind + " drill overdue")
        require(all(d["target"] in TARGETS for d in entries) and all(a["target"] != b["target"]
                for a, b in zip(entries, entries[1:])), kind + " destinations did not alternate")
    for target in TARGETS:
        inventory = r["finalPrune"][target]
        prune = inventory["atMs"]
        day(prune)
        require(point(inventory["oldest"]) and point(inventory["latest"]) and
                inventory["oldest"] < inventory["latest"], target + " missing retained range")
        journal = r["publicationJournal"][target]
        retained = sorted(inventory["points"])
        if not journal:
            failures.append(target + " has no confirmed publication")
            continue
        expected = retained_at(journal, prune, now)
        require(retained == expected and [inventory["oldest"], inventory["latest"]] == [expected[0], expected[-1]],
                target + " retained inventory differs from the publication journal")
        require(inventory["repository"] == repos[target + "-v3"], target + " prune repository mismatch")
        if request == "retire":
            begin = max(start, prune - 30 * DAY)
            times = [begin, *sorted(at for p in journal if begin <= (at := point_ms(p)) <= now), now]
            require(all(b - a <= r["limits"]["rpoMs"] for a, b in zip(times, times[1:])),
                    target + " publication journal has a gap beyond RPO")
            require(prune >= start + 30 * DAY, target + " final prune preceded the retention window")
        require(all(any(d["kind"] == kind and d["target"] == target and d["atMs"] >= prune
                        and d["copy"] == inventory[end] for d in drills)
                    for end, kind in (("oldest", "after-oldest"), ("latest", "after-latest"))),
                target + " lacks both retained ends after the final prune")
        require(any(d["kind"] == "before-prune" and d["target"] == target and d["atMs"] < prune for d in drills),
                target + " lacks a verified point before pruning")
        require(any(d["kind"] == "reader-first" and d["target"] == target and
                    d["atMs"] <= r["readerVerifiedMs"] for d in drills), target + " lacks a reader-first restore")
    return list(dict.fromkeys(failures))


def review(receipt, keys, request):
    raw = receipt.read_bytes()
    r = json.loads(raw)
    identities = set()
    lab_labels = []
    if r.get("material") == "synthetic":
        lab_labels.append("submitted receipt is labelled synthetic")
    for role in keys:
        if r["signers"][role]["authority"] == "lab":
            lab_labels.append("submitted " + role + " signature is labelled lab")
    for role, key in keys.items():
        public = subprocess.check_output(["openssl", "pkey", "-pubin", "-in", key, "-outform", "DER"])
        digest = hashlib.sha256(public).hexdigest()
        if digest in identities or digest != r["signers"][role]["keySha256"]:
            return ["untrusted signing key for " + role, *lab_labels]
        identities.add(digest)
        verified = subprocess.run(["openssl", "dgst", "-sha256", "-verify", key, "-signature",
                        str(receipt) + "." + role + ".sig"], input=raw, capture_output=True)
        if verified.returncode:
            return ["invalid signature for " + role, *lab_labels]
    return check_receipt(r, request, time.time_ns() // 1_000_000)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("request", choices=["promote", "retire"])
    parser.add_argument("receipt", type=Path)
    for role in ("operations", "maintenance"):
        parser.add_argument("--" + role + "-key", type=Path, required=True)
    args = parser.parse_args()
    try:
        failures = review(args.receipt, {"operations": args.operations_key, "maintenance": args.maintenance_key},
                          args.request)
    except (OSError, KeyError, TypeError, ValueError, OverflowError, subprocess.SubprocessError) as error:
        print("HOLD invalid receipt or signature (" + type(error).__name__ + ")", file=sys.stderr)
        return 1
    for reason in failures:
        print("HOLD " + reason)
    if not failures:
        print("Receipt passed review. The release operator owns the change.")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
