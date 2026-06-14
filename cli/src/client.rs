use crate::args::Config;
use crate::error::Result;
use std::io::{self, Read, Write};
use std::net::TcpStream;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub(crate) enum Connection {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Connection::Tcp(stream) => stream.read(buf),
            #[cfg(unix)]
            Connection::Unix(stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Connection::Tcp(stream) => stream.write(buf),
            #[cfg(unix)]
            Connection::Unix(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Connection::Tcp(stream) => stream.flush(),
            #[cfg(unix)]
            Connection::Unix(stream) => stream.flush(),
        }
    }
}

pub(crate) fn connect(cfg: &Config) -> Result<Connection> {
    if let Some(path) = cfg.addr.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            return Ok(Connection::Unix(UnixStream::connect(path)?));
        }
        #[cfg(not(unix))]
        {
            use crate::error::CliError;
            return Err(CliError::new(
                "Unix sockets are not supported on this platform",
            ));
        }
    }
    Ok(Connection::Tcp(TcpStream::connect(format!(
        "{}:{}",
        cfg.addr, cfg.port
    ))?))
}
