"""Permissions and receipts for the disposable writer drill on macOS."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import shlex
import subprocess
from urllib.parse import urlsplit


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def sandbox(path, writable, hidden=()):
    profile = path / "process.sb"
    rules = ['(version 1)', '(allow default)', '(deny network*)',
             '(allow network-bind network-inbound (local ip "localhost:*"))',
             '(allow network-outbound (remote tcp "localhost:*"))', '(deny file-write*)',
             '(allow file-write* (literal "/dev/null"))']
    for p in writable:
        rules.append('(allow file-write* (subpath ' + json.dumps(str(p.resolve())) + '))')
    for p in hidden:
        rules.append('(deny file-read* file-write* (subpath ' + json.dumps(str(p.resolve())) + '))')
    path.mkdir()
    profile.write_text("\n".join(rules))
    return ["/usr/bin/sandbox-exec", "-f", str(profile)]


def wrap(binary, directory, command):
    wrapper = directory / binary.name
    wrapper.write_text("#!/bin/sh\nexec " + shlex.join([*command, str(binary)]) + ' "$@"\n')
    wrapper.chmod(0o700)
    return wrapper


class Minio:
    def __init__(self, mc, root, settings):
        self.mc = mc
        self.root = root
        self.settings = settings
        self.bucket = "konae-test"
        self.admin = {k: os.environ[k] for k in ("MC_CONFIG_DIR", "MC_HOST_lab")}
        self.admin["PATH"] = os.defpath

    def call(self, *args, env=None, data=None, denied=False):
        result = subprocess.run([self.mc, "--json", *args], env=self.admin if env is None else env,
                                input=data, capture_output=True, timeout=90)
        output = result.stdout + result.stderr
        if denied:
            assert result.returncode and (b"AccessDenied" in output or b"Insufficient permissions" in output or b"Access Denied" in output), args[0]
        elif result.returncode:
            # The admin user creation arguments include a disposable password.
            raise RuntimeError("MinIO failed " + args[0])
        return result.stdout

    def user(self, bucket, prefix, writes):
        name, password = "drill-" + secrets.token_hex(6), secrets.token_hex(24)
        arn = "arn:aws:s3:::" + bucket
        statements = [
            {"Effect": "Allow", "Action": "s3:GetBucketLocation", "Resource": arn},
            {"Effect": "Allow", "Action": "s3:ListBucket", "Resource": arn,
             "Condition": {"StringLike": {"s3:prefix": prefix + "*"}}},
            {"Effect": "Allow", "Action": "s3:GetObject", "Resource": arn + "/" + prefix + "*"},
        ]
        if writes:
            statements.append({"Effect": "Allow", "Action": ["s3:PutObject", "s3:DeleteObject"],
                               "Resource": arn + "/" + writes + "*"})
        policy = self.root / (name + ".json")
        policy.write_text(json.dumps({"Version": "2012-10-17", "Statement": statements}))
        self.call("admin", "user", "add", "lab", name, password)
        self.call("admin", "policy", "create", "lab", name, policy)
        self.call("admin", "policy", "attach", "lab", name, "--user", name)
        endpoint = self.settings["KONAE_S3_ENDPOINT"]
        port = urlsplit(endpoint).port
        reader = {"PATH": os.defpath, "MC_CONFIG_DIR": str(self.root / (name + "-mc")),
                  "MC_HOST_user": f"http://{name}:{password}@127.0.0.1:{port}"}
        return reader, dict(KONAE_S3_ENDPOINT=endpoint, KONAE_S3_BUCKET=bucket,
                            KONAE_S3_ACCESS_KEY=name, KONAE_S3_SECRET_KEY=password)

    def files(self, prefix):
        entries = self.call("ls", "--recursive", "lab/" + self.bucket + "/" + prefix)
        return sorted(json.loads(line)["key"] for line in entries.splitlines())

    def deny_changes(self, credentials, bucket, prefix, keys):
        env = dict(PATH=os.defpath, AWS_CONFIG_FILE=os.devnull, AWS_SHARED_CREDENTIALS_FILE=os.devnull,
                   AWS_REGION="us-east-1", AWS_EC2_METADATA_DISABLED="true",
                   AWS_ACCESS_KEY_ID=credentials["KONAE_S3_ACCESS_KEY"],
                   AWS_SECRET_ACCESS_KEY=credentials["KONAE_S3_SECRET_KEY"])
        aws = shutil.which("aws")
        if not aws:
            raise RuntimeError("AWS CLI is required for direct PutObject and DeleteObject probes")
        for key in keys:
            for operation in ("put-object", "delete-object"):
                result = subprocess.run([aws, "--endpoint-url", credentials["KONAE_S3_ENDPOINT"], "s3api",
                                         operation, "--bucket", bucket, "--key", prefix + key],
                                        env=env, capture_output=True, timeout=20)
                assert result.returncode and b"(AccessDenied)" in result.stderr, operation


def sign_receipt(root, receipt):
    path = root / "restricted.json"
    signers = receipt["signers"] = {}
    keys = {}
    for role in ("operations", "maintenance"):
        private = root / (role + ".pem")
        public = root / (role + ".pub")
        subprocess.run(["openssl", "ecparam", "-genkey", "-name", "prime256v1", "-noout", "-out", private], check=True)
        subprocess.run(["openssl", "pkey", "-in", private, "-pubout", "-out", public], check=True)
        der = subprocess.check_output(["openssl", "pkey", "-pubin", "-in", public, "-outform", "DER"])
        signers[role] = dict(authority="lab", keySha256=hashlib.sha256(der).hexdigest())
        keys[role] = public
    path.write_text(json.dumps(receipt, sort_keys=True) + "\n")
    for role in keys:
        subprocess.run(["openssl", "dgst", "-sha256", "-sign", root / (role + ".pem"), "-out",
                        str(path) + "." + role + ".sig", path], check=True)
    return path, keys
