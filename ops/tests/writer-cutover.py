"""Exercise the receipt rules with a fixed clock and fictional operational evidence."""
from datetime import datetime, timezone
from pathlib import Path
import json
import os
import runpy
import subprocess
import sys
import tempfile

import unittest

from cutover_lab import sign_receipt

gate = runpy.run_path(str(Path(__file__).resolve().parents[1] / "writer-cutover.py"))
DAY = gate["DAY"]
START = 1_700_000_000_000
NOW = START + 30 * DAY + 600_000
COMMIT = "1" * 40
OLD = "2" * 40
SHA = "1" * 64
OLD_SHA = "2" * 64


def point(ms):
    return datetime.fromtimestamp(ms / 1000, timezone.utc).strftime("%Y-%m-%dT%H-%M-%S.%f")[:-3] + "Z"


def example():
    latest = NOW - 120_000
    compatible = latest - 60_000
    intervals = range(START, latest, 3_600_000)
    journal = [point(at) for at in [*intervals, latest]]
    prune_at = latest + 1000
    retained = gate["retained_at"](journal, prune_at, NOW)
    repos = {k: "fixture-" + k for k in (*gate["FORMATS"], "vault")}
    drills = []
    prune = {}

    def record(kind, target, at, selected, **fields):
        historical = kind == "rollback"
        legacy = historical or kind == "reader-first"
        repo = target if legacy else target + "-v3"
        if kind == "independent":
            repo = "vault"
        drills.append(dict(kind=kind, target=target, atMs=at, copy=point(selected),
                           format=(1 if target == "directory" else 2) if legacy else 3,
                           repository=repos[repo], commit=OLD if historical else COMMIT,
                           sha256=OLD_SHA if historical else SHA, result="passed", elapsedMs=950, **fields))
    for day in range(31):
        at = START + day * DAY
        record("daily", gate["TARGETS"][day % 2], at + 1000, at)
        if day % 7 == 0:
            record("weekly", gate["TARGETS"][(day // 7) % 2], at + 2000, START, oldestRetained=point(START))
    for target in gate["TARGETS"]:
        record("reader-first", target, START - 1000, START - 2000)
        record("before-prune", target, START + 30_000, START)
        record("after-oldest", target, NOW - 1000, gate["point_ms"](retained[0]))
        record("after-latest", target, NOW - 1000, latest)
        record("rollback", target, NOW - 500, compatible)
        prune[target] = dict(atMs=prune_at, points=retained, oldest=retained[0], latest=retained[-1],
                             repository=repos[target + "-v3"])
    record("independent", "s3", NOW - 1000, latest, emergencyCredentials=True)
    legacy = {t: {point(compatible): i + 1, point(START - 2000): i + 1}
              for i, t in enumerate(gate["TARGETS"])}
    return dict(repositories=repos, legacyInventory=legacy, publicationJournal={t: journal for t in gate["TARGETS"]}, material="installation", pilot=dict(result="passed", commit=COMMIT),
                completeCut=dict.fromkeys(gate["FORMATS"], True), signers={role: dict(authority="operational")
                for role in ("operations", "maintenance")}, unclassified=[], integrityAlerts=[], readerFirst=True,
                historicalCustody=True, commit=COMMIT, sha256=SHA, limits=dict(rpoMs=3_900_000, rtoMs=1_800_000),
                rollback=dict(commit=OLD, sha256=OLD_SHA, cleanerDenied=True, copy=point(compatible),
                              latestCutMs=latest, compatibleCutMs=compatible, incidentMs=latest + 10),
                remoteCopy=point(latest), remoteCutMs=latest, readerVerifiedMs=START-1000, writerEnabledMs=START, firstV3WriteMs=START,
                windowStartMs=START, finalPrune=prune, drills=drills)


class Receipt(unittest.TestCase):
    def test_calendar_and_limits(self):
        self.assertEqual(gate["check_receipt"](example(), "retire", NOW), [])
        cases = [
            (lambda r: r.update(material="synthetic"), "receipt material = synthetic"),
            (lambda r: r["signers"]["maintenance"].update(authority="lab"), "maintenance signature authority = lab"),
            (lambda r: r["pilot"].update(result="failed"), "B11 pilot missing or failed"),
            (lambda r: r["completeCut"].update(s3=False), "usual writer cut incomplete"),
            (lambda r: r["completeCut"].pop("s3-v3"), "v3 writer cut incomplete"),
            (lambda r: r.update(remoteCutMs=NOW - 3_900_001), "remote cut outside RPO"),
            (lambda r: r.update(remoteCopy=None), "no identified complete remote cut"),
            (lambda r: r["rollback"].update(incidentMs=NOW + 1), "compatible point outside incident RPO"),
            (lambda r: r["rollback"].update(cleanerDenied=False), "rollback build or isolation missing"),
            (lambda r: r.update(readerVerifiedMs=START + 1), "reader verification followed writer enablement"),
            (lambda r: r.update(unclassified=["unknown"]), "inventory or integrity alert"),
            (lambda r: r.update(integrityAlerts=["object mismatch"]), "inventory or integrity alert"),
            (lambda r: r["drills"][0].update(elapsedMs=1_800_001), "restore failed, lacks a point, or exceeded RTO"),
            (lambda r: r["drills"][0].update(result="failed"), "restore failed, lacks a point, or exceeded RTO"),
            (lambda r: r["drills"].pop(-1), "independent restore missing"),
            (lambda r: r.update(historicalCustody=False), "reader rollout or historical custody unverified"),
        ]
        for change, reason in cases:
            with self.subTest(reason=reason):
                r = example()
                change(r)
                self.assertIn(reason, gate["check_receipt"](r, "promote", NOW))

    def test_retention(self):
        for change, reason in [
            (lambda r: r.update(windowStartMs=START + DAY), "30 real days have not elapsed"),
            (lambda r: r["drills"].pop(3), "daily drill overdue"),
            (lambda r: r["drills"][2].update(target="directory"), "daily destinations did not alternate"),
            (lambda r: r["drills"][1].update(oldestRetained=point(NOW)), "weekly restore missed the oldest retained point"),
            (lambda r: r["finalPrune"]["s3"].update(atMs=NOW), "s3 lacks both retained ends after the final prune"),
            (lambda r: r["finalPrune"]["s3"].update(oldest=point(START + DAY)), "s3 lacks both retained ends after the final prune"),
        ]:
            with self.subTest(reason=reason):
                r = example()
                change(r)
                self.assertIn(reason, gate["check_receipt"](r, "retire", NOW))

    def test_retained_inventory_against_independent_journal(self):
        r = example()
        # Select the last UTC cut of each eligible day independently of the gate.
        prune = r["finalPrune"]["s3"]["atMs"]
        journal = r["publicationJournal"]["s3"]
        eligible = [p for p in journal if gate["point_ms"](p) >= prune - 30 * DAY]
        expected = sorted(set(journal[-24:]) | {max(p for p in eligible if p[:10] == date)
                                               for date in {p[:10] for p in eligible}})
        self.assertEqual(r["finalPrune"]["s3"]["points"], expected)
        self.assertEqual(gate["check_receipt"](r, "retire", NOW), [])
        # Both restores can pass after a cleaner loses the rest of the month.
        r["finalPrune"]["s3"].update(points=expected[-2:], oldest=expected[-2])
        next(d for d in r["drills"] if d["target"] == "s3" and d["kind"] == "after-oldest")["copy"] = expected[-2]
        self.assertIn("s3 retained inventory differs from the publication journal",
                      gate["check_receipt"](r, "retire", NOW))
        r["publicationJournal"]["s3"] = expected[-2:]
        self.assertIn("s3 publication journal has a gap beyond RPO", gate["check_receipt"](r, "retire", NOW))

    def test_no_point_after_failed_usual_writer_cut(self):
        r = example()
        r.update(completeCut={"directory": False, "s3": False}, drills=[], remoteCopy=None, remoteCutMs=None,
                 publicationJournal={"directory": [], "s3": []})
        holds = gate["check_receipt"](r, "promote", NOW)
        self.assertIn("usual writer cut incomplete", holds)
        self.assertIn("no identified complete remote cut", holds)
        self.assertIn("s3 has no confirmed publication", holds)
        self.assertIn("missing targets for rollback", holds)

    def test_publication_during_endpoint_restores(self):
        r = example()
        newer = point(NOW - 400)
        for target, journal in r["publicationJournal"].items():
            r["publicationJournal"][target] = [*journal, newer]
        r.update(remoteCopy=newer, remoteCutMs=NOW - 400)
        self.assertEqual(gate["check_receipt"](r, "retire", NOW), [])
        roots = r["finalPrune"]["s3"]["points"]
        r["finalPrune"]["s3"]["points"] = roots[1:]
        self.assertIn("s3 retained inventory differs from the publication journal",
                      gate["check_receipt"](r, "retire", NOW))
        for invalid in (newer, point(NOW + 1)):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                gate["retained_at"]([*r["publicationJournal"]["s3"], invalid], NOW - 500, NOW)

    def test_utc_day_and_age_boundary(self):
        midnight = 1_700_006_400_000
        self.assertEqual(midnight % DAY, 0)
        cutoff = midnight + 30 * DAY
        recent = [point(cutoff - i) for i in range(24)]
        roots = [point(midnight - 1), point(midnight), *recent]
        self.assertEqual(gate["retained_at"](roots, cutoff, cutoff), sorted([point(midnight), *recent]))
        self.assertEqual(gate["retained_at"](roots, cutoff + 1, cutoff + 1), sorted(recent))
        roots.append(point(midnight + 1))
        self.assertEqual(gate["retained_at"](roots, cutoff, cutoff), sorted([point(midnight + 1), *recent]))

    def test_substituted_reader_format_and_destination(self):
        for kind, field, value, expected in [
            ("reader-first", "format", 3, "restore format or repository mismatch"),
            ("reader-first", "sha256", OLD_SHA, "restore reader build mismatch"),
            ("after-oldest", "commit", OLD, "restore reader build mismatch"),
            ("after-latest", "repository", "fixture-directory", "restore format or repository mismatch"),
            ("weekly", "format", 1, "restore format or repository mismatch"),
            ("rollback", "sha256", SHA, "missing exact historical restore on directory"),
        ]:
            with self.subTest(kind=kind, field=field):
                r = example()
                next(d for d in r["drills"] if d["kind"] == kind and d["target"] == "directory")[field] = value
                self.assertIn(expected, gate["check_receipt"](r, "retire", NOW))

    def test_wrong_legacy_manifest_version(self):
        r = example()
        next(d for d in r["drills"] if d["kind"] == "rollback" and d["target"] == "s3")["format"] = 1
        self.assertIn("restore format or repository mismatch", gate["check_receipt"](r, "retire", NOW))

    def test_missing_destination_exits_before_startup(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = Path(tmp) / "report.json"
            result = subprocess.run([sys.executable, "-B", Path(__file__).with_name("writer-rollback.py"),
                                     "--binary", "kernel", "--build-commit", COMMIT, "--baseline", tmp,
                                     "--mc", "mc", "--report", report], env={"PATH": os.defpath}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"SKIP directory and S3", result.stderr)
            self.assertEqual(json.loads(report.read_bytes())["omitted"], ["directory", "s3"])

    def test_detached_signatures_and_restricted_output(self):
        with tempfile.TemporaryDirectory() as temp:
            r = example()
            r["material"] = "synthetic"
            receipt, keys = sign_receipt(Path(temp), r)
            self.assertIn("receipt material = synthetic", gate["review"](receipt, keys, "retire"))
            wrong = dict(keys, operations=keys["maintenance"])
            reasons = gate["review"](receipt, wrong, "retire")
            self.assertIn("untrusted signing key for operations", reasons)
            self.assertIn("submitted receipt is labelled synthetic", reasons)
            self.assertIn("submitted maintenance signature is labelled lab", reasons)
            receipt.write_text(receipt.read_text() + " ")
            self.assertIn("invalid signature for operations", gate["review"](receipt, keys, "retire"))
            self.assertNotIn(r["remoteCopy"], json.dumps(reasons))


if __name__ == "__main__":
    unittest.main()
