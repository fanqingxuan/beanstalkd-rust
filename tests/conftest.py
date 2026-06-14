import os
import socket
import subprocess
import time
from pathlib import Path

import pytest


def _free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _wait_until_ready(port, proc, timeout=5.0):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            stdout, stderr = proc.communicate(timeout=1)
            raise RuntimeError(
                f"beanstalkd exited early with {proc.returncode}\n"
                f"stdout={stdout!r}\nstderr={stderr!r}"
            )
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError as err:
            last_error = err
            time.sleep(0.05)
    raise RuntimeError(f"beanstalkd did not accept connections: {last_error}")


class RunningServer:
    def __init__(self, proc, port):
        self.proc = proc
        self.port = port
        self.address = ("127.0.0.1", port)

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)


@pytest.fixture(scope="session")
def beanstalkd_bin():
    configured = os.environ.get("BEANSTALKD_BIN")
    if configured:
        path = Path(configured)
    else:
        path = Path(__file__).resolve().parents[1] / "target" / "release" / "beanstalkd"
    if not path.exists():
        pytest.fail(f"beanstalkd binary not found: {path}. Run cargo build --release first.")
    return str(path)


@pytest.fixture(scope="session")
def beanstalkctl_bin():
    configured = os.environ.get("BEANSTALKCTL_BIN")
    if configured:
        path = Path(configured)
    else:
        path = Path(__file__).resolve().parents[1] / "target" / "release" / "beanstalkctl"
    if not path.exists():
        pytest.fail(f"beanstalkctl binary not found: {path}. Run cargo build --release --workspace first.")
    return str(path)


@pytest.fixture(scope="session")
def c_beanstalkd_bin():
    root = Path(__file__).resolve().parents[1]
    path = root / "source" / "beanstalkd"
    if not path.exists():
        subprocess.run(["make", "beanstalkd"], cwd=root / "source", check=True)
    if not path.exists():
        pytest.fail(f"C beanstalkd binary not found: {path}")
    return str(path)


@pytest.fixture
def server_factory(beanstalkd_bin):
    servers = []

    def start(*extra_args):
        port = _free_port()
        proc = subprocess.Popen(
            [beanstalkd_bin, "-l", "127.0.0.1", "-p", str(port), *map(str, extra_args)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        _wait_until_ready(port, proc)
        server = RunningServer(proc, port)
        servers.append(server)
        return server

    yield start

    for server in reversed(servers):
        server.stop()
