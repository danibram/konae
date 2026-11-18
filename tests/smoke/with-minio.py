"""Run a test command against its own loopback MinIO process and empty bucket."""
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
from urllib.request import urlopen

import runpy

wait = runpy.run_path(str(Path(__file__).with_name("common-cut.py")))["wait"]


def main(command):
    if not command:
        raise SystemExit("Pass a test command. Install minio and mc on PATH.")
    with tempfile.TemporaryDirectory(prefix="recovery-minio-") as tmp:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{sock.getsockname()[1]}"
        endpoint = "http://" + address
        access, secret = secrets.token_hex(16), secrets.token_hex(24)
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("KONAE_", "AWS_", "MINIO_", "MC_"))}
        env.update(MINIO_ROOT_USER=access, MINIO_ROOT_PASSWORD=secret, MC_CONFIG_DIR=tmp + "/mc",
                   MINIO_BROWSER="off", MINIO_UPDATE="off", MC_HOST_lab=f"http://{access}:{secret}@{address}",
                   AWS_EC2_METADATA_DISABLED="true")
        server = subprocess.Popen(["minio", "server", "--address", address, tmp + "/objects"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            def ready():
                if server.poll() is not None:
                    raise RuntimeError("MinIO exited during startup")
                try:
                    with urlopen(endpoint + "/minio/health/ready", timeout=1) as response:
                        return response.status == 200
                except OSError:
                    return False

            wait(ready, "MinIO did not start", 30)
            subprocess.run(["mc", "mb", "lab/konae-test"], env=env, check=True, capture_output=True)
            env.update(KONAE_TEST_S3_BUCKET="konae-test", KONAE_TEST_S3_ENDPOINT=endpoint,
                       KONAE_TEST_S3_ACCESS_KEY=access, KONAE_TEST_S3_SECRET_KEY=secret)
            return subprocess.run(command, env=env).returncode
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
