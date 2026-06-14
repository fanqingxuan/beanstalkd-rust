import os
import signal
import socket
import subprocess
import sys
import time

import pytest

greenstalk = pytest.importorskip("greenstalk")


@pytest.fixture
def client(server_factory):
    server = server_factory()
    conn = greenstalk.Client(server.address)
    try:
        yield conn
    finally:
        conn.close()


def wait_for(predicate, timeout=3.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    return predicate()


def test_core_lifecycle_uses_standard_client_api(client):
    job_id = client.put("hello", priority=5, delay=0, ttr=2)

    job = client.reserve(timeout=0)
    assert job.id == job_id
    assert job.body == "hello"

    stats = client.stats_job(job)
    assert stats["id"] == job_id
    assert stats["state"] == "reserved"
    assert stats["tube"] == "default"

    client.touch(job)
    client.delete(job)

    with pytest.raises(greenstalk.NotFoundError):
        client.peek(job_id)


def test_tubes_priority_and_watch_lists_match_client_expectations(client):
    client.use("low")
    low_id = client.put("low", priority=100, delay=0, ttr=5)
    client.use("high")
    high_id = client.put("high", priority=1, delay=0, ttr=5)

    assert client.using() == "high"
    assert {"default", "low", "high"}.issubset(set(client.tubes()))

    assert client.watch("low") >= 2
    assert client.watch("high") >= 2
    assert client.ignore("default") >= 2
    assert {"low", "high"} == set(client.watching())

    first = client.reserve(timeout=0)
    second = client.reserve(timeout=0)

    assert (first.id, first.body) == (high_id, "high")
    assert (second.id, second.body) == (low_id, "low")

    client.delete(first)
    client.delete(second)


def test_delay_release_bury_kick_and_peek_with_client_api(client):
    job_id = client.put("later", priority=10, delay=1, ttr=2)
    delayed = client.peek_delayed()
    assert delayed.id == job_id

    with pytest.raises(greenstalk.TimedOutError):
        client.reserve(timeout=0)

    job = client.reserve(timeout=2)
    assert job.body == "later"

    client.release(job, priority=20, delay=0)
    job = client.reserve(timeout=0)
    client.bury(job, priority=30)

    buried = client.peek_buried()
    assert buried.id == job_id
    assert client.kick(1) == 1

    ready = client.peek_ready()
    assert ready.id == job_id
    job = client.reserve(timeout=0)
    assert job.id == job_id
    client.delete(job)


def test_pause_tube_blocks_reserve_until_unpaused(client):
    client.put("paused", priority=1, delay=0, ttr=5)
    client.pause_tube("default", 1)

    started = time.monotonic()
    job = client.reserve(timeout=2)
    elapsed = time.monotonic() - started

    assert elapsed >= 0.8
    assert job.body == "paused"
    client.delete(job)


def test_empty_tubes_are_removed_like_the_c_server(client):
    client.use("transient")
    job_id = client.put("tmp", priority=1, delay=0, ttr=5)
    client.delete(job_id)

    client.use("default")
    assert "transient" not in client.tubes()
    with pytest.raises(greenstalk.NotFoundError):
        client.stats_tube("transient")


def test_sigusr1_enters_drain_mode(server_factory):
    server = server_factory()
    client = greenstalk.Client(server.address)
    try:
        os.kill(server.proc.pid, signal.SIGUSR1)

        stats = wait_for(lambda: client.stats() if client.stats().get("draining") else None)
        assert str(stats["draining"]).lower() == "true"

        with pytest.raises(greenstalk.DrainingError):
            client.put("no new jobs")
    finally:
        client.close()


def test_rust_wal_recovers_jobs_without_client_code_changes(server_factory, tmp_path):
    wal_dir = tmp_path / "wal"
    server = server_factory("-b", wal_dir)
    client = greenstalk.Client(server.address)
    try:
        job_id = client.put("persisted", priority=7, delay=0, ttr=5)
    finally:
        client.close()
        server.stop()

    restarted = server_factory("-b", wal_dir)
    client = greenstalk.Client(restarted.address)
    try:
        job = client.reserve(timeout=0)
        assert job.id == job_id
        assert job.body == "persisted"
        client.delete(job)
    finally:
        client.close()


def test_rust_reads_c_binary_binlog(c_beanstalkd_bin, server_factory, tmp_path):
    wal_dir = tmp_path / "c-wal"
    wal_dir.mkdir()

    port = _free_port()
    c_proc = subprocess.Popen(
        [c_beanstalkd_bin, "-l", "127.0.0.1", "-p", str(port), "-b", str(wal_dir)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        _wait_for_port(port, c_proc)
        c_client = greenstalk.Client(("127.0.0.1", port))
        try:
            c_client.use("migrated")
            ready_id = c_client.put("from-c-ready", priority=11, delay=0, ttr=9)
            buried_id = c_client.put("from-c-buried", priority=12, delay=0, ttr=9)
            buried = c_client.reserve_job(buried_id)
            c_client.bury(buried, priority=12)
        finally:
            c_client.close()
    finally:
        c_proc.terminate()
        c_proc.wait(timeout=5)

    rust_server = server_factory("-b", wal_dir)
    client = greenstalk.Client(rust_server.address)
    try:
        client.use("migrated")
        client.watch("migrated")
        client.ignore("default")

        ready = client.reserve(timeout=0)
        assert ready.id == ready_id
        assert ready.body == "from-c-ready"
        client.delete(ready)

        buried = client.peek_buried()
        assert buried.id == buried_id
        assert buried.body == "from-c-buried"
        assert client.kick(1) == 1
        kicked = client.reserve(timeout=0)
        assert kicked.id == buried_id
        client.delete(kicked)
    finally:
        client.close()


def test_systemd_socket_activation_fd_is_used(beanstalkd_bin):
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)
    port = listener.getsockname()[1]

    wrapper = (
        "import os, sys;"
        "os.dup2(int(os.environ['LISTEN_FD_SRC']), 3);"
        "os.environ['LISTEN_PID']=str(os.getpid());"
        "os.environ['LISTEN_FDS']='1';"
        "os.environ.pop('LISTEN_FD_SRC', None);"
        "os.execv(sys.argv[1], sys.argv[1:])"
    )
    env = os.environ.copy()
    env["LISTEN_FD_SRC"] = str(listener.fileno())
    proc = subprocess.Popen(
        [
            sys.executable,
            "-c",
            wrapper,
            beanstalkd_bin,
            "-l",
            "127.0.0.1",
            "-p",
            "1",
        ],
        env=env,
        pass_fds=(listener.fileno(),),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    listener.close()
    try:
        _wait_for_port(port, proc)
        client = greenstalk.Client(("127.0.0.1", port))
        try:
            job_id = client.put("activated", priority=1, delay=0, ttr=5)
            job = client.reserve(timeout=0)
            assert job.id == job_id
            assert job.body == "activated"
            client.delete(job)
        finally:
            client.close()
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


def _free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _wait_for_port(port, proc, timeout=5.0):
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
