use crate::constants::{
    DEFAULT_ADDR, DEFAULT_FSYNC_MS, DEFAULT_PORT, FILE_SIZE_DEFAULT, JOB_DATA_SIZE_LIMIT_DEFAULT,
    JOB_DATA_SIZE_LIMIT_MAX, VERSION,
};
use nix::unistd::{setgid, setuid, Gid, Uid, User};
use std::env;
use std::path::PathBuf;
use std::process;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) addr: String,
    pub(crate) port: String,
    pub(crate) user: Option<String>,
    pub(crate) max_job_size: usize,
    pub(crate) wal_dir: Option<PathBuf>,
    pub(crate) wal_file_size: usize,
    pub(crate) fsync: bool,
    pub(crate) fsync_ms: u64,
    pub(crate) verbose: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: DEFAULT_ADDR.to_string(),
            port: DEFAULT_PORT.to_string(),
            user: None,
            max_job_size: JOB_DATA_SIZE_LIMIT_DEFAULT,
            wal_dir: None,
            wal_file_size: FILE_SIZE_DEFAULT,
            fsync: true,
            fsync_ms: DEFAULT_FSYNC_MS,
            verbose: 0,
        }
    }
}

pub(crate) fn parse_args() -> Result<Config, i32> {
    let mut cfg = Config::default();
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !arg.starts_with('-') || arg == "-" {
            eprintln!("beanstalkd: unknown argument: {arg}");
            usage(5);
            return Err(5);
        }
        let mut chars = arg[1..].chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                'p' | 'l' | 'z' | 's' | 'f' | 'u' | 'b' => {
                    let rest: String = chars.collect();
                    let value = if rest.is_empty() {
                        i += 1;
                        args.get(i).cloned()
                    } else {
                        Some(rest)
                    };
                    let Some(value) = value else {
                        eprintln!("beanstalkd: flag requires an argument: -{ch}");
                        usage(5);
                        return Err(5);
                    };
                    match ch {
                        'p' => cfg.port = value,
                        'l' => cfg.addr = value,
                        'z' => {
                            cfg.max_job_size = value.parse().unwrap_or_else(|_| {
                                eprintln!("beanstalkd: invalid size: {value}");
                                process::exit(5);
                            });
                            if cfg.max_job_size > JOB_DATA_SIZE_LIMIT_MAX {
                                eprintln!(
                                    "beanstalkd: maximum job size was set to {JOB_DATA_SIZE_LIMIT_MAX}"
                                );
                                cfg.max_job_size = JOB_DATA_SIZE_LIMIT_MAX;
                            }
                        }
                        's' => {
                            cfg.wal_file_size = value.parse().unwrap_or_else(|_| {
                                eprintln!("beanstalkd: invalid size: {value}");
                                process::exit(5);
                            });
                        }
                        'f' => {
                            cfg.fsync_ms = value.parse().unwrap_or_else(|_| {
                                eprintln!("beanstalkd: invalid size: {value}");
                                process::exit(5);
                            });
                            cfg.fsync = true;
                        }
                        'u' => cfg.user = Some(value),
                        'b' => cfg.wal_dir = Some(PathBuf::from(value)),
                        _ => unreachable!(),
                    }
                    break;
                }
                'F' => cfg.fsync = false,
                'V' => cfg.verbose = cfg.verbose.saturating_add(1),
                'v' => {
                    println!("beanstalkd {VERSION}");
                    return Err(0);
                }
                'h' => {
                    usage(0);
                    return Err(0);
                }
                'c' | 'n' => {
                    eprintln!("beanstalkd: -{ch} flag was removed. binlog is always compacted.")
                }
                _ => {
                    eprintln!("beanstalkd: unknown flag: -{ch}");
                    usage(5);
                    return Err(5);
                }
            }
        }
        i += 1;
    }
    Ok(cfg)
}

fn usage(code: i32) {
    eprintln!(
        "Use: beanstalkd [OPTIONS]\n\n\
Options:\n\
 -b DIR   write-ahead log directory\n\
 -f MS    fsync at most once every MS milliseconds (default is {DEFAULT_FSYNC_MS}ms);\n\
          use -f0 for \"always fsync\"\n\
 -F       never fsync\n\
 -l ADDR  listen on address (default is 0.0.0.0)\n\
 -p PORT  listen on port (default is {DEFAULT_PORT})\n\
 -u USER  become user and group\n\
 -z BYTES set the maximum job size in bytes (default is {JOB_DATA_SIZE_LIMIT_DEFAULT});\n\
          max allowed is {JOB_DATA_SIZE_LIMIT_MAX} bytes\n\
 -s BYTES set the size of each write-ahead log file (default is {FILE_SIZE_DEFAULT});\n\
          accepted for compatibility by the Rust WAL\n\
 -v       show version information\n\
 -V       increase verbosity\n\
 -h       show this help"
    );
    process::exit(code);
}

pub(crate) fn drop_privileges(user: &str) {
    let found = User::from_name(user).unwrap_or_else(|err| {
        eprintln!("beanstalkd: getpwnam(\"{user}\"): {err}");
        process::exit(32);
    });
    let Some(user) = found else {
        eprintln!("beanstalkd: getpwnam(\"{user}\"): no such user");
        process::exit(33);
    };
    if let Err(err) = setgid(Gid::from_raw(user.gid.as_raw())) {
        eprintln!("beanstalkd: setgid({} \"{}\"): {err}", user.gid, user.name);
        process::exit(34);
    }
    if let Err(err) = setuid(Uid::from_raw(user.uid.as_raw())) {
        eprintln!("beanstalkd: setuid({} \"{}\"): {err}", user.uid, user.name);
        process::exit(34);
    }
}
