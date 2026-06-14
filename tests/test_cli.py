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
    assert "RESERVED 1 14\nhello-from-cli\n" in reserve.stdout
    assert "DELETED\n" in reserve.stdout

    stats = run_cli(beanstalkctl_bin, server, "stats")
    assert "cmd-put: 1\n" in stats.stdout


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
    assert "RESERVED 1 17\nhello interactive\n" in repl.stdout
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
