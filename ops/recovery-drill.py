#!/usr/bin/env python3
"""Restore one retained point inside a private, encrypted macOS disk image."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import resource
import secrets
import signal
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit


class DrillFailure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise DrillFailure(message)


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def disk(*args, password=None):
    result = subprocess.run(["/usr/bin/hdiutil", *map(str, args)], input=password or b"",
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    require(result.returncode == 0, f"disk-{args[0]}-failed")
    return result.stdout


def attached(image):
    info = plistlib.loads(disk("info", "-plist"))
    return [entity["dev-entry"] for item in info["images"]
            if Path(item["image-path"]).resolve() == image
            for entity in item["system-entities"] if "dev-entry" in entity]


@contextlib.contextmanager
def volume(evidence, size):
    work = Path(tempfile.mkdtemp(prefix="konae-drill-")).resolve()
    image = work / "scratch.sparseimage"
    mount = work / "volume"
    mount.mkdir(mode=0o700)
    password = secrets.token_hex(32).encode() + b"\0"
    evidence["volumeRemoved"] = False
    try:
        disk("create", "-size", f"{size}g", "-type", "SPARSE", "-fs", "Case-sensitive Journaled HFS+",
             "-nospotlight", "-volname", "konae-drill", "-encryption", "AES-256",
             "-stdinpass", image, password=password)
        require(plistlib.loads(disk("isencrypted", "-plist", image))["encrypted"], "scratch-unencrypted")
        disk("attach", "-nobrowse", "-owners", "on", "-mountpoint", mount, "-stdinpass", image,
             password=password)
        require(os.path.ismount(mount), "scratch-not-mounted")
        mount.chmod(0o700)
        private = mount / "private"
        private.mkdir(mode=0o700)
        require(private.stat().st_mode & 0o077 == 0, "scratch-permissions")
        yield private
    finally:
        devices = attached(image)
        if devices:
            disk("detach", devices[0])
        require(not attached(image) and not os.path.ismount(mount), "scratch-still-attached")
        image.unlink(missing_ok=True)
        if mount.exists():
            mount.rmdir()
        work.rmdir()
        evidence["volumeRemoved"] = not work.exists()


def restricted(binary, root, args, env, reads=(), port=None):
    rules = ['(version 1)', '(deny default)', '(import "system.sb")',
             '(deny network*)', '(allow sysctl-read file-read-metadata)',
             '(allow file-read* file-map-executable process-exec (literal (param "BINARY")))',
             '(allow file-read* file-write* (subpath (param "SCRATCH")))']
    params = ["-D", f"BINARY={binary}", "-D", f"SCRATCH={root}"]
    for i, path in enumerate(reads):
        rules.append(f'(allow file-read* (subpath (param "READ{i}")))')
        params += ["-D", f"READ{i}={path}"]
    if port:
        rules.append(f'(allow network-outbound (remote tcp "localhost:{port}"))')
    command = ["/usr/bin/sandbox-exec", *params, "-p", "\n".join(rules), str(binary), *args]
    return subprocess.run(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                          capture_output=True, timeout=1800)


def exercise(args, report):
    point = args.copy
    require(re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d\.\d{3}Z", point), "invalid-point")
    require(re.fullmatch(r"[a-f0-9]{64}", args.witness), "invalid-witness")
    report["pointFingerprint"] = hashlib.sha256(point.encode()).hexdigest()
    binary = args.binary.resolve(strict=True)
    report["binarySha256"] = digest(binary)
    require(report["binarySha256"] == args.binary_sha256, "binary-digest-mismatch")
    require(binary.name in {"konae", "kernel"}, "unknown-binary")
    report["binary"] = binary.name
    port = None
    reads = []
    credentials = {}
    if args.source.startswith("s3://"):
        source = args.source
        location = urlsplit(source)
        require(re.fullmatch(r"[a-z0-9.-]+", location.netloc) and location.path == "/copies/"
                and not location.query and not location.fragment, "invalid-copy-prefix")
        endpoint = urlsplit(args.endpoint or "")
        require(endpoint.scheme == "http" and endpoint.hostname == "127.0.0.1" and endpoint.port
                and endpoint.netloc == f"127.0.0.1:{endpoint.port}" and endpoint.path in {"", "/"}
                and not endpoint.query and not endpoint.fragment, "need-dedicated-loopback-endpoint")
        port = endpoint.port
        # The AWS client initializes its TLS roots even for a loopback relay.
        certs = Path("/etc/ssl/cert.pem").resolve(strict=True)
        reads = [certs]
        credentials = {"SSL_CERT_FILE": str(certs), "KONAE_S3_BUCKET": location.netloc, "KONAE_S3_ENDPOINT": args.endpoint,
                       "KONAE_S3_REGION": args.region, "KONAE_S3_PATH_STYLE": "1"}
        for key in ["KONAE_S3_ACCESS_KEY", "KONAE_S3_SECRET_KEY"]:
            credentials[key] = os.environ.get(key, "")
            require(credentials[key], "missing-reader-credentials")
        report["sourceType"] = "sealed-s3"
    else:
        require(args.endpoint is None, "unexpected-endpoint")
        directory = Path(args.source).resolve(strict=True)
        source = str(directory)
        reads = [directory / point, directory / "revocations"]
        require(all(p.is_dir() and not p.is_symlink() for p in reads), "missing-directory-input")
        report["sourceType"] = "directory"
    with volume(report, args.size_gib) as root:
        data = root / "data"
        data.mkdir()
        env = {"PATH": "/usr/bin:/bin", "TMPDIR": str(root), "SQLITE_TMPDIR": str(root),
               "KONAE_DATA_DIR": str(data), "KONAE_EXECUTION_MODE": "test", "KONAE_BIND": "127.0.0.1"}
        restore = ["restore", "--from", source, "--copy", point, "--require-revocation", args.witness]
        if port:
            refusal = restricted(binary, root, restore, env | credentials, reads, port)
            require(refusal.returncode != 0 and b"is encrypted. Set KONAE_COPY_SECRET" in refusal.stderr
                    and {p.name for p in data.iterdir()} <= {"kernel.lock"}, "missing-key-check-failed")
            report["missingKeyRefused"] = True
            credentials["KONAE_COPY_SECRET"] = os.environ.get("KONAE_COPY_SECRET", "")
            require(credentials["KONAE_COPY_SECRET"], "custody-key-unavailable")
        result = restricted(binary, root, restore, env | credentials, reads, port)
        if result.returncode:
            reasons = {
                b"does not open copy": "wrong-custody-key",
                b"absent from the verified record": "missing-witness",
                b"must follow the selected cut": "witness-before-cut",
                b"already holds at the cut": "witness-already-applied",
                b"no MAC": "unsigned-input",
                b"carries no MAC": "unsigned-input",
                b"does not verify": "invalid-signature",
                b"could not be read": "record-unavailable",
                b"does not match its checksum": "damaged-file",
            }
            raise DrillFailure(next((label for text, label in reasons.items() if text in result.stderr),
                                    "restore-failed"))
        summary = re.search(rb"(\d+) Deployments, (\d+) files, checksums verified", result.stdout)
        changed = re.search(rb"Applied the revocation record: (\d+) rows changed", result.stdout)
        require(summary and changed and not (data / "recovery-hold.json").exists(), "unreconciled")
        report.update(deployments=int(summary[1]), files=int(summary[2]), reconciledRows=int(changed[1]))
        require(report["files"] > 0 and report["reconciledRows"] > 0, "empty-recovery")
        report["postCutRevocationApplied"] = True
        loaded = restricted(binary, root, ["drill-check"], env)
        require(loaded.returncode == 0, "isolated-health-check-failed")
        check = json.loads(loaded.stdout)
        require(check["loadedDeployments"] == report["deployments"], "deployment-count-mismatch")
        require(re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][a-zA-Z0-9.-]+)?", check["version"]), "unknown-version")
        report.update(version=check["version"], workerlessHealth=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--binary-sha256", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--endpoint", help="Dedicated S3 recovery relay on loopback")
    parser.add_argument("--region", default="us-east-1")
    parser.add_argument("--copy", required=True)
    parser.add_argument("--witness", required=True, help="Custodied SHA-256 of a post-cut revocation entry")
    parser.add_argument("--scenario", choices=["primary", "independent"], required=True)
    parser.add_argument("--material", choices=["lab", "installation"], required=True)
    parser.add_argument("--size-gib", type=int, default=2)
    args = parser.parse_args()
    os.umask(0o077)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    start = time.monotonic()
    report = {"schema": 1, "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "scenario": args.scenario, "material": args.material}
    try:
        require(sys.platform == "darwin" and os.geteuid() != 0, "need-unprivileged-macos")
        require(args.size_gib > 0, "invalid-volume-size")
        exercise(args, report)
        report["result"] = "passed"
    except DrillFailure as error:
        report.update(result="failed", reason=str(error))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        # Paths and child output can identify applications or credentials.
        report.update(result="failed", reason=type(error).__name__)
    report["durationSeconds"] = round(time.monotonic() - start, 3)
    print(json.dumps(report, sort_keys=True))
    return int(report["result"] != "passed")


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main())
