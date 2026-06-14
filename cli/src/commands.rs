use crate::args::take_value;
use crate::client::Connection;
use crate::error::{CliError, Result};
use crate::protocol::{
    print_raw_response, print_response, read_response, reserved_id, send_bytes, send_line,
};
use crate::usage::usage;
use std::fs;
use std::io::{self, Read};

pub(crate) fn dispatch(conn: &mut Connection, command: &str, args: Vec<String>) -> Result<()> {
    match command {
        "put" => cmd_put(conn, args),
        "reserve" => cmd_reserve(conn, args),
        "delete" => cmd_simple(conn, "delete", args, 1),
        "release" => cmd_release(conn, args),
        "bury" => cmd_bury(conn, args),
        "touch" => cmd_simple(conn, "touch", args, 1),
        "peek" => cmd_simple(conn, "peek", args, 1),
        "peek-ready" => cmd_noarg(conn, "peek-ready", args),
        "peek-delayed" => cmd_noarg(conn, "peek-delayed", args),
        "peek-buried" => cmd_noarg(conn, "peek-buried", args),
        "kick" => cmd_simple(conn, "kick", args, 1),
        "kick-job" => cmd_simple(conn, "kick-job", args, 1),
        "stats" => cmd_stats(conn, args),
        "tubes" | "list-tubes" => cmd_noarg(conn, "list-tubes", args),
        "using" | "list-tube-used" => cmd_noarg(conn, "list-tube-used", args),
        "watching" | "list-tubes-watched" => cmd_noarg(conn, "list-tubes-watched", args),
        "pause-tube" => cmd_simple(conn, "pause-tube", args, 2),
        "raw" => cmd_raw(conn, args),
        "help" => {
            usage();
            Ok(())
        }
        _ => Err(CliError::new(format!("unknown command: {command}"))),
    }
}

fn cmd_put(conn: &mut Connection, mut args: Vec<String>) -> Result<()> {
    let mut tube = None;
    let mut pri = "65536".to_string();
    let mut delay = "0".to_string();
    let mut ttr = "60".to_string();
    let mut file = None;
    let mut stdin_body = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tube" | "-t" => tube = Some(take_value(&mut args, i)?),
            "--pri" | "--priority" => pri = take_value(&mut args, i)?,
            "--delay" => delay = take_value(&mut args, i)?,
            "--ttr" => ttr = take_value(&mut args, i)?,
            "--file" | "-f" => file = Some(take_value(&mut args, i)?),
            "--stdin" => {
                args.remove(i);
                stdin_body = true;
            }
            _ => i += 1,
        }
    }

    let body = if let Some(path) = file {
        fs::read(path)?
    } else if stdin_body || args.first().map(|s| s.as_str()) == Some("-") {
        let mut body = Vec::new();
        io::stdin().read_to_end(&mut body)?;
        body
    } else {
        args.join(" ").into_bytes()
    };

    if let Some(tube) = tube {
        send_line(conn, &format!("use {tube}\r\n"))?;
        let _ = read_response(conn)?;
    }

    send_bytes(
        conn,
        &[
            format!("put {pri} {delay} {ttr} {}\r\n", body.len()).into_bytes(),
            body,
            b"\r\n".to_vec(),
        ]
        .concat(),
    )?;
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_reserve(conn: &mut Connection, mut args: Vec<String>) -> Result<()> {
    let mut timeout = None;
    let mut watch = Vec::new();
    let mut delete = false;
    let i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--timeout" => timeout = Some(take_value(&mut args, i)?),
            "--watch" | "-w" => watch.push(take_value(&mut args, i)?),
            "--delete" => {
                args.remove(i);
                delete = true;
            }
            _ => {
                return Err(CliError::new(format!(
                    "unknown reserve option: {}",
                    args[i]
                )))
            }
        }
    }

    if !watch.is_empty() {
        for tube in &watch {
            send_line(conn, &format!("watch {tube}\r\n"))?;
            let _ = read_response(conn)?;
        }
        if !watch.iter().any(|tube| tube == "default") {
            send_line(conn, "ignore default\r\n")?;
            let _ = read_response(conn)?;
        }
    }

    match timeout {
        Some(timeout) => send_line(conn, &format!("reserve-with-timeout {timeout}\r\n"))?,
        None => send_line(conn, "reserve\r\n")?,
    }
    let response = read_response(conn)?;
    let reserved_id = reserved_id(&response);
    print_response(response);

    if delete {
        if let Some(id) = reserved_id {
            send_line(conn, &format!("delete {id}\r\n"))?;
            print_response(read_response(conn)?);
        }
    }
    Ok(())
}

fn cmd_release(conn: &mut Connection, args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err(CliError::new("release requires ID [PRI] [DELAY]"));
    }
    let id = &args[0];
    let pri = args.get(1).map(String::as_str).unwrap_or("65536");
    let delay = args.get(2).map(String::as_str).unwrap_or("0");
    send_line(conn, &format!("release {id} {pri} {delay}\r\n"))?;
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_bury(conn: &mut Connection, args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err(CliError::new("bury requires ID [PRI]"));
    }
    let id = &args[0];
    let pri = args.get(1).map(String::as_str).unwrap_or("65536");
    send_line(conn, &format!("bury {id} {pri}\r\n"))?;
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_stats(conn: &mut Connection, args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [] => send_line(conn, "stats\r\n")?,
        [kind, value] if kind == "job" => send_line(conn, &format!("stats-job {value}\r\n"))?,
        [kind, value] if kind == "tube" => send_line(conn, &format!("stats-tube {value}\r\n"))?,
        _ => return Err(CliError::new("usage: stats [job ID|tube NAME]")),
    }
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_simple(conn: &mut Connection, name: &str, args: Vec<String>, arity: usize) -> Result<()> {
    if args.len() != arity {
        return Err(CliError::new(format!(
            "{name} requires {arity} argument(s)"
        )));
    }
    send_line(conn, &format!("{name} {}\r\n", args.join(" ")))?;
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_noarg(conn: &mut Connection, name: &str, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        return Err(CliError::new(format!("{name} does not accept arguments")));
    }
    send_line(conn, &format!("{name}\r\n"))?;
    print_response(read_response(conn)?);
    Ok(())
}

fn cmd_raw(conn: &mut Connection, args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Err(CliError::new("raw requires a command line"));
    }
    let mut line = args.join(" ");
    if !line.ends_with("\r\n") {
        line.push_str("\r\n");
    }
    send_line(conn, &line)?;
    print_raw_response(&read_response(conn)?);
    Ok(())
}
