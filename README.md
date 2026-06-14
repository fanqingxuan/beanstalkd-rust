# beanstalkd-rs

`beanstalkd-rs` is a Rust rewrite of the original C beanstalkd server. The Rust
implementation lives in `src/` and builds a binary named `beanstalkd`.

The goal is operational compatibility: existing beanstalkd clients should be
able to switch from the C daemon to the Rust daemon without changing client
code.

中文文档见 [README.zh-CN.md](README.zh-CN.md).

## Status

Implemented:

- Beanstalk ASCII protocol over TCP.
- Unix socket listener through `-l unix:/path/to/socket`.
- systemd socket activation through `LISTEN_PID` / `LISTEN_FDS`.
- C-compatible startup behavior for binding before `-u` privilege drop.
- `SIGUSR1` drain mode.
- C binary binlog migration reader for `binlog.N` files, including WAL v7 and
  legacy v5.
- Rust JSONL WAL for jobs written after migration.
- Python client compatibility tests using `greenstalk`.

Known difference:

- The Rust daemon can read C-format `binlog.N` files, but it writes subsequent
  updates to `rust-beanstalkd.wal` instead of emitting new C-format binlog
  segments.

See [COMPATIBILITY_AUDIT.md](COMPATIBILITY_AUDIT.md) for the detailed audit.

## Supported Commands

Producer and worker commands:

```text
put
use
reserve
reserve-with-timeout
reserve-job
delete
release
bury
touch
```

Tube, inspection, stats, and control commands:

```text
watch
ignore
peek
peek-ready
peek-delayed
peek-buried
kick
kick-job
stats
stats-job
stats-tube
list-tubes
list-tube-used
list-tubes-watched
pause-tube
quit
```

## Build

```sh
cargo build --release
```

The binary is:

```sh
target/release/beanstalkd
```

Build the standalone command-line client:

```sh
cargo build --release -p beanstalkctl
```

The client binary is:

```sh
target/release/beanstalkctl
```

If your local `cargo` and `rustc` come from different toolchains, build with an
explicit Rust compiler:

```sh
RUSTC=$(rustup which rustc) cargo build --release
```

## GitHub Release Builds

The repository includes a GitHub Actions workflow at
`.github/workflows/release.yml`.

- Pushes to `main` and pull requests run workspace tests and release builds.
- `workflow_dispatch` lets you start a multi-platform build manually from the
  GitHub Actions page.
- Tags matching `v*`, for example `v1.13.0-rust.1`, create a GitHub Release and
  upload Linux, macOS, and Windows packages.

Each package contains:

- `beanstalkd`
- `beanstalkctl`
- `README.md`
- `README.zh-CN.md`

Windows packages support TCP listeners and TCP client connections. Unix sockets,
systemd socket activation, SIGUSR1 drain mode, and `-u USER` privilege dropping
are Unix-only features.

Create a release from your local checkout:

```sh
git tag v1.13.0-rust.1
git push origin v1.13.0-rust.1
```

## Run

Default TCP port:

```sh
target/release/beanstalkd
```

Bind to a specific address and port:

```sh
target/release/beanstalkd -l 127.0.0.1 -p 11300
```

Use a Unix socket:

```sh
target/release/beanstalkd -l unix:/tmp/beanstalkd.sock
```

Enable WAL persistence:

```sh
target/release/beanstalkd -b /var/lib/beanstalkd
```

## Command-Line Client

This repository also includes `beanstalkctl`, a standalone command-line client
in `cli/`. It does not depend on the server implementation in `src/`; it talks
to beanstalkd only through the public TCP or Unix-socket protocol.

Full CLI documentation:

- [cli/README.md](cli/README.md)
- [cli/README.zh-CN.md](cli/README.zh-CN.md)

Examples:

```sh
# Put a job into the default tube.
target/release/beanstalkctl put "hello"

# Put a job into a specific tube.
target/release/beanstalkctl put --tube emails --pri 100 --ttr 60 "send welcome email"

# Reserve a job, watching only one tube.
target/release/beanstalkctl reserve --watch emails --timeout 5

# Reserve and delete the job in one command.
target/release/beanstalkctl reserve --timeout 5 --delete

# Delete a job.
target/release/beanstalkctl delete 1

# Peek at a job without consuming it.
target/release/beanstalkctl peek 1

# Inspect stats.
target/release/beanstalkctl stats
target/release/beanstalkctl stats tube default
target/release/beanstalkctl stats job 1

# Use a non-default server address or Unix socket.
target/release/beanstalkctl --addr 127.0.0.1:11300 tubes
target/release/beanstalkctl --unix /tmp/beanstalkd.sock tubes

# Send a raw protocol command.
target/release/beanstalkctl raw stats

# Start an interactive session.
target/release/beanstalkctl
```

Interactive mode supports line editing, history, and redis-cli-style Tab
completion for whole command and option tokens. See [cli/README.md](cli/README.md)
for details.

## Command-Line Options

The Rust daemon accepts the same primary options as the C daemon:

```text
-b DIR    write-ahead log directory
-f MS     fsync at most once every MS milliseconds; -f0 means always fsync
-F        never fsync
-l ADDR   listen address, or unix:/path for a Unix socket
-p PORT   listen port
-s BYTES  WAL segment size option accepted for compatibility
-u USER   become user and group after binding the listener
-z BYTES  maximum job size
-v        show version
-V        increase verbosity
-h        show help
```

Removed C flags `-c` and `-n` are accepted with compatibility warnings.

## Migrating From C beanstalkd

1. Stop the C daemon cleanly.
2. Keep the existing WAL directory containing `binlog.N` files.
3. Start the Rust daemon with the same `-b DIR`.
4. Keep client configuration unchanged.

Example:

```sh
target/release/beanstalkd -l 0.0.0.0 -p 11300 -b /var/lib/beanstalkd
```

On startup, the Rust daemon replays C `binlog.N` files first, then replays
`rust-beanstalkd.wal` if present. Jobs reserved by a client at the time of C
daemon shutdown are restored as ready, matching the C daemon's recovery
behavior.

## systemd Socket Activation

The Rust daemon supports inherited listening sockets via `LISTEN_PID` and
`LISTEN_FDS`. When a valid inherited TCP or Unix listening socket is present,
the daemon uses fd `3` and ignores manual `-l` / `-p` binding behavior, matching
the C implementation.

Existing C systemd unit/socket files can be reused if they execute the Rust
`beanstalkd` binary.

## Testing

Install Python test dependencies:

```sh
python -m pip install -r requirements-dev.txt
```

Run Rust checks:

```sh
RUSTC=$(rustup which rustc) cargo check
RUSTC=$(rustup which rustc) cargo test
RUSTC=$(rustup which rustc) cargo build --release --workspace
```

Run Python client compatibility tests:

```sh
python -m pytest -q
```

The Python tests start the Rust daemon as a subprocess and use the real
`greenstalk` client library. They cover normal client operations, tube behavior,
delays, bury/kick, pause, drain mode, Rust WAL recovery, C binlog migration, and
systemd-style fd inheritance.

## Repository Layout

```text
src/                    Rust implementation
cli/                    Standalone beanstalkctl command-line client
tests/                  Python compatibility tests
COMPATIBILITY_AUDIT.md  Compatibility coverage and known differences
requirements-dev.txt    Python test dependencies
```

## License

This Rust rewrite is intended as a compatible beanstalkd implementation.
