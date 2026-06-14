use crate::client::Connection;
use crate::error::{CliError, Result};
use std::io::{self, Read, Write};

pub(crate) fn send_line(conn: &mut Connection, line: &str) -> Result<()> {
    send_bytes(conn, line.as_bytes())
}

pub(crate) fn send_bytes(conn: &mut Connection, bytes: &[u8]) -> Result<()> {
    conn.write_all(bytes)?;
    conn.flush()?;
    Ok(())
}

pub(crate) fn read_response(conn: &mut Connection) -> Result<Vec<u8>> {
    let line = read_line(conn)?;
    let mut out = line.clone();
    let fields: Vec<&[u8]> = line
        .strip_suffix(b"\r\n")
        .unwrap_or(&line)
        .split(|b| *b == b' ')
        .collect();
    let body_len = match fields.as_slice() {
        [b"RESERVED", _, bytes] | [b"FOUND", _, bytes] | [b"OK", bytes] => {
            parse_usize_bytes(bytes)?
        }
        _ => return Ok(out),
    };
    let mut body = vec![0_u8; body_len + 2];
    conn.read_exact(&mut body)?;
    out.extend(body);
    Ok(out)
}

pub(crate) fn print_response(response: Vec<u8>) {
    if print_job_response(&response).is_ok_and(|printed| printed) {
        return;
    }
    print_raw_response(&response);
}

pub(crate) fn print_raw_response(response: &[u8]) {
    io::stdout().write_all(&response).ok();
}

fn print_job_response(response: &[u8]) -> io::Result<bool> {
    let Some((line, body)) = split_response(response) else {
        return Ok(false);
    };
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [status @ ("FOUND" | "RESERVED"), id, bytes] = fields.as_slice() else {
        return Ok(false);
    };

    let mut stdout = io::stdout();
    writeln!(stdout, "status: {}", status.to_ascii_lowercase())?;
    writeln!(stdout, "job id: {id}")?;
    writeln!(stdout, "bytes: {bytes}")?;
    writeln!(stdout, "body:")?;
    stdout.write_all(body)?;
    if !body.ends_with(b"\n") {
        writeln!(stdout)?;
    }
    Ok(true)
}

fn split_response(response: &[u8]) -> Option<(&str, &[u8])> {
    let header_end = response.windows(2).position(|window| window == b"\r\n")?;
    let line = std::str::from_utf8(&response[..header_end]).ok()?;
    let body_start = header_end + 2;
    let body_end = response
        .get(body_start..)?
        .strip_suffix(b"\r\n")
        .map(|body| body_start + body.len())
        .unwrap_or(response.len());
    Some((line, &response[body_start..body_end]))
}

pub(crate) fn reserved_id(response: &[u8]) -> Option<String> {
    let line = response.split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?.trim();
    let mut parts = line.split_whitespace();
    if parts.next()? != "RESERVED" {
        return None;
    }
    Some(parts.next()?.to_string())
}

fn read_line(conn: &mut Connection) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let n = conn.read(&mut byte)?;
        if n == 0 {
            return Err(CliError::new("unexpected EOF"));
        }
        out.push(byte[0]);
        if out.ends_with(b"\r\n") {
            return Ok(out);
        }
    }
}

fn parse_usize_bytes(bytes: &[u8]) -> Result<usize> {
    let s = std::str::from_utf8(bytes).map_err(|_| CliError::new("invalid response size"))?;
    s.parse::<usize>()
        .map_err(|_| CliError::new("invalid response size"))
}
