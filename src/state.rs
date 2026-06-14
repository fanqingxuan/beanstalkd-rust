use crate::config::Config;
use crate::constants::{URGENT_THRESHOLD, VERSION};
use crate::model::*;
use crate::protocol::{valid_tube_name, yaml_list};
use crate::wal::{ReplayedJob, Wal, WalEvent};
use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::process;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

pub(crate) struct ServerState {
    cfg: Config,
    started_at: Instant,
    instance_id: String,
    jobs: HashMap<u64, Job>,
    tubes: HashMap<String, Tube>,
    conns: HashMap<u64, ConnInfo>,
    waiters: VecDeque<Waiter>,
    next_job_id: u64,
    next_conn_id: u64,
    total_connections: u64,
    global: Counters,
    op: OpCounters,
    job_timeouts: u64,
    draining: bool,
    wal: Option<Wal>,
}

impl ServerState {
    pub(crate) fn new(cfg: Config) -> std::io::Result<Self> {
        let mut rnd = [0_u8; 8];
        rand::thread_rng().fill_bytes(&mut rnd);
        let mut state = Self {
            cfg,
            started_at: Instant::now(),
            instance_id: rnd.iter().map(|b| format!("{b:02x}")).collect(),
            jobs: HashMap::new(),
            tubes: HashMap::new(),
            conns: HashMap::new(),
            waiters: VecDeque::new(),
            next_job_id: 1,
            next_conn_id: 1,
            total_connections: 0,
            global: Counters::default(),
            op: OpCounters::default(),
            job_timeouts: 0,
            draining: false,
            wal: None,
        };
        state.ensure_tube("default");
        if let Some(dir) = state.cfg.wal_dir.clone() {
            fs::create_dir_all(&dir)?;
            let c_jobs = Wal::replay_c_binlogs(&dir, state.cfg.max_job_size)?;
            state.apply_replayed_jobs(c_jobs);
            let mut wal = Wal::open(&dir, state.cfg.fsync, state.cfg.fsync_ms)?;
            let events = wal.replay()?;
            state.apply_replay(events);
            state.wal = Some(wal);
        }
        Ok(state)
    }

    fn apply_replayed_jobs(&mut self, jobs: Vec<ReplayedJob>) {
        let now = Instant::now();
        for replayed in jobs {
            self.ensure_tube(&replayed.tube);
            let state = match replayed.state.as_str() {
                "buried" => JobState::Buried,
                "delayed" => JobState::Delayed,
                _ => JobState::Ready,
            };
            let created_age = unix_now().saturating_sub(replayed.created_unix);
            let created_at = now
                .checked_sub(Duration::from_secs(created_age))
                .unwrap_or(now);
            let delay = Duration::from_secs(replayed.delay_secs);
            let job = Job {
                id: replayed.id,
                pri: replayed.pri,
                delay,
                ttr: Duration::from_secs(replayed.ttr_secs.max(1)),
                body: replayed.body,
                tube: replayed.tube.clone(),
                state,
                created_at,
                created_unix: replayed.created_unix,
                deadline_at: (state == JobState::Delayed).then(|| now + delay),
                reserve_ct: replayed.reserve_ct,
                timeout_ct: replayed.timeout_ct,
                release_ct: replayed.release_ct,
                bury_ct: replayed.bury_ct,
                kick_ct: replayed.kick_ct,
                reserver: None,
            };
            self.jobs.insert(replayed.id, job);
            match state {
                JobState::Ready => self.insert_ready(replayed.id),
                JobState::Delayed => self.insert_delayed(replayed.id),
                JobState::Buried => self.insert_buried(replayed.id),
                JobState::Reserved => {}
            }
            self.next_job_id = self.next_job_id.max(replayed.id + 1);
            self.global.total_jobs_ct += 1;
            if let Some(tube) = self.tubes.get_mut(&replayed.tube) {
                tube.stats.total_jobs_ct += 1;
            }
        }
    }

    fn apply_replay(&mut self, events: Vec<WalEvent>) {
        let now = Instant::now();
        for event in events {
            match event {
                WalEvent::Put {
                    id,
                    pri,
                    delay_secs,
                    ttr_secs,
                    tube,
                    body,
                    created_unix,
                } => {
                    let body = general_purpose::STANDARD.decode(body).unwrap_or_default();
                    let created_age = unix_now().saturating_sub(created_unix);
                    let created_at = now
                        .checked_sub(Duration::from_secs(created_age))
                        .unwrap_or(now);
                    let state = if delay_secs == 0 {
                        JobState::Ready
                    } else {
                        JobState::Delayed
                    };
                    self.ensure_tube(&tube);
                    let deadline_at =
                        (delay_secs > 0).then(|| now + Duration::from_secs(delay_secs));
                    let job = Job {
                        id,
                        pri,
                        delay: Duration::from_secs(delay_secs),
                        ttr: Duration::from_secs(ttr_secs.max(1)),
                        body,
                        tube: tube.clone(),
                        state,
                        created_at,
                        created_unix,
                        deadline_at,
                        reserve_ct: 0,
                        timeout_ct: 0,
                        release_ct: 0,
                        bury_ct: 0,
                        kick_ct: 0,
                        reserver: None,
                    };
                    self.jobs.insert(id, job);
                    if state == JobState::Ready {
                        self.insert_ready(id);
                    } else {
                        self.insert_delayed(id);
                    }
                    self.next_job_id = self.next_job_id.max(id + 1);
                    self.global.total_jobs_ct += 1;
                    if let Some(t) = self.tubes.get_mut(&tube) {
                        t.stats.total_jobs_ct += 1;
                    }
                }
                WalEvent::Delete { id } => {
                    self.remove_job(id);
                }
                WalEvent::State {
                    id,
                    state,
                    pri,
                    delay_secs,
                } => {
                    self.remove_from_current_queue(id);
                    if let Some(job) = self.jobs.get_mut(&id) {
                        if let Some(pri) = pri {
                            job.pri = pri;
                        }
                        if let Some(delay_secs) = delay_secs {
                            job.delay = Duration::from_secs(delay_secs);
                        }
                        job.reserver = None;
                        job.deadline_at = None;
                        job.state = match state.as_str() {
                            "ready" => JobState::Ready,
                            "delayed" => {
                                job.deadline_at = Some(Instant::now() + job.delay);
                                JobState::Delayed
                            }
                            "buried" => JobState::Buried,
                            _ => JobState::Ready,
                        };
                    }
                    match self.jobs.get(&id).map(|j| j.state) {
                        Some(JobState::Ready) => self.insert_ready(id),
                        Some(JobState::Delayed) => self.insert_delayed(id),
                        Some(JobState::Buried) => self.insert_buried(id),
                        _ => {}
                    }
                }
            }
        }
    }

    fn ensure_tube(&mut self, name: &str) {
        self.tubes
            .entry(name.to_string())
            .or_insert_with(|| Tube::new(name.to_string()));
    }

    pub(crate) fn connect(&mut self) -> u64 {
        let id = self.next_conn_id;
        self.next_conn_id += 1;
        self.total_connections += 1;
        self.ensure_tube("default");
        if let Some(t) = self.tubes.get_mut("default") {
            t.watching_ct += 1;
            t.using_ct += 1;
        }
        let mut watching = HashSet::new();
        watching.insert("default".to_string());
        self.conns.insert(
            id,
            ConnInfo {
                using: "default".to_string(),
                watching,
                ..ConnInfo::default()
            },
        );
        id
    }

    pub(crate) fn disconnect(&mut self, conn_id: u64) {
        self.cancel_waiter(conn_id, None);
        if let Some(conn) = self.conns.remove(&conn_id) {
            if let Some(t) = self.tubes.get_mut(&conn.using) {
                t.using_ct = t.using_ct.saturating_sub(1);
            }
            for tube in &conn.watching {
                if let Some(t) = self.tubes.get_mut(tube) {
                    t.watching_ct = t.watching_ct.saturating_sub(1);
                }
            }
            for id in conn.reserved {
                self.release_reserved_due_to_close(id);
            }
        }
        self.process_queue();
        self.cleanup_tubes();
    }

    pub(crate) fn command(&mut self, conn_id: u64, cmd: Command) -> CommandResult {
        self.promote_delayed();
        self.timeout_reserved();
        match cmd {
            Command::Put {
                pri,
                delay,
                ttr,
                body,
            } => {
                self.op.put += 1;
                if self.draining {
                    return CommandResult::Immediate(line("DRAINING\r\n"));
                }
                if body.len() > self.cfg.max_job_size {
                    return CommandResult::Immediate(line("JOB_TOO_BIG\r\n"));
                }
                if let Some(c) = self.conns.get_mut(&conn_id) {
                    c.producer = true;
                }
                let tube = self
                    .conns
                    .get(&conn_id)
                    .map(|c| c.using.clone())
                    .unwrap_or_else(|| "default".to_string());
                self.ensure_tube(&tube);
                let id = self.next_job_id;
                self.next_job_id += 1;
                let ttr = ttr.max(Duration::from_secs(1));
                let state = if delay.is_zero() {
                    JobState::Ready
                } else {
                    JobState::Delayed
                };
                let job = Job {
                    id,
                    pri,
                    delay,
                    ttr,
                    body,
                    tube: tube.clone(),
                    state,
                    created_at: Instant::now(),
                    created_unix: unix_now(),
                    deadline_at: (!delay.is_zero()).then(|| Instant::now() + delay),
                    reserve_ct: 0,
                    timeout_ct: 0,
                    release_ct: 0,
                    bury_ct: 0,
                    kick_ct: 0,
                    reserver: None,
                };
                self.jobs.insert(id, job);
                if state == JobState::Ready {
                    self.insert_ready(id);
                } else {
                    self.insert_delayed(id);
                }
                self.global.total_jobs_ct += 1;
                if let Some(t) = self.tubes.get_mut(&tube) {
                    t.stats.total_jobs_ct += 1;
                }
                self.write_wal(WalEvent::Put {
                    id,
                    pri,
                    delay_secs: delay.as_secs(),
                    ttr_secs: ttr.as_secs(),
                    tube,
                    body: general_purpose::STANDARD.encode(&self.jobs[&id].body),
                    created_unix: self.jobs[&id].created_unix,
                });
                self.process_queue();
                CommandResult::Immediate(line(format!("INSERTED {id}\r\n")))
            }
            Command::Use(tube) => {
                self.op.use_cmd += 1;
                if !valid_tube_name(&tube) {
                    return CommandResult::Immediate(line("BAD_FORMAT\r\n"));
                }
                self.ensure_tube(&tube);
                let old = if let Some(c) = self.conns.get_mut(&conn_id) {
                    let old = c.using.clone();
                    c.using = tube.clone();
                    Some(old)
                } else {
                    None
                };
                if let Some(old) = old {
                    if let Some(t) = self.tubes.get_mut(&old) {
                        t.using_ct = t.using_ct.saturating_sub(1);
                    }
                }
                if let Some(t) = self.tubes.get_mut(&tube) {
                    t.using_ct += 1;
                }
                self.cleanup_tubes();
                CommandResult::Immediate(line(format!("USING {tube}\r\n")))
            }
            Command::Watch(tube) => {
                self.op.watch += 1;
                if !valid_tube_name(&tube) {
                    return CommandResult::Immediate(line("BAD_FORMAT\r\n"));
                }
                self.ensure_tube(&tube);
                let mut added = false;
                let count = if let Some(c) = self.conns.get_mut(&conn_id) {
                    added = c.watching.insert(tube.clone());
                    c.watching.len()
                } else {
                    1
                };
                if added {
                    if let Some(t) = self.tubes.get_mut(&tube) {
                        t.watching_ct += 1;
                    }
                }
                CommandResult::Immediate(line(format!("WATCHING {count}\r\n")))
            }
            Command::Ignore(tube) => {
                self.op.ignore += 1;
                let mut removed = false;
                let count = if let Some(c) = self.conns.get_mut(&conn_id) {
                    if c.watching.len() <= 1 && c.watching.contains(&tube) {
                        return CommandResult::Immediate(line("NOT_IGNORED\r\n"));
                    }
                    removed = c.watching.remove(&tube);
                    c.watching.len()
                } else {
                    0
                };
                if removed {
                    if let Some(t) = self.tubes.get_mut(&tube) {
                        t.watching_ct = t.watching_ct.saturating_sub(1);
                    }
                }
                self.cleanup_tubes();
                CommandResult::Immediate(line(format!("WATCHING {count}\r\n")))
            }
            Command::Reserve(timeout) => {
                if timeout.is_some() {
                    self.op.reserve_with_timeout += 1;
                } else {
                    self.op.reserve += 1;
                }
                if let Some(c) = self.conns.get_mut(&conn_id) {
                    c.worker = true;
                }
                if self.deadline_soon(conn_id) && !self.conn_has_ready(conn_id) {
                    return CommandResult::Immediate(line("DEADLINE_SOON\r\n"));
                }
                if let Some(id) = self.find_ready_for_conn(conn_id) {
                    let resp = self.reserve_for_conn(conn_id, id, "RESERVED");
                    return CommandResult::Immediate(resp);
                }
                if timeout == Some(Duration::ZERO) {
                    return CommandResult::Immediate(line("TIMED_OUT\r\n"));
                }
                let (tx, rx) = oneshot::channel();
                let watches = self
                    .conns
                    .get(&conn_id)
                    .map(|c| c.watching.clone())
                    .unwrap_or_default();
                let timeout_at = timeout.map(|d| Instant::now() + d);
                self.mark_waiting(conn_id, &watches, true);
                self.waiters.push_back(Waiter {
                    conn_id,
                    watches,
                    timeout_at,
                    tx,
                });
                CommandResult::Pending(rx)
            }
            Command::ReserveJob(id) => {
                self.op.reserve_job += 1;
                let Some(job) = self.jobs.get(&id) else {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                };
                if job.state == JobState::Reserved {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                self.remove_from_current_queue(id);
                if let Some(c) = self.conns.get_mut(&conn_id) {
                    c.worker = true;
                }
                CommandResult::Immediate(self.reserve_for_conn(conn_id, id, "RESERVED"))
            }
            Command::Delete(id) => {
                self.op.delete += 1;
                let deletable = self
                    .jobs
                    .get(&id)
                    .map(|j| j.state != JobState::Reserved || j.reserver == Some(conn_id))
                    .unwrap_or(false);
                if !deletable {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                let tube = self.jobs.get(&id).map(|j| j.tube.clone());
                self.remove_job(id);
                if let Some(tube) = tube {
                    if let Some(t) = self.tubes.get_mut(&tube) {
                        t.stats.total_delete_ct += 1;
                    }
                }
                self.write_wal(WalEvent::Delete { id });
                self.cleanup_tubes();
                CommandResult::Immediate(line("DELETED\r\n"))
            }
            Command::Release { id, pri, delay } => {
                self.op.release += 1;
                if !self.is_reserved_by(conn_id, id) {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                self.unreserve(conn_id, id);
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.pri = pri;
                    job.delay = delay;
                    job.release_ct += 1;
                    job.reserver = None;
                    job.deadline_at = (!delay.is_zero()).then(|| Instant::now() + delay);
                    job.state = if delay.is_zero() {
                        JobState::Ready
                    } else {
                        JobState::Delayed
                    };
                }
                if delay.is_zero() {
                    self.insert_ready(id);
                    self.write_wal(WalEvent::State {
                        id,
                        state: "ready".to_string(),
                        pri: Some(pri),
                        delay_secs: Some(0),
                    });
                } else {
                    self.insert_delayed(id);
                    self.write_wal(WalEvent::State {
                        id,
                        state: "delayed".to_string(),
                        pri: Some(pri),
                        delay_secs: Some(delay.as_secs()),
                    });
                }
                self.process_queue();
                CommandResult::Immediate(line("RELEASED\r\n"))
            }
            Command::Bury { id, pri } => {
                self.op.bury += 1;
                if !self.is_reserved_by(conn_id, id) {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                self.unreserve(conn_id, id);
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.pri = pri;
                    job.bury_ct += 1;
                    job.state = JobState::Buried;
                    job.reserver = None;
                    job.deadline_at = None;
                }
                self.insert_buried(id);
                self.write_wal(WalEvent::State {
                    id,
                    state: "buried".to_string(),
                    pri: Some(pri),
                    delay_secs: None,
                });
                CommandResult::Immediate(line("BURIED\r\n"))
            }
            Command::Touch(id) => {
                self.op.touch += 1;
                if !self.is_reserved_by(conn_id, id) {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.deadline_at = Some(Instant::now() + job.ttr);
                }
                CommandResult::Immediate(line("TOUCHED\r\n"))
            }
            Command::Peek(id) => {
                self.op.peek += 1;
                match self.jobs.get(&id) {
                    Some(job) => CommandResult::Immediate(job_response("FOUND", job)),
                    None => CommandResult::Immediate(line("NOT_FOUND\r\n")),
                }
            }
            Command::PeekReady => {
                self.op.peek_ready += 1;
                let tube = self.current_tube(conn_id);
                let id = self
                    .tubes
                    .get(&tube)
                    .and_then(|t| t.ready.iter().next().copied())
                    .map(|k| k.id);
                self.peek_id(id)
            }
            Command::PeekDelayed => {
                self.op.peek_delayed += 1;
                let tube = self.current_tube(conn_id);
                let id = self
                    .tubes
                    .get(&tube)
                    .and_then(|t| t.delay.iter().next().copied())
                    .map(|k| k.id);
                self.peek_id(id)
            }
            Command::PeekBuried => {
                self.op.peek_buried += 1;
                let tube = self.current_tube(conn_id);
                let id = self
                    .tubes
                    .get(&tube)
                    .and_then(|t| t.buried.front())
                    .copied();
                self.peek_id(id)
            }
            Command::Kick(bound) => {
                self.op.kick += 1;
                let tube = self.current_tube(conn_id);
                let kicked = self.kick_many(&tube, bound);
                self.process_queue();
                CommandResult::Immediate(line(format!("KICKED {kicked}\r\n")))
            }
            Command::KickJob(id) => {
                self.op.kick_job += 1;
                let kickable = self
                    .jobs
                    .get(&id)
                    .map(|j| matches!(j.state, JobState::Buried | JobState::Delayed))
                    .unwrap_or(false);
                if !kickable {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                }
                self.remove_from_current_queue(id);
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.kick_ct += 1;
                    job.state = JobState::Ready;
                    job.deadline_at = None;
                }
                self.insert_ready(id);
                self.write_wal(WalEvent::State {
                    id,
                    state: "ready".to_string(),
                    pri: None,
                    delay_secs: Some(0),
                });
                self.process_queue();
                CommandResult::Immediate(line("KICKED\r\n"))
            }
            Command::Stats => {
                self.op.stats += 1;
                CommandResult::Immediate(chunk("OK", self.format_stats().into_bytes()))
            }
            Command::StatsJob(id) => {
                self.op.stats_job += 1;
                match self.jobs.get(&id) {
                    Some(job) => CommandResult::Immediate(chunk(
                        "OK",
                        self.format_job_stats(job).into_bytes(),
                    )),
                    None => CommandResult::Immediate(line("NOT_FOUND\r\n")),
                }
            }
            Command::StatsTube(tube) => {
                self.op.stats_tube += 1;
                match self.tubes.get(&tube) {
                    Some(tube) => CommandResult::Immediate(chunk(
                        "OK",
                        self.format_tube_stats(tube).into_bytes(),
                    )),
                    None => CommandResult::Immediate(line("NOT_FOUND\r\n")),
                }
            }
            Command::ListTubes => {
                self.op.list_tubes += 1;
                let mut names: Vec<_> = self.tubes.keys().cloned().collect();
                names.sort();
                CommandResult::Immediate(chunk("OK", yaml_list(&names).into_bytes()))
            }
            Command::ListTubeUsed => {
                self.op.list_tube_used += 1;
                CommandResult::Immediate(line(format!("USING {}\r\n", self.current_tube(conn_id))))
            }
            Command::ListTubesWatched => {
                self.op.list_tubes_watched += 1;
                let mut names: Vec<_> = self
                    .conns
                    .get(&conn_id)
                    .map(|c| c.watching.iter().cloned().collect())
                    .unwrap_or_else(Vec::new);
                names.sort();
                CommandResult::Immediate(chunk("OK", yaml_list(&names).into_bytes()))
            }
            Command::PauseTube { tube, delay } => {
                self.op.pause_tube += 1;
                let Some(t) = self.tubes.get_mut(&tube) else {
                    return CommandResult::Immediate(line("NOT_FOUND\r\n"));
                };
                t.pause = delay;
                t.unpause_at = (!delay.is_zero()).then(|| Instant::now() + delay);
                t.stats.pause_ct += 1;
                CommandResult::Immediate(line("PAUSED\r\n"))
            }
            Command::Quit => CommandResult::Close,
        }
    }

    fn peek_id(&self, id: Option<u64>) -> CommandResult {
        match id.and_then(|id| self.jobs.get(&id)) {
            Some(job) => CommandResult::Immediate(job_response("FOUND", job)),
            None => CommandResult::Immediate(line("NOT_FOUND\r\n")),
        }
    }

    fn current_tube(&self, conn_id: u64) -> String {
        self.conns
            .get(&conn_id)
            .map(|c| c.using.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    fn insert_ready(&mut self, id: u64) {
        let Some(job) = self.jobs.get(&id) else {
            return;
        };
        let key = ReadyKey { pri: job.pri, id };
        let tube_name = job.tube.clone();
        let urgent = job.pri < URGENT_THRESHOLD;
        if let Some(tube) = self.tubes.get_mut(&tube_name) {
            if tube.ready.insert(key) && urgent {
                tube.stats.urgent_ct += 1;
                self.global.urgent_ct += 1;
            }
        }
    }

    fn insert_delayed(&mut self, id: u64) {
        let Some(job) = self.jobs.get(&id) else {
            return;
        };
        let Some(when) = job.deadline_at else { return };
        let tube_name = job.tube.clone();
        if let Some(tube) = self.tubes.get_mut(&tube_name) {
            tube.delay.insert(DelayKey { when, id });
        }
    }

    fn insert_buried(&mut self, id: u64) {
        let Some(job) = self.jobs.get(&id) else {
            return;
        };
        let tube_name = job.tube.clone();
        if let Some(tube) = self.tubes.get_mut(&tube_name) {
            if !tube.buried.contains(&id) {
                tube.buried.push_back(id);
                tube.stats.buried_ct += 1;
                self.global.buried_ct += 1;
            }
        }
    }

    fn remove_from_current_queue(&mut self, id: u64) {
        let Some(job) = self.jobs.get(&id).cloned() else {
            return;
        };
        if let Some(tube) = self.tubes.get_mut(&job.tube) {
            match job.state {
                JobState::Ready => {
                    if tube.ready.remove(&ReadyKey { pri: job.pri, id })
                        && job.pri < URGENT_THRESHOLD
                    {
                        tube.stats.urgent_ct = tube.stats.urgent_ct.saturating_sub(1);
                        self.global.urgent_ct = self.global.urgent_ct.saturating_sub(1);
                    }
                }
                JobState::Delayed => {
                    if let Some(when) = job.deadline_at {
                        tube.delay.remove(&DelayKey { when, id });
                    } else {
                        tube.delay.retain(|k| k.id != id);
                    }
                }
                JobState::Buried => {
                    if let Some(pos) = tube.buried.iter().position(|x| *x == id) {
                        tube.buried.remove(pos);
                        tube.stats.buried_ct = tube.stats.buried_ct.saturating_sub(1);
                        self.global.buried_ct = self.global.buried_ct.saturating_sub(1);
                    }
                }
                JobState::Reserved => {}
            }
        }
    }

    fn remove_job(&mut self, id: u64) {
        self.remove_from_current_queue(id);
        if let Some(job) = self.jobs.remove(&id) {
            if job.state == JobState::Reserved {
                self.global.reserved_ct = self.global.reserved_ct.saturating_sub(1);
                if let Some(t) = self.tubes.get_mut(&job.tube) {
                    t.stats.reserved_ct = t.stats.reserved_ct.saturating_sub(1);
                }
                if let Some(conn_id) = job.reserver {
                    if let Some(c) = self.conns.get_mut(&conn_id) {
                        c.reserved.remove(&id);
                    }
                }
            }
        }
    }

    fn reserve_for_conn(&mut self, conn_id: u64, id: u64, msg: &str) -> Response {
        self.remove_from_current_queue(id);
        if let Some(job) = self.jobs.get_mut(&id) {
            job.state = JobState::Reserved;
            job.reserver = Some(conn_id);
            job.reserve_ct += 1;
            job.deadline_at = Some(Instant::now() + job.ttr);
            self.global.reserved_ct += 1;
            if let Some(t) = self.tubes.get_mut(&job.tube) {
                t.stats.reserved_ct += 1;
            }
            if let Some(c) = self.conns.get_mut(&conn_id) {
                c.reserved.insert(id);
            }
            return job_response(msg, job);
        }
        line("NOT_FOUND\r\n")
    }

    fn unreserve(&mut self, conn_id: u64, id: u64) {
        if let Some(c) = self.conns.get_mut(&conn_id) {
            c.reserved.remove(&id);
        }
        if let Some(job) = self.jobs.get(&id) {
            self.global.reserved_ct = self.global.reserved_ct.saturating_sub(1);
            if let Some(t) = self.tubes.get_mut(&job.tube) {
                t.stats.reserved_ct = t.stats.reserved_ct.saturating_sub(1);
            }
        }
    }

    fn is_reserved_by(&self, conn_id: u64, id: u64) -> bool {
        self.jobs
            .get(&id)
            .map(|j| j.state == JobState::Reserved && j.reserver == Some(conn_id))
            .unwrap_or(false)
    }

    fn release_reserved_due_to_close(&mut self, id: u64) {
        if let Some(job) = self.jobs.get(&id).cloned() {
            if job.state != JobState::Reserved {
                return;
            }
            self.global.reserved_ct = self.global.reserved_ct.saturating_sub(1);
            if let Some(t) = self.tubes.get_mut(&job.tube) {
                t.stats.reserved_ct = t.stats.reserved_ct.saturating_sub(1);
            }
            if let Some(j) = self.jobs.get_mut(&id) {
                j.state = JobState::Ready;
                j.reserver = None;
                j.deadline_at = None;
            }
            self.insert_ready(id);
        }
    }

    fn find_ready_for_conn(&mut self, conn_id: u64) -> Option<u64> {
        let watches = self.conns.get(&conn_id)?.watching.clone();
        let now = Instant::now();
        let mut best: Option<ReadyKey> = None;
        for tube_name in watches {
            let Some(tube) = self.tubes.get_mut(&tube_name) else {
                continue;
            };
            if tube.paused(now) {
                continue;
            }
            if let Some(key) = tube.ready.iter().next().copied() {
                if best.map(|b| key < b).unwrap_or(true) {
                    best = Some(key);
                }
            }
        }
        best.map(|k| k.id)
    }

    fn conn_has_ready(&mut self, conn_id: u64) -> bool {
        self.find_ready_for_conn(conn_id).is_some()
    }

    fn deadline_soon(&self, conn_id: u64) -> bool {
        let now = Instant::now();
        self.conns
            .get(&conn_id)
            .map(|c| {
                c.reserved.iter().any(|id| {
                    self.jobs
                        .get(id)
                        .and_then(|j| j.deadline_at)
                        .map(|d| d <= now + Duration::from_secs(1))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    fn mark_waiting(&mut self, conn_id: u64, watches: &HashSet<String>, waiting: bool) {
        let already = self.conns.get(&conn_id).map(|c| c.waiting).unwrap_or(false);
        if already == waiting {
            return;
        }
        if let Some(c) = self.conns.get_mut(&conn_id) {
            c.waiting = waiting;
        }
        if waiting {
            self.global.waiting_ct += 1;
            for t in watches {
                if let Some(tube) = self.tubes.get_mut(t) {
                    tube.stats.waiting_ct += 1;
                }
            }
        } else {
            self.global.waiting_ct = self.global.waiting_ct.saturating_sub(1);
            for t in watches {
                if let Some(tube) = self.tubes.get_mut(t) {
                    tube.stats.waiting_ct = tube.stats.waiting_ct.saturating_sub(1);
                }
            }
        }
    }

    fn cancel_waiter(&mut self, conn_id: u64, response: Option<Response>) {
        let mut remaining = VecDeque::new();
        while let Some(waiter) = self.waiters.pop_front() {
            if waiter.conn_id == conn_id {
                self.mark_waiting(waiter.conn_id, &waiter.watches, false);
                if let Some(resp) = response.clone() {
                    let _ = waiter.tx.send(resp);
                }
            } else {
                remaining.push_back(waiter);
            }
        }
        self.waiters = remaining;
    }

    fn process_queue(&mut self) {
        let mut rest = VecDeque::new();
        while let Some(waiter) = self.waiters.pop_front() {
            if !self.conns.contains_key(&waiter.conn_id) {
                continue;
            }
            let mut best: Option<ReadyKey> = None;
            let now = Instant::now();
            for tube_name in &waiter.watches {
                let Some(tube) = self.tubes.get_mut(tube_name) else {
                    continue;
                };
                if tube.paused(now) {
                    continue;
                }
                if let Some(key) = tube.ready.iter().next().copied() {
                    if best.map(|b| key < b).unwrap_or(true) {
                        best = Some(key);
                    }
                }
            }
            if let Some(key) = best {
                self.mark_waiting(waiter.conn_id, &waiter.watches, false);
                let resp = self.reserve_for_conn(waiter.conn_id, key.id, "RESERVED");
                let _ = waiter.tx.send(resp);
            } else {
                rest.push_back(waiter);
            }
        }
        self.waiters = rest;
    }

    pub(crate) fn tick(&mut self) {
        self.promote_delayed();
        self.timeout_reserved();
        self.timeout_waiters();
        self.process_queue();
    }

    pub(crate) fn enter_drain_mode(&mut self) {
        self.draining = true;
    }

    fn promote_delayed(&mut self) {
        let now = Instant::now();
        let tubes: Vec<String> = self.tubes.keys().cloned().collect();
        for tube_name in tubes {
            loop {
                let next = self
                    .tubes
                    .get(&tube_name)
                    .and_then(|tube| tube.delay.iter().next().copied());
                let Some(key) = next else { break };
                if key.when > now {
                    break;
                }
                if let Some(tube) = self.tubes.get_mut(&tube_name) {
                    tube.delay.remove(&key);
                }
                if let Some(job) = self.jobs.get_mut(&key.id) {
                    job.state = JobState::Ready;
                    job.deadline_at = None;
                }
                self.insert_ready(key.id);
            }
        }
    }

    fn timeout_reserved(&mut self) {
        let now = Instant::now();
        let ids: Vec<u64> = self
            .jobs
            .iter()
            .filter_map(|(id, job)| {
                (job.state == JobState::Reserved
                    && job.deadline_at.map(|d| d <= now).unwrap_or(false))
                .then_some(*id)
            })
            .collect();
        for id in ids {
            let conn_id = self.jobs.get(&id).and_then(|j| j.reserver);
            if let Some(conn_id) = conn_id {
                self.unreserve(conn_id, id);
            }
            if let Some(job) = self.jobs.get_mut(&id) {
                job.state = JobState::Ready;
                job.reserver = None;
                job.deadline_at = None;
                job.timeout_ct += 1;
            }
            self.job_timeouts += 1;
            self.insert_ready(id);
        }
    }

    fn timeout_waiters(&mut self) {
        let now = Instant::now();
        let mut rest = VecDeque::new();
        while let Some(waiter) = self.waiters.pop_front() {
            if waiter.timeout_at.map(|t| t <= now).unwrap_or(false) {
                self.mark_waiting(waiter.conn_id, &waiter.watches, false);
                let _ = waiter.tx.send(line("TIMED_OUT\r\n"));
            } else if self.deadline_soon(waiter.conn_id) {
                self.mark_waiting(waiter.conn_id, &waiter.watches, false);
                let _ = waiter.tx.send(line("DEADLINE_SOON\r\n"));
            } else {
                rest.push_back(waiter);
            }
        }
        self.waiters = rest;
    }

    fn kick_many(&mut self, tube_name: &str, bound: u32) -> u32 {
        let mut kicked = 0;
        let mut ids = Vec::new();
        if let Some(tube) = self.tubes.get(tube_name) {
            if !tube.buried.is_empty() {
                ids.extend(tube.buried.iter().take(bound as usize).copied());
            } else {
                ids.extend(tube.delay.iter().take(bound as usize).map(|k| k.id));
            }
        }
        for id in ids {
            self.remove_from_current_queue(id);
            if let Some(job) = self.jobs.get_mut(&id) {
                job.kick_ct += 1;
                job.state = JobState::Ready;
                job.deadline_at = None;
            }
            self.insert_ready(id);
            kicked += 1;
            self.write_wal(WalEvent::State {
                id,
                state: "ready".to_string(),
                pri: None,
                delay_secs: Some(0),
            });
        }
        kicked
    }

    fn format_stats(&self) -> String {
        let delayed: usize = self.tubes.values().map(|t| t.delay.len()).sum();
        let ready: usize = self.tubes.values().map(|t| t.ready.len()).sum();
        let producers = self.conns.values().filter(|c| c.producer).count();
        let workers = self.conns.values().filter(|c| c.worker).count();
        let uname = uname_info();
        let rusage = rusage_times();
        format!(
            "---\n\
current-jobs-urgent: {}\n\
current-jobs-ready: {}\n\
current-jobs-reserved: {}\n\
current-jobs-delayed: {}\n\
current-jobs-buried: {}\n\
cmd-put: {}\n\
cmd-peek: {}\n\
cmd-peek-ready: {}\n\
cmd-peek-delayed: {}\n\
cmd-peek-buried: {}\n\
cmd-reserve: {}\n\
cmd-reserve-with-timeout: {}\n\
cmd-delete: {}\n\
cmd-release: {}\n\
cmd-use: {}\n\
cmd-watch: {}\n\
cmd-ignore: {}\n\
cmd-bury: {}\n\
cmd-kick: {}\n\
cmd-touch: {}\n\
cmd-stats: {}\n\
cmd-stats-job: {}\n\
cmd-stats-tube: {}\n\
cmd-list-tubes: {}\n\
cmd-list-tube-used: {}\n\
cmd-list-tubes-watched: {}\n\
cmd-pause-tube: {}\n\
job-timeouts: {}\n\
total-jobs: {}\n\
max-job-size: {}\n\
current-tubes: {}\n\
current-connections: {}\n\
current-producers: {}\n\
current-workers: {}\n\
current-waiting: {}\n\
total-connections: {}\n\
pid: {}\n\
version: \"{}\"\n\
rusage-utime: {}.{:06}\n\
rusage-stime: {}.{:06}\n\
uptime: {}\n\
binlog-oldest-index: {}\n\
binlog-current-index: {}\n\
binlog-records-migrated: 0\n\
binlog-records-written: {}\n\
binlog-max-size: {}\n\
draining: {}\n\
id: {}\n\
hostname: \"{}\"\n\
os: \"{}\"\n\
platform: \"{}\"\n\
\r\n",
            self.global.urgent_ct,
            ready,
            self.global.reserved_ct,
            delayed,
            self.global.buried_ct,
            self.op.put,
            self.op.peek,
            self.op.peek_ready,
            self.op.peek_delayed,
            self.op.peek_buried,
            self.op.reserve,
            self.op.reserve_with_timeout,
            self.op.delete,
            self.op.release,
            self.op.use_cmd,
            self.op.watch,
            self.op.ignore,
            self.op.bury,
            self.op.kick,
            self.op.touch,
            self.op.stats,
            self.op.stats_job,
            self.op.stats_tube,
            self.op.list_tubes,
            self.op.list_tube_used,
            self.op.list_tubes_watched,
            self.op.pause_tube,
            self.job_timeouts,
            self.global.total_jobs_ct,
            self.cfg.max_job_size,
            self.tubes.len(),
            self.conns.len(),
            producers,
            workers,
            self.global.waiting_ct,
            self.total_connections,
            process::id(),
            VERSION,
            rusage.0 .0,
            rusage.0 .1,
            rusage.1 .0,
            rusage.1 .1,
            self.started_at.elapsed().as_secs(),
            self.wal.as_ref().map(|_| 1).unwrap_or(0),
            self.wal.as_ref().map(|_| 1).unwrap_or(0),
            self.wal.as_ref().map(|w| w.records_written).unwrap_or(0),
            self.cfg.wal_file_size,
            if self.draining { "true" } else { "false" },
            self.instance_id,
            uname.0,
            uname.1,
            uname.2,
        )
    }

    fn format_tube_stats(&self, tube: &Tube) -> String {
        let pause_left = tube
            .unpause_at
            .map(|t| t.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0);
        format!(
            "---\n\
name: \"{}\"\n\
current-jobs-urgent: {}\n\
current-jobs-ready: {}\n\
current-jobs-reserved: {}\n\
current-jobs-delayed: {}\n\
current-jobs-buried: {}\n\
total-jobs: {}\n\
current-using: {}\n\
current-watching: {}\n\
current-waiting: {}\n\
cmd-delete: {}\n\
cmd-pause-tube: {}\n\
pause: {}\n\
pause-time-left: {}\n\
\r\n",
            tube.name,
            tube.stats.urgent_ct,
            tube.ready.len(),
            tube.stats.reserved_ct,
            tube.delay.len(),
            tube.stats.buried_ct,
            tube.stats.total_jobs_ct,
            tube.using_ct,
            tube.watching_ct,
            tube.stats.waiting_ct,
            tube.stats.total_delete_ct,
            tube.stats.pause_ct,
            tube.pause.as_secs(),
            pause_left,
        )
    }

    fn format_job_stats(&self, job: &Job) -> String {
        let time_left = job
            .deadline_at
            .map(|d| d.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0);
        format!(
            "---\n\
id: {}\n\
tube: \"{}\"\n\
state: {}\n\
pri: {}\n\
age: {}\n\
delay: {}\n\
ttr: {}\n\
time-left: {}\n\
file: {}\n\
reserves: {}\n\
timeouts: {}\n\
releases: {}\n\
buries: {}\n\
kicks: {}\n\
\r\n",
            job.id,
            job.tube,
            job.state.as_str(),
            job.pri,
            job.created_at.elapsed().as_secs(),
            job.delay.as_secs(),
            job.ttr.as_secs(),
            time_left,
            if self.wal.is_some() { 1 } else { 0 },
            job.reserve_ct,
            job.timeout_ct,
            job.release_ct,
            job.bury_ct,
            job.kick_ct,
        )
    }

    fn write_wal(&mut self, event: WalEvent) {
        if let Some(wal) = self.wal.as_mut() {
            if let Err(err) = wal.write(&event) {
                eprintln!("beanstalkd: wal write failed: {err}");
            }
        }
    }

    fn cleanup_tubes(&mut self) {
        let empty: Vec<String> = self
            .tubes
            .iter()
            .filter_map(|(name, tube)| {
                (tube.using_ct == 0
                    && tube.watching_ct == 0
                    && tube.ready.is_empty()
                    && tube.delay.is_empty()
                    && tube.buried.is_empty()
                    && tube.stats.reserved_ct == 0)
                    .then(|| name.clone())
            })
            .collect();
        for name in empty {
            self.tubes.remove(&name);
        }
    }
}

fn rusage_times() -> ((i64, i64), (i64, i64)) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return ((0, 0), (0, 0));
    }
    let usage = unsafe { usage.assume_init() };
    (
        (usage.ru_utime.tv_sec as i64, usage.ru_utime.tv_usec as i64),
        (usage.ru_stime.tv_sec as i64, usage.ru_stime.tv_usec as i64),
    )
}

fn uname_info() -> (String, String, String) {
    let mut info = std::mem::MaybeUninit::<libc::utsname>::uninit();
    let rc = unsafe { libc::uname(info.as_mut_ptr()) };
    if rc != 0 {
        return (
            "unknown".to_string(),
            "unknown".to_string(),
            "unknown".to_string(),
        );
    }
    let info = unsafe { info.assume_init() };
    (
        c_chars_to_string(&info.nodename),
        c_chars_to_string(&info.version),
        c_chars_to_string(&info.machine),
    )
}

fn c_chars_to_string(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
