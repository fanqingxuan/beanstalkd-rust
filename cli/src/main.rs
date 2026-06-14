mod args;
mod client;
mod commands;
mod error;
mod protocol;
mod repl;
mod usage;

use crate::args::parse_global_options;
use crate::client::connect;
use crate::commands::dispatch;
use crate::error::Result;
use crate::repl::run_repl;
use crate::usage::usage;
use std::env;
use std::process;

fn main() {
    if let Err(err) = run() {
        eprintln!("beanstalkctl: {err}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("-h")
        || args.first().map(String::as_str) == Some("--help")
    {
        usage();
        return Ok(());
    }

    let cfg = parse_global_options(&mut args)?;
    if args.is_empty() {
        let mut conn = connect(&cfg)?;
        return run_repl(&mut conn);
    }
    let command = args.remove(0);
    let mut conn = connect(&cfg)?;
    if matches!(command.as_str(), "repl" | "interactive") {
        return run_repl(&mut conn);
    }
    dispatch(&mut conn, &command, args)
}
