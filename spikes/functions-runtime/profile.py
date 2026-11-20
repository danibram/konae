#!/usr/bin/env python3
"""Sample each owned macOS server during verified GET traffic, outside benchmarks."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import time

from benchmark import connection, database_state, process_sample, request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--targets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    sampler = shutil.which("sample")
    if not sampler:
        parser.error("This profiling script needs the macOS sample command")
    args.output.mkdir(parents=True, exist_ok=False)
    reports = []
    for target in json.loads(args.targets.read_text()):
        expected, before_audit = database_state(target["data_dir"])
        pids = process_sample(target["pid"])["pids"]
        # Next's launcher owns a single server child; sample the server itself.
        pid = next((pid for pid in pids if pid != target["pid"]), target["pid"])
        profile = args.output / f"{target['name']}.txt"
        conn = connection(target["url"])
        verified = 0
        with (args.output / f"{target['name']}-sample.log").open("w") as log:
            process = subprocess.Popen([sampler, str(pid), "3", "-file", str(profile)],
                                       stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    request(conn, "get", expected)
                    verified += 1
            finally:
                conn.close()
                process.wait(timeout=10)
        after_row, after_audit = database_state(target["data_dir"])
        if after_row != expected or after_audit != before_audit:
            raise AssertionError("GET profiling changed database state")
        reports.append({"target": target["name"], "sampled_pid": pid,
                        "sample_exit": process.returncode, "verified_requests": verified,
                        "profile": profile.name, "excluded_from_benchmark": True})
        (args.output / "results.json").write_text(json.dumps(reports, indent=2) + "\n")
        if process.returncode:
            raise RuntimeError(f"sample failed for {target['name']}; inspect its log")


if __name__ == "__main__":
    main()
