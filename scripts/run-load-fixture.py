#!/usr/bin/env python3
"""Run a bounded load smoke against an independent disposable database/service.

Requires an already-built binary, local PostgreSQL, and the deterministic gateway
on127.0.0.1:18086. Refuses an existing obleth_load database or occupied service port.
It never reads application .env files or the other browser QA service's credentials.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
DATABASE = "obleth_load"
ORIGIN = "http://127.0.0.1:18087"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(ROOT / "target/debug/obleth-chat"))
    parser.add_argument("--output", default=str(ROOT / ".local/foundation-load.json"))
    parser.add_argument("--psql", default=shutil.which("psql") or "/tmp/obleth-postgres/root/usr/lib/postgresql/17/bin/psql")
    parser.add_argument("--pg-socket", default="/tmp/obleth-postgres/socket")
    parser.add_argument("--pg-port", type=int, default=55439)
    parser.add_argument("--pg-user", default="nobody")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        raise RuntimeError("Build the application before starting the load fixture")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 18087))
    with socket.create_connection(("127.0.0.1", 18086), timeout=2):
        pass
    pg_env = os.environ.copy()
    packaged_lib = Path("/tmp/obleth-postgres/root/usr/lib/x86_64-linux-gnu")
    if packaged_lib.is_dir() and str(args.psql).startswith("/tmp/obleth-postgres/"):
        pg_env["LD_LIBRARY_PATH"] = str(packaged_lib)

    def sql(statement):
        result = subprocess.run([args.psql, "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-h", args.pg_socket,
                                 "-p", str(args.pg_port), "-U", args.pg_user, "-d", "postgres", "-c", statement],
                                env=pg_env, text=True, capture_output=True)
        if result.returncode:
            raise RuntimeError("Disposable database operation failed")
        return result.stdout.strip()

    if sql("SELECT 1 FROM pg_database WHERE datname='obleth_load'"):
        raise RuntimeError("Refusing to reuse or remove an existing obleth_load database")
    created = False
    service = None
    result_code = 1
    with tempfile.TemporaryDirectory(prefix="obleth-load-") as directory:
        private = Path(directory)
        private.chmod(0o700)
        try:
            sql("CREATE DATABASE obleth_load")
            created = True
            key = private / "signing.pem"
            subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(key)],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            key.chmod(0o600)
            username, password = "load_fixture_admin", secrets.token_urlsafe(32)
            env = {key: value for key, value in os.environ.items() if not key.startswith("OBLETH_CHAT_")}
            env.update({
                "OBLETH_CHAT_DATABASE_URL": f"postgresql://{args.pg_user}@localhost:{args.pg_port}/{DATABASE}?host={args.pg_socket}",
                "OBLETH_CHAT_ORIGIN": ORIGIN, "OBLETH_CHAT_LISTEN": "127.0.0.1:18087",
                "OBLETH_CHAT_STATIC_DIR": str(ROOT / "web"), "OBLETH_CHAT_GATEWAY_URL": "http://127.0.0.1:18086",
                "OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE": str(key), "OBLETH_CHAT_GATEWAY_ISSUER": ORIGIN,
                "OBLETH_CHAT_BOOTSTRAP_USERNAME": username, "OBLETH_CHAT_BOOTSTRAP_PASSWORD": password,
                "OBLETH_CHAT_LOGIN_RATE_PER_MINUTE": "1000", "OBLETH_CHAT_MAX_STREAMS": "128",
                "OBLETH_CHAT_DB_CONNECTIONS": "20", "RUST_LOG": "obleth_chat=warn",
            })
            log = private / "service.log"
            log.touch(mode=0o600)
            with log.open("w") as stream:
                service = subprocess.Popen([str(binary)], cwd=ROOT, env=env, stdout=stream, stderr=stream)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if service.poll() is not None:
                    raise RuntimeError("Disposable service exited before readiness")
                try:
                    with urllib.request.urlopen(f"{ORIGIN}/readyz", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError("Disposable service did not become ready")
            credentials = private / "credentials.json"
            with credentials.open("x") as stream:
                credentials.chmod(0o600)
                json.dump({"kind": "obleth-chat-disposable-load", "database": DATABASE, "url": ORIGIN,
                           "username": username, "password": password, "servicePid": service.pid,
                           "buildProfile": "release" if "/release/" in str(binary) else "debug"}, stream)
            test = subprocess.run(["node", str(ROOT / "scripts/load-test.mjs"), "--credentials", str(credentials),
                                   "--output", str(Path(args.output).resolve()), "--users", "16", "--rounds", "3"], cwd=ROOT)
            result_code = test.returncode
        finally:
            if service is not None and service.poll() is None:
                service.send_signal(signal.SIGTERM)
                try:
                    service.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    service.kill()
                    service.wait(timeout=5)
            if created:
                sql("DROP DATABASE obleth_load WITH (FORCE)")
    return result_code


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        # No service logs, credentials, environment values, or API bodies in output.
        print(f"Local load fixture failed: {type(error).__name__}: {error}")
        raise SystemExit(1)
