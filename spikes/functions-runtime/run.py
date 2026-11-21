#!/usr/bin/env python3
"""Start disposable servers, verify behavior, and run serial measurements."""

import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from benchmark import connection


ROOT = Path(__file__).resolve().parent


def wait_ready(url, process, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Server exited with {process.returncode}; inspect its log")
        conn = connection(url)
        try:
            conn.request("GET", "/health")
            response = conn.getresponse()
            if response.status == 200 and json.loads(response.read()) == {"ok": True}:
                return
        except (OSError, ValueError):
            pass
        finally:
            conn.close()
        time.sleep(0.1)
    raise TimeoutError(f"Server did not become ready at {url}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-port", type=int, default=43101)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--requests", type=int, default=5000)
    parser.add_argument("--concurrency-requests", type=int, default=4000)
    parser.add_argument("--allow-next-malformed-url", action="store_true",
                        help="Retain Next's known malformed-URL failures but measure valid requests if every other check passes")
    parser.add_argument("--hold", action="store_true", help="Leave servers running for separate profiling; caller must stop their process groups")
    args = parser.parse_args()
    if not args.binary.is_file():
        parser.error("--binary must name a built Rust experiment executable")
    output = args.output.resolve()
    if output.exists():
        parser.error("--output must be a new directory")
    output.mkdir(parents=True)
    node = shutil.which("node")
    if not node:
        parser.error("node is required")
    processes = []
    files = []
    targets = []
    hold = False
    try:
        for offset, name in enumerate(["native", "deno", "node", "next"]):
            data = output / "data" / name
            data.mkdir(parents=True)
            port = args.base_port + offset
            env = {
                **os.environ,
                "PORT": str(port), "DATA_DIR": str(data), "NODE_ENV": "production",
                "NEXT_TELEMETRY_DISABLED": "1", "MODE": name,
                "FUNCTIONS_ENABLED": "1", "CACHE_CAPACITY": "4", "CACHE_TTL_MS": "60000",
                "JS_TIMEOUT_MS": "2000", "APP_BUNDLE_DIR": str(ROOT / "app" / "dist"),
            }
            if name in {"native", "deno"}:
                command = [str(args.binary.resolve())]
                cwd = ROOT
            else:
                command = [node, str(ROOT / "baselines" / name / ("server.mjs" if name == "node" else "start.mjs"))]
                cwd = ROOT / "baselines"
            log = (output / f"{name}.log").open("w")
            files.append(log)
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            processes.append(process)
            target = {"name": name, "url": f"http://127.0.0.1:{port}", "pid": process.pid, "data_dir": str(data)}
            targets.append(target)
            (output / "targets.json").write_text(json.dumps(targets, indent=2) + "\n")
            wait_ready(target["url"], process)

        versions = {
            "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
            "node": subprocess.check_output([node, "--version"], text=True).strip(),
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "binary": str(args.binary.resolve()), "binary_bytes": args.binary.stat().st_size,
            "server_commands": [p.args for p in processes],
            "app_manifest": json.loads((ROOT / "app" / "package.json").read_text()),
            "baseline_manifest": json.loads((ROOT / "baselines" / "package.json").read_text()),
        }
        (output / "versions.json").write_text(json.dumps(versions, indent=2) + "\n")
        subprocess.run([sys.executable, str(ROOT / "observe.py"), "--targets", str(output / "targets.json"),
                        "--output", str(output / "idle-before.json"), "--cold-rounds", "0"], check=True)

        divergences = []
        for target in targets:
            result = subprocess.run([
                sys.executable, str(ROOT / "checks" / "verify.py"),
                "--url", target["url"], "--data-dir", target["data_dir"],
                "--name", target["name"], "--output", str(output / f"verify-{target['name']}.json"),
            ])
            if result.returncode:
                report = json.loads((output / f"verify-{target['name']}.json").read_text())
                failures = [item for item in report["results"] if item["status"] == "fail"]
                names = {f"{app}.id.{method}.%ZZ" for app in ("a", "b") for method in ("GET", "POST")}
                known = (
                    args.allow_next_malformed_url and target["name"] == "next" and failures
                    and all(item["criterion"] in names
                            and item["detail"].get("database_ok") is True
                            and item["detail"].get("expected_status") == 400
                            and item["detail"].get("response", {}).get("status") == 500
                            for item in failures)
                )
                if not known:
                    raise RuntimeError(f"Verification failed for {target['name']}; inspect its JSON report")
                divergences.extend(failures)
        (output / "baseline-divergences.json").write_text(json.dumps(divergences, indent=2) + "\n")

        for concurrency, count, name in [(1, args.requests, "sequential"), (8, args.concurrency_requests, "concurrency-8")]:
            subprocess.run([
                sys.executable, str(ROOT / "benchmark.py"), "--targets", str(output / "targets.json"),
                "--output", str(output / name), "--rounds", str(args.rounds),
                "--requests", str(count), "--concurrency", str(concurrency),
            ], check=True)
        subprocess.run([sys.executable, str(ROOT / "observe.py"), "--targets", str(output / "targets.json"),
                        "--output", str(output / "idle-after-and-cold.json")], check=True)
        hold = args.hold
    finally:
        if not hold:
            for process in processes:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for process in processes:
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for file in files:
            file.close()
    print(f"Results: {output}")
    if hold:
        print("Server process groups remain running; use targets.json to stop them.")


if __name__ == "__main__":
    main()
