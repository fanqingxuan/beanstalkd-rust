# Compatibility Audit

This Rust rewrite was checked against the original C beanstalkd behavior and
the public beanstalk protocol contract.

## Covered

- Command-line flags accepted by the C daemon: `-b`, `-f`, `-F`, `-l`, `-p`,
  `-s`, `-u`, `-z`, `-v`, `-V`, `-h`, plus removed `-c` and `-n` warnings.
- TCP and `unix:` socket listeners.
- systemd socket activation through `LISTEN_PID` / `LISTEN_FDS`; the first
  inherited TCP or UNIX listening socket is used just like the C daemon.
- Bind-before-`-u` privilege-drop ordering, matching the C daemon startup
  order.
- `SIGUSR1` drain mode: new `put` commands return `DRAINING`.
- Producer and worker commands: `put`, `use`, `reserve`,
  `reserve-with-timeout`, `reserve-job`, `delete`, `release`, `bury`, `touch`.
- Tube commands: `watch`, `ignore`, `pause-tube`, `list-tubes`,
  `list-tube-used`, `list-tubes-watched`.
- Inspection and stats commands: `peek`, `peek-ready`, `peek-delayed`,
  `peek-buried`, `stats`, `stats-job`, `stats-tube`.
- Priority ordering across watched tubes, using the same priority/id ordering
  as the C implementation.
- Delay promotion, TTR timeout, `DEADLINE_SOON`, pause/unpause behavior, and
  release-on-connection-close.
- Empty tube cleanup when no current user, watcher, or job references remain.
- Runtime `stats` fields for process id, rusage, uname-derived hostname,
  OS-version, and platform.
- Python client compatibility using `greenstalk` without protocol shims.
- C binary binlog migration: Rust reads `binlog.N` files written by the C
  daemon, including current v7 records and legacy v5 records.

## Known Differences

- After migrating C `binlog.N` files, the Rust daemon writes subsequent updates
  to `DIR/rust-beanstalkd.wal` JSONL. It reads C binlogs, but does not emit new
  C-format binlog segments.
- Low-level allocation-failure behavior from the C unit tests is not
  reproducible in safe Rust tests; client-visible normal-path responses are
  covered.
- The Rust implementation does not try to match internal C data structures or
  file compaction details, only the externally visible protocol and operational
  behavior.

## Verification Commands

```sh
RUSTC=$(rustup which rustc) cargo check
RUSTC=$(rustup which rustc) cargo test
RUSTC=$(rustup which rustc) cargo build --release --workspace
python -m pip install -r requirements-dev.txt
python -m pytest -q
```
