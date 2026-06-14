pub(crate) fn usage() {
    println!(
        "beanstalkctl - command-line beanstalkd client\n\n\
Usage:\n  \
beanstalkctl [OPTIONS] [COMMAND] [ARGS]\n\n\
With no COMMAND, beanstalkctl starts interactive mode.\n\n\
Options:\n  \
-a, --addr ADDR       TCP host:port, host, or unix:/path (default 127.0.0.1:11300)\n  \
-H, --host HOST       TCP host (default 127.0.0.1)\n  \
-p, --port PORT       TCP port (default 11300)\n      \
--unix PATH       Connect to Unix socket\n\n\
Commands:\n  \
put [OPTIONS] BODY...             Insert a job\n  \
reserve [--timeout N] [--watch T] [--delete]\n  \
delete ID                         Delete a job\n  \
release ID [PRI] [DELAY]          Release a reserved job\n  \
bury ID [PRI]                     Bury a reserved job\n  \
touch ID                          Touch a reserved job\n  \
peek ID | peek-ready | peek-delayed | peek-buried\n  \
kick BOUND | kick-job ID\n  \
stats [job ID|tube NAME]\n  \
tubes | using | watching\n  \
pause-tube TUBE DELAY\n  \
raw COMMAND...                    Send a raw protocol command\n\n\
repl | interactive                Start interactive mode\n\n\
put options:\n  \
--tube TUBE     Use tube before put\n  \
--pri N         Priority (default 65536)\n  \
--delay N       Delay seconds (default 0)\n  \
--ttr N         TTR seconds (default 60)\n  \
--file PATH     Read body from file\n  \
--stdin         Read body from stdin"
    );
}
