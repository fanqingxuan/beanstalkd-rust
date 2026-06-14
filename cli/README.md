# beanstalkctl

`beanstalkctl` is a standalone command-line client for beanstalkd. It does not
depend on the Rust server implementation in `src/`; it talks to beanstalkd only
through the public TCP or Unix-socket protocol.

中文文档见 [README.zh-CN.md](README.zh-CN.md).

## Build

From the repository root:

```sh
cargo build --release -p beanstalkctl
```

The binary is:

```sh
target/release/beanstalkctl
```

## Connection Options

```text
-a, --addr ADDR    TCP host:port, host, or unix:/path
-H, --host HOST    TCP host, default 127.0.0.1
-p, --port PORT    TCP port, default 11300
    --unix PATH    Connect to a Unix socket
```

Examples:

```sh
beanstalkctl --addr 127.0.0.1:11300 tubes
beanstalkctl --host 127.0.0.1 --port 11300 stats
beanstalkctl --unix /tmp/beanstalkd.sock tubes
```

## Commands

Running `beanstalkctl` with no command starts interactive mode.

```text
put [OPTIONS] BODY...             Insert a job
reserve [--timeout N] [--watch T] [--delete]
delete ID                         Delete a job
release ID [PRI] [DELAY]          Release a reserved job
bury ID [PRI]                     Bury a reserved job
touch ID                          Touch a reserved job
peek ID                           Peek by job id
peek-ready                        Peek the next ready job
peek-delayed                      Peek the next delayed job
peek-buried                       Peek the next buried job
kick BOUND                        Kick buried or delayed jobs
kick-job ID                       Kick one buried or delayed job
stats [job ID|tube NAME]          Show server, job, or tube stats
tubes                             List tubes
using                             Show the currently used tube
watching                          Show watched tubes
pause-tube TUBE DELAY             Pause a tube
raw COMMAND...                    Send a raw protocol command
repl | interactive                Start interactive mode
```

## Put Jobs

Put a simple job:

```sh
beanstalkctl put "hello"
```

Put into a specific tube:

```sh
beanstalkctl put --tube emails --pri 100 --delay 0 --ttr 60 "send welcome email"
```

Read the body from a file:

```sh
beanstalkctl put --tube imports --file payload.json
```

Read the body from standard input:

```sh
printf 'payload' | beanstalkctl put --stdin
```

## Reserve Jobs

Reserve from the default watch list:

```sh
beanstalkctl reserve --timeout 5
```

Reserve from a specific tube:

```sh
beanstalkctl reserve --watch emails --timeout 5
```

Reserve and delete in one command:

```sh
beanstalkctl reserve --timeout 5 --delete
```

## Manage Jobs

```sh
beanstalkctl delete 1
beanstalkctl release 1 65536 0
beanstalkctl bury 1 65536
beanstalkctl touch 1
beanstalkctl kick 10
beanstalkctl kick-job 1
```

## Inspect Jobs And Tubes

```sh
beanstalkctl peek 1
beanstalkctl peek-ready
beanstalkctl peek-delayed
beanstalkctl peek-buried
beanstalkctl tubes
beanstalkctl using
beanstalkctl watching
```

## Stats

```sh
beanstalkctl stats
beanstalkctl stats job 1
beanstalkctl stats tube default
```

## Raw Protocol

Use `raw` when a new or uncommon command is not wrapped by the CLI yet:

```sh
beanstalkctl raw stats
beanstalkctl raw list-tubes
beanstalkctl raw pause-tube default 5
```

## Interactive Mode

Start an interactive session:

```sh
beanstalkctl repl
```

Or just run `beanstalkctl` with no command:

```sh
beanstalkctl
```

You will see a prompt:

```text
beanstalkctl>
```

Commands entered in interactive mode use the same syntax as normal
`beanstalkctl` commands:

```text
beanstalkctl> put --tube emails "hello"
INSERTED 1
beanstalkctl> reserve --watch emails --timeout 5 --delete
RESERVED 1 5
hello
DELETED
beanstalkctl> stats
OK ...
beanstalkctl> exit
```

`exit` and `quit` leave interactive mode. Use `help` to print the command
summary. The same connection is reused for the whole session, so connection
state such as watched tubes is preserved between commands.

## Exit Codes

- `0`: command completed and a beanstalkd response was printed.
- `1`: connection, argument, or protocol I/O error.

Beanstalkd application-level responses such as `NOT_FOUND`, `TIMED_OUT`, or
`DEADLINE_SOON` are printed as server responses; they do not currently change
the process exit code.
