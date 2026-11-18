#!/usr/bin/env python3
"""Test a writer on fresh canaries in a versioned copy bucket. Leaves both for retention."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
from tempfile import TemporaryDirectory


class ProbeFailure(Exception):
    pass


def probe(args):
    # A named profile selects each identity. Environment credentials can belong to the operator.
    env = {k: v for k, v in os.environ.items() if not k.startswith("AWS_") or k in
           {"AWS_CONFIG_FILE", "AWS_SHARED_CREDENTIALS_FILE", "AWS_CA_BUNDLE"}}
    env.update(AWS_EC2_METADATA_DISABLED="true", AWS_PAGER="", AWS_MAX_ATTEMPTS="1")
    command = ["aws", "--region", args.region, "--output", "json"]
    if args.endpoint:
        command += ["--endpoint-url", args.endpoint]

    def s3(profile, operation, key, *options, denied=False):
        result = subprocess.run([*command, "--profile", profile, "s3api", operation, "--bucket",
                                 args.bucket, "--key", key, *options], env=env, capture_output=True, timeout=60)
        if denied:
            if result.returncode == 0:
                raise ProbeFailure("writer-deleted-canary" if profile == args.writer else "reader-wrote-canary")
            api = {"delete-object": "DeleteObject", "put-object": "PutObject"}[operation]
            if f"An error occurred (AccessDenied) when calling the {api} operation".encode() not in result.stderr:
                raise ProbeFailure("access-denial-unconfirmed")
            return
        if result.returncode:
            raise ProbeFailure(f"{operation}-failed")
        return json.loads(result.stdout or b"{}")

    if args.reader == args.writer:
        raise ProbeFailure("profiles-must-differ")
    count = 0
    with TemporaryDirectory(prefix="konae-canary-") as tmp:
        body = Path(tmp) / "canary"
        body.write_bytes(secrets.token_bytes(32))
        suffix = "drill-" + secrets.token_hex(16) + ".canary"
        for prefix in ["copies/drill-canaries/", "copies/revocations/"]:
            key = prefix + suffix
            created = s3(args.writer, "put-object", key, "--body", str(body))
            version = created.get("VersionId")
            if not version or version == "null":
                raise ProbeFailure("bucket-versioning-required")
            for options in [[], ["--version-id", version], ["--version-id", version, "--bypass-governance-retention"]]:
                s3(args.writer, "delete-object", key, *options, denied=True)
                count += 1
            s3(args.reader, "put-object", key, "--body", str(body), denied=True)
            s3(args.reader, "delete-object", key, "--version-id", version, denied=True)
            restored = Path(tmp) / "readback"
            current = s3(args.reader, "get-object", key, str(restored))
            if current.get("VersionId") != version or restored.read_bytes() != body.read_bytes():
                raise ProbeFailure("current-version-changed")
    return count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ["bucket", "reader", "writer"]:
        parser.add_argument("--" + flag, required=True)
    parser.add_argument("--endpoint")
    parser.add_argument("--region", default="us-east-1")
    args = parser.parse_args()
    try:
        count = probe(args)
        print(json.dumps({"result": "passed", "canariesReadBack": 2, "deniedDeleteRequests": count, "deniedReaderWrites": 4}))
    except (ProbeFailure, OSError, ValueError, subprocess.SubprocessError) as error:
        print(json.dumps({"result": "failed", "reason": str(error) if isinstance(error, ProbeFailure) else type(error).__name__}))
        sys.exit(1)
