use crate::constants::{LINE_BUF_SIZE, MAX_TUBE_NAME_LEN, NAME_CHARS};
use crate::model::{line, Command, Response};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub(crate) async fn read_command<S: AsyncReadExt + Unpin>(
    stream: &mut S,
) -> Result<Option<Command>, Response> {
    let mut line_buf = Vec::with_capacity(LINE_BUF_SIZE);
    loop {
        let mut b = [0_u8; 1];
        match stream.read(&mut b).await {
            Ok(0) => return Ok(None),
            Ok(_) => {
                line_buf.push(b[0]);
                if line_buf.len() > LINE_BUF_SIZE {
                    return Err(line("BAD_FORMAT\r\n"));
                }
                if line_buf.ends_with(b"\r\n") {
                    break;
                }
            }
            Err(_) => return Ok(None),
        }
    }
    if line_buf.contains(&0) {
        return Err(line("BAD_FORMAT\r\n"));
    }
    let request_line = String::from_utf8_lossy(&line_buf[..line_buf.len() - 2]).to_string();
    let mut parts = request_line.split_whitespace();
    let Some(name) = parts.next() else {
        return Err(line("BAD_FORMAT\r\n"));
    };
    let cmd = match name {
        "put" => {
            let pri = parse_u32(parts.next())?;
            let delay = parse_duration(parts.next())?;
            let ttr = parse_duration(parts.next())?;
            let bytes = parse_usize(parts.next())?;
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            let mut body = vec![0_u8; bytes + 2];
            if stream.read_exact(&mut body).await.is_err() {
                return Ok(None);
            }
            if !body.ends_with(b"\r\n") {
                return Err(line("EXPECTED_CRLF\r\n"));
            }
            body.truncate(bytes);
            Command::Put {
                pri,
                delay,
                ttr,
                body,
            }
        }
        "use" => Command::Use(parse_name(parts.next(), parts.next())?),
        "watch" => Command::Watch(parse_name(parts.next(), parts.next())?),
        "ignore" => Command::Ignore(parse_name(parts.next(), parts.next())?),
        "reserve" => {
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            Command::Reserve(None)
        }
        "reserve-with-timeout" => {
            let timeout = parse_duration(parts.next())?;
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            Command::Reserve(Some(timeout))
        }
        "reserve-job" => Command::ReserveJob(parse_u64_exact(parts.next(), parts.next())?),
        "delete" => Command::Delete(parse_u64_exact(parts.next(), parts.next())?),
        "release" => {
            let id = parse_u64(parts.next())?;
            let pri = parse_u32(parts.next())?;
            let delay = parse_duration(parts.next())?;
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            Command::Release { id, pri, delay }
        }
        "bury" => {
            let id = parse_u64(parts.next())?;
            let pri = parse_u32(parts.next())?;
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            Command::Bury { id, pri }
        }
        "touch" => Command::Touch(parse_u64_exact(parts.next(), parts.next())?),
        "peek" => Command::Peek(parse_u64_exact(parts.next(), parts.next())?),
        "peek-ready" => exact_no_args(parts).map(|_| Command::PeekReady)?,
        "peek-delayed" => exact_no_args(parts).map(|_| Command::PeekDelayed)?,
        "peek-buried" => exact_no_args(parts).map(|_| Command::PeekBuried)?,
        "kick" => Command::Kick(parse_u32_exact(parts.next(), parts.next())?),
        "kick-job" => Command::KickJob(parse_u64_exact(parts.next(), parts.next())?),
        "stats" => exact_no_args(parts).map(|_| Command::Stats)?,
        "stats-job" => Command::StatsJob(parse_u64_exact(parts.next(), parts.next())?),
        "stats-tube" => Command::StatsTube(parse_name(parts.next(), parts.next())?),
        "list-tubes" => exact_no_args(parts).map(|_| Command::ListTubes)?,
        "list-tube-used" => exact_no_args(parts).map(|_| Command::ListTubeUsed)?,
        "list-tubes-watched" => exact_no_args(parts).map(|_| Command::ListTubesWatched)?,
        "pause-tube" => {
            let tube = parse_name(parts.next(), None)?;
            let delay = parse_duration(parts.next())?;
            if parts.next().is_some() {
                return Err(line("BAD_FORMAT\r\n"));
            }
            Command::PauseTube { tube, delay }
        }
        "quit" => exact_no_args(parts).map(|_| Command::Quit)?,
        _ => return Err(line("UNKNOWN_COMMAND\r\n")),
    };
    Ok(Some(cmd))
}

fn exact_no_args<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<(), Response> {
    if parts.next().is_some() {
        Err(line("BAD_FORMAT\r\n"))
    } else {
        Ok(())
    }
}

fn parse_name(value: Option<&str>, extra: Option<&str>) -> Result<String, Response> {
    if extra.is_some() {
        return Err(line("BAD_FORMAT\r\n"));
    }
    let Some(value) = value else {
        return Err(line("BAD_FORMAT\r\n"));
    };
    if valid_tube_name(value) {
        Ok(value.to_string())
    } else {
        Err(line("BAD_FORMAT\r\n"))
    }
}

fn parse_u64_exact(value: Option<&str>, extra: Option<&str>) -> Result<u64, Response> {
    if extra.is_some() {
        return Err(line("BAD_FORMAT\r\n"));
    }
    parse_u64(value)
}

fn parse_u32_exact(value: Option<&str>, extra: Option<&str>) -> Result<u32, Response> {
    if extra.is_some() {
        return Err(line("BAD_FORMAT\r\n"));
    }
    parse_u32(value)
}

fn parse_u64(value: Option<&str>) -> Result<u64, Response> {
    value
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| line("BAD_FORMAT\r\n"))
}

fn parse_u32(value: Option<&str>) -> Result<u32, Response> {
    value
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(|| line("BAD_FORMAT\r\n"))
}

fn parse_usize(value: Option<&str>) -> Result<usize, Response> {
    value
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| line("BAD_FORMAT\r\n"))
}

fn parse_duration(value: Option<&str>) -> Result<Duration, Response> {
    parse_u32(value).map(|s| Duration::from_secs(s as u64))
}

pub(crate) fn valid_tube_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TUBE_NAME_LEN
        && !name.starts_with('-')
        && name.chars().all(|c| NAME_CHARS.contains(c))
}

pub(crate) fn yaml_list(names: &[String]) -> String {
    let mut out = String::from("---\n");
    for name in names {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("\r\n");
    out
}
