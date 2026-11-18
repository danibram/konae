"""Mirror a stationary synthetic MinIO repository using only emergency read access."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
from urllib.parse import urlsplit


def mirror(mc, prefix, destination):
    endpoint = urlsplit(os.environ["KONAE_TEST_S3_ENDPOINT"])
    admin = urlsplit(os.environ["MC_HOST_lab"])
    assert endpoint.scheme == admin.scheme == "http" and endpoint.hostname == admin.hostname == "127.0.0.1"
    bucket = "konae-test"
    assert os.environ["KONAE_TEST_S3_BUCKET"] == bucket
    assert prefix.startswith("copies-v3/") and prefix.endswith("/")
    username, password = "reader-"+secrets.token_hex(6), secrets.token_hex(24)

    def configure(*args):
        result = subprocess.run([mc, "admin", *args], capture_output=True, timeout=30)
        assert result.returncode == 0, "Failed to configure the disposable read-only user"

    with tempfile.TemporaryDirectory() as tmp:
        configure("user", "add", "lab", username, password)
        policy = {"Version": "2012-10-17", "Statement": [
            {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket"],
             "Resource": [f"arn:aws:s3:::{bucket}"]},
            {"Effect": "Allow", "Action": ["s3:GetObject"],
             "Resource": [f"arn:aws:s3:::{bucket}/{prefix}*"]},
        ]}
        path = Path(tmp)/"policy.json"
        path.write_text(json.dumps(policy))
        configure("policy", "create", "lab", username, path)
        configure("policy", "attach", "lab", username, "--user", username)
        reader = {"PATH": os.defpath, "MC_CONFIG_DIR": str(Path(tmp)/"config"),
                  "MC_HOST_reader": f"http://{username}:{password}@{admin.hostname}:{admin.port}"}
        source = f"reader/{bucket}/{prefix}"
        denied = subprocess.run([mc, "pipe", source+"denied"], input=b"x", capture_output=True,
                                env=reader, timeout=30)
        assert denied.returncode and b"denied" in denied.stderr.lower()
        subprocess.run([mc, "mirror", source, destination], env=reader, capture_output=True,
                       check=True, timeout=120)
