use crate::config::{drop_privileges, parse_args};
use crate::model::{ActorMsg, CommandResult};
use crate::protocol::read_command;
use crate::state::ServerState;
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::FromRawFd;
use std::process;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, oneshot};

enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

pub(crate) async fn run() {
    let cfg = parse_args().unwrap_or_else(|code| process::exit(code));
    if cfg.verbose > 0 {
        println!("pid {}", process::id());
    }
    let listener = bind_listener(&cfg).await;
    if let Some(user) = &cfg.user {
        drop_privileges(user);
    }
    let state = match ServerState::new(cfg.clone()) {
        Ok(state) => state,
        Err(err) => {
            eprintln!("beanstalkd: failed to initialize: {err}");
            process::exit(1);
        }
    };
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(actor(state, rx));
    let ticker = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if ticker.send(ActorMsg::Tick).await.is_err() {
                break;
            }
        }
    });
    spawn_signal_drainer(tx.clone());
    accept_loop(listener, tx).await;
}

#[cfg(unix)]
fn spawn_signal_drainer(drainer: mpsc::Sender<ActorMsg>) {
    tokio::spawn(async move {
        if let Ok(mut sigusr1) = signal(SignalKind::user_defined1()) {
            while sigusr1.recv().await.is_some() {
                if drainer.send(ActorMsg::Drain).await.is_err() {
                    break;
                }
            }
        }
    });
}

#[cfg(not(unix))]
fn spawn_signal_drainer(_drainer: mpsc::Sender<ActorMsg>) {}

async fn bind_listener(cfg: &crate::config::Config) -> Listener {
    if let Some(listener) = inherited_systemd_listener() {
        return listener;
    }
    if cfg.addr.starts_with("unix:") {
        #[cfg(unix)]
        {
            return bind_unix_listener(cfg);
        }
        #[cfg(not(unix))]
        {
            eprintln!("beanstalkd: Unix sockets are not supported on this platform");
            process::exit(111);
        }
    }
    let bind = format!("{}:{}", cfg.addr, cfg.port);
    let listener = TcpListener::bind(&bind).await.unwrap_or_else(|err| {
        eprintln!("beanstalkd: bind {bind}: {err}");
        process::exit(111);
    });
    Listener::Tcp(listener)
}

#[cfg(unix)]
fn bind_unix_listener(cfg: &crate::config::Config) -> Listener {
    let path = cfg.addr.trim_start_matches("unix:");
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path).unwrap_or_else(|err| {
        eprintln!("beanstalkd: bind {path}: {err}");
        process::exit(111);
    });
    Listener::Unix(listener)
}

#[cfg(unix)]
fn inherited_systemd_listener() -> Option<Listener> {
    let listen_pid = env::var("LISTEN_PID").ok()?.parse::<u32>().ok()?;
    if listen_pid != process::id() {
        return None;
    }
    let listen_fds = env::var("LISTEN_FDS").ok()?.parse::<i32>().ok()?;
    env::remove_var("LISTEN_PID");
    env::remove_var("LISTEN_FDS");
    env::remove_var("LISTEN_FDNAMES");
    if listen_fds <= 0 {
        return None;
    }
    if listen_fds > 1 {
        eprintln!("beanstalkd: inherited more than one listen socket; ignoring all but the first");
    }
    let fd = 3;
    match socket_family(fd) {
        Some(libc::AF_UNIX) => {
            set_nonblocking(fd).unwrap_or_else(|err| {
                eprintln!("beanstalkd: setting O_NONBLOCK on inherited fd {fd}: {err}");
                process::exit(111);
            });
            let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
            Some(Listener::Unix(
                UnixListener::from_std(std_listener).unwrap_or_else(|err| {
                    eprintln!("beanstalkd: inherited unix listener: {err}");
                    process::exit(111);
                }),
            ))
        }
        Some(libc::AF_INET) | Some(libc::AF_INET6) => {
            set_nonblocking(fd).unwrap_or_else(|err| {
                eprintln!("beanstalkd: setting O_NONBLOCK on inherited fd {fd}: {err}");
                process::exit(111);
            });
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            Some(Listener::Tcp(
                TcpListener::from_std(std_listener).unwrap_or_else(|err| {
                    eprintln!("beanstalkd: inherited tcp listener: {err}");
                    process::exit(111);
                }),
            ))
        }
        Some(_) => {
            eprintln!("beanstalkd: inherited fd is not a TCP or UNIX listening socket");
            process::exit(111);
        }
        None => None,
    }
}

#[cfg(not(unix))]
fn inherited_systemd_listener() -> Option<Listener> {
    None
}

#[cfg(unix)]
fn socket_family(fd: i32) -> Option<i32> {
    let mut storage = std::mem::MaybeUninit::<libc::sockaddr_storage>::uninit();
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockname(
            fd,
            storage.as_mut_ptr() as *mut libc::sockaddr,
            &mut len as *mut libc::socklen_t,
        )
    };
    if rc != 0 {
        return None;
    }
    let storage = unsafe { storage.assume_init() };
    Some(storage.ss_family as i32)
}

#[cfg(unix)]
fn set_nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

async fn accept_loop(listener: Listener, tx: mpsc::Sender<ActorMsg>) {
    match listener {
        #[cfg(unix)]
        Listener::Unix(listener) => loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        handle_stream(stream, tx).await;
                    });
                }
                Err(err) => eprintln!("beanstalkd: accept: {err}"),
            }
        },
        Listener::Tcp(listener) => loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        handle_stream(stream, tx).await;
                    });
                }
                Err(err) => eprintln!("beanstalkd: accept: {err}"),
            }
        },
    }
}

async fn actor(mut state: ServerState, mut rx: mpsc::Receiver<ActorMsg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            ActorMsg::Connect { tx } => {
                let _ = tx.send(state.connect());
            }
            ActorMsg::Disconnect { conn_id } => state.disconnect(conn_id),
            ActorMsg::Command { conn_id, cmd, tx } => {
                let result = state.command(conn_id, cmd);
                let _ = tx.send(result);
            }
            #[cfg(unix)]
            ActorMsg::Drain => state.enter_drain_mode(),
            ActorMsg::Tick => state.tick(),
        }
    }
}

trait AsyncBeanstalkStream: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static {}
impl AsyncBeanstalkStream for TcpStream {}
#[cfg(unix)]
impl AsyncBeanstalkStream for UnixStream {}

async fn handle_stream<S: AsyncBeanstalkStream>(mut stream: S, tx: mpsc::Sender<ActorMsg>) {
    let (ctx_tx, ctx_rx) = oneshot::channel();
    if tx.send(ActorMsg::Connect { tx: ctx_tx }).await.is_err() {
        return;
    }
    let Ok(conn_id) = ctx_rx.await else { return };
    loop {
        let cmd = match read_command(&mut stream).await {
            Ok(Some(cmd)) => cmd,
            Ok(None) => break,
            Err(resp) => {
                if stream.write_all(&resp.0).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let (res_tx, res_rx) = oneshot::channel();
        if tx
            .send(ActorMsg::Command {
                conn_id,
                cmd,
                tx: res_tx,
            })
            .await
            .is_err()
        {
            break;
        }
        match res_rx.await {
            Ok(CommandResult::Immediate(resp)) => {
                if stream.write_all(&resp.0).await.is_err() {
                    break;
                }
            }
            Ok(CommandResult::Pending(rx)) => match rx.await {
                Ok(resp) => {
                    if stream.write_all(&resp.0).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            Ok(CommandResult::Close) | Err(_) => break,
        }
    }
    let _ = tx.send(ActorMsg::Disconnect { conn_id }).await;
}
