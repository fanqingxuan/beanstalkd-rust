use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum JobState {
    Ready,
    Reserved,
    Buried,
    Delayed,
}

impl JobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            JobState::Ready => "ready",
            JobState::Reserved => "reserved",
            JobState::Buried => "buried",
            JobState::Delayed => "delayed",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Job {
    pub(crate) id: u64,
    pub(crate) pri: u32,
    pub(crate) delay: Duration,
    pub(crate) ttr: Duration,
    pub(crate) body: Vec<u8>,
    pub(crate) tube: String,
    pub(crate) state: JobState,
    pub(crate) created_at: Instant,
    pub(crate) created_unix: u64,
    pub(crate) deadline_at: Option<Instant>,
    pub(crate) reserve_ct: u32,
    pub(crate) timeout_ct: u32,
    pub(crate) release_ct: u32,
    pub(crate) bury_ct: u32,
    pub(crate) kick_ct: u32,
    pub(crate) reserver: Option<u64>,
}

#[derive(Default, Clone)]
pub(crate) struct Counters {
    pub(crate) urgent_ct: u64,
    pub(crate) waiting_ct: u64,
    pub(crate) buried_ct: u64,
    pub(crate) reserved_ct: u64,
    pub(crate) pause_ct: u64,
    pub(crate) total_delete_ct: u64,
    pub(crate) total_jobs_ct: u64,
}

pub(crate) struct Tube {
    pub(crate) name: String,
    pub(crate) ready: BTreeSet<ReadyKey>,
    pub(crate) delay: BTreeSet<DelayKey>,
    pub(crate) buried: VecDeque<u64>,
    pub(crate) stats: Counters,
    pub(crate) using_ct: u32,
    pub(crate) watching_ct: u32,
    pub(crate) pause: Duration,
    pub(crate) unpause_at: Option<Instant>,
}

impl Tube {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            ready: BTreeSet::new(),
            delay: BTreeSet::new(),
            buried: VecDeque::new(),
            stats: Counters::default(),
            using_ct: 0,
            watching_ct: 0,
            pause: Duration::ZERO,
            unpause_at: None,
        }
    }

    pub(crate) fn paused(&mut self, now: Instant) -> bool {
        if let Some(unpause_at) = self.unpause_at {
            if unpause_at <= now {
                self.pause = Duration::ZERO;
                self.unpause_at = None;
                return false;
            }
            return true;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReadyKey {
    pub(crate) pri: u32,
    pub(crate) id: u64,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.pri.cmp(&other.pri).then(self.id.cmp(&other.id))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DelayKey {
    pub(crate) when: Instant,
    pub(crate) id: u64,
}

impl Eq for DelayKey {}

impl PartialEq for DelayKey {
    fn eq(&self, other: &Self) -> bool {
        self.when == other.when && self.id == other.id
    }
}

impl Ord for DelayKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.when.cmp(&other.when).then(self.id.cmp(&other.id))
    }
}

impl PartialOrd for DelayKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
pub(crate) struct ConnInfo {
    pub(crate) using: String,
    pub(crate) watching: HashSet<String>,
    pub(crate) reserved: HashSet<u64>,
    pub(crate) producer: bool,
    pub(crate) worker: bool,
    pub(crate) waiting: bool,
}

pub(crate) struct Waiter {
    pub(crate) conn_id: u64,
    pub(crate) watches: HashSet<String>,
    pub(crate) timeout_at: Option<Instant>,
    pub(crate) tx: oneshot::Sender<Response>,
}

#[derive(Default)]
pub(crate) struct OpCounters {
    pub(crate) put: u64,
    pub(crate) peek: u64,
    pub(crate) peek_ready: u64,
    pub(crate) peek_delayed: u64,
    pub(crate) peek_buried: u64,
    pub(crate) reserve: u64,
    pub(crate) reserve_with_timeout: u64,
    pub(crate) reserve_job: u64,
    pub(crate) delete: u64,
    pub(crate) release: u64,
    pub(crate) bury: u64,
    pub(crate) kick: u64,
    pub(crate) kick_job: u64,
    pub(crate) touch: u64,
    pub(crate) stats: u64,
    pub(crate) stats_job: u64,
    pub(crate) stats_tube: u64,
    pub(crate) use_cmd: u64,
    pub(crate) watch: u64,
    pub(crate) ignore: u64,
    pub(crate) list_tubes: u64,
    pub(crate) list_tube_used: u64,
    pub(crate) list_tubes_watched: u64,
    pub(crate) pause_tube: u64,
}

#[derive(Clone)]
pub(crate) struct Response(pub(crate) Vec<u8>);

pub(crate) fn line(s: impl AsRef<str>) -> Response {
    Response(s.as_ref().as_bytes().to_vec())
}

pub(crate) fn chunk(status: &str, mut body: Vec<u8>) -> Response {
    let bytes = body.len().saturating_sub(2);
    let mut out = format!("{status} {bytes}\r\n").into_bytes();
    out.append(&mut body);
    Response(out)
}

pub(crate) fn job_response(status: &str, job: &Job) -> Response {
    let mut out = format!("{status} {} {}\r\n", job.id, job.body.len()).into_bytes();
    out.extend_from_slice(&job.body);
    out.extend_from_slice(b"\r\n");
    Response(out)
}

#[derive(Debug)]
pub(crate) enum Command {
    Put {
        pri: u32,
        delay: Duration,
        ttr: Duration,
        body: Vec<u8>,
    },
    Use(String),
    Watch(String),
    Ignore(String),
    Reserve(Option<Duration>),
    ReserveJob(u64),
    Delete(u64),
    Release {
        id: u64,
        pri: u32,
        delay: Duration,
    },
    Bury {
        id: u64,
        pri: u32,
    },
    Touch(u64),
    Peek(u64),
    PeekReady,
    PeekDelayed,
    PeekBuried,
    Kick(u32),
    KickJob(u64),
    Stats,
    StatsJob(u64),
    StatsTube(String),
    ListTubes,
    ListTubeUsed,
    ListTubesWatched,
    PauseTube {
        tube: String,
        delay: Duration,
    },
    Quit,
}

pub(crate) enum ActorMsg {
    Connect {
        tx: oneshot::Sender<u64>,
    },
    Disconnect {
        conn_id: u64,
    },
    Command {
        conn_id: u64,
        cmd: Command,
        tx: oneshot::Sender<CommandResult>,
    },
    Drain,
    Tick,
}

pub(crate) enum CommandResult {
    Immediate(Response),
    Pending(oneshot::Receiver<Response>),
    Close,
}
