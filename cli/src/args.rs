use crate::error::{CliError, Result};
use crate::usage::usage;
use std::process;

const DEFAULT_ADDR: &str = "127.0.0.1";
const DEFAULT_PORT: &str = "11300";

pub(crate) struct Config {
    pub(crate) addr: String,
    pub(crate) port: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: DEFAULT_ADDR.to_string(),
            port: DEFAULT_PORT.to_string(),
        }
    }
}

pub(crate) fn parse_global_options(args: &mut Vec<String>) -> Result<Config> {
    let mut cfg = Config::default();
    let i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-a" | "--addr" => {
                let value = take_value(args, i)?;
                if let Some(path) = value.strip_prefix("unix:") {
                    cfg.addr = format!("unix:{path}");
                } else if let Some((host, port)) = value.rsplit_once(':') {
                    cfg.addr = host.to_string();
                    cfg.port = port.to_string();
                } else {
                    cfg.addr = value;
                }
            }
            "-H" | "--host" => cfg.addr = take_value(args, i)?,
            "-p" | "--port" => cfg.port = take_value(args, i)?,
            "--unix" => cfg.addr = format!("unix:{}", take_value(args, i)?),
            "--help" | "-h" => {
                usage();
                process::exit(0);
            }
            option if option.starts_with('-') => {
                return Err(CliError::new(format!("unknown global option: {option}")));
            }
            _ => break,
        }
    }
    Ok(cfg)
}

pub(crate) fn take_value(args: &mut Vec<String>, index: usize) -> Result<String> {
    if index + 1 >= args.len() {
        return Err(CliError::new(format!("missing value for {}", args[index])));
    }
    args.remove(index);
    Ok(args.remove(index))
}
