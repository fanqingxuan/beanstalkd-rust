import subprocess


def run_cli(beanstalkctl_bin, server, *args):
    return subprocess.run(
        [beanstalkctl_bin, "--addr", f"127.0.0.1:{server.port}", *args],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def test_beanstalkctl_put_reserve_delete(server_factory, beanstalkctl_bin):
    server = server_factory()

    put = run_cli(
        beanstalkctl_bin,
        server,
        "put",
        "--tube",
        "cli-test",
        "--pri",
        "10",
        "--ttr",
        "30",
        "hello-from-cli",
    )
    assert put.stdout == "INSERTED 1\n"

    reserve = run_cli(
        beanstalkctl_bin,
        server,
        "reserve",
        "--watch",
        "cli-test",
        "--timeout",
        "1",
        "--delete",
    )
    assert "status: reserved\n" in reserve.stdout
    assert "job id: 1\n" in reserve.stdout
    assert "bytes: 14\n" in reserve.stdout
    assert "body:\nhello-from-cli\n" in reserve.stdout
    assert "DELETED\n" in reserve.stdout

    stats = run_cli(beanstalkctl_bin, server, "stats")
    assert "cmd-put: 1\n" in stats.stdout


def test_beanstalkctl_peek_prints_readable_job_without_consuming(server_factory, beanstalkctl_bin):
    server = server_factory()

    run_cli(beanstalkctl_bin, server, "put", "name")

    peek = run_cli(beanstalkctl_bin, server, "peek", "1")
    assert peek.stdout == "status: found\njob id: 1\nbytes: 4\nbody:\nname\n"

    reserve = run_cli(beanstalkctl_bin, server, "reserve", "--timeout", "1")
    assert "status: reserved\n" in reserve.stdout
    assert "job id: 1\n" in reserve.stdout
    assert "body:\nname\n" in reserve.stdout


def test_beanstalkctl_repl_mode(server_factory, beanstalkctl_bin):
    server = server_factory()

    repl = subprocess.run(
        [beanstalkctl_bin, "--addr", f"127.0.0.1:{server.port}", "repl"],
        input=(
            'put --tube repl-test "hello interactive"\n'
            "reserve --watch repl-test --timeout 1 --delete\n"
            "exit\n"
        ),
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    assert "beanstalkctl> " in repl.stdout
    assert "INSERTED 1\n" in repl.stdout
    assert "status: reserved\n" in repl.stdout
    assert "job id: 1\n" in repl.stdout
    assert "bytes: 17\n" in repl.stdout
    assert "body:\nhello interactive\n" in repl.stdout
    assert "DELETED\n" in repl.stdout
    assert repl.stderr == ""


def test_beanstalkctl_without_command_starts_repl(server_factory, beanstalkctl_bin):
    server = server_factory()

    repl = subprocess.run(
        [beanstalkctl_bin, "--addr", f"127.0.0.1:{server.port}"],
        input="put hello-default-repl\nexit\n",
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    assert "beanstalkctl> " in repl.stdout
    assert "INSERTED 1\n" in repl.stdout
    assert repl.stderr == ""
