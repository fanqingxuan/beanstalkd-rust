use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const C_WAL_VERSION: i32 = 7;
const C_WAL_VERSION_5: i32 = 5;
const C_JOBREC_SIZE: usize = 80;
const C_JOBREC5_SIZE: usize = 77;
const MAX_TUBE_NAME_LEN: usize = 201;

#[derive(Clone)]
pub(crate) struct ReplayedJob {
    pub(crate) id: u64,
    pub(crate) pri: u32,
    pub(crate) delay_secs: u64,
    pub(crate) ttr_secs: u64,
    pub(crate) tube: String,
    pub(crate) body: Vec<u8>,
    pub(crate) state: String,
    pub(crate) created_unix: u64,
    pub(crate) reserve_ct: u32,
    pub(crate) timeout_ct: u32,
    pub(crate) release_ct: u32,
    pub(crate) bury_ct: u32,
    pub(crate) kick_ct: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum WalEvent {
    Put {
        id: u64,
        pri: u32,
        delay_secs: u64,
        ttr_secs: u64,
        tube: String,
        body: String,
        created_unix: u64,
    },
    Delete {
        id: u64,
    },
    State {
        id: u64,
        state: String,
        pri: Option<u32>,
        delay_secs: Option<u64>,
    },
}

pub(crate) struct Wal {
    path: PathBuf,
    file: File,
    fsync: bool,
    fsync_ms: u64,
    last_sync: Instant,
    pub(crate) records_written: u64,
}

impl Wal {
    pub(crate) fn open(dir: &Path, fsync: bool, fsync_ms: u64) -> std::io::Result<Self> {
        let path = dir.join("rust-beanstalkd.wal");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            fsync,
            fsync_ms,
            last_sync: Instant::now(),
            records_written: 0,
        })
    }

    pub(crate) fn replay(&mut self) -> std::io::Result<Vec<WalEvent>> {
        let input = File::open(&self.path)?;
        let mut events = Vec::new();
        for line in BufReader::new(input).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<WalEvent>(&line) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub(crate) fn replay_c_binlogs(
        dir: &Path,
        max_job_size: usize,
    ) -> std::io::Result<Vec<ReplayedJob>> {
        let mut files = c_binlog_files(dir)?;
        files.sort_by_key(|(seq, _)| *seq);
        let mut jobs = HashMap::<u64, CJob>::new();
        for (_, path) in files {
            replay_c_binlog_file(&path, max_job_size, &mut jobs)?;
        }
        let mut replayed: Vec<_> = jobs
            .into_values()
            .map(|job| ReplayedJob {
                id: job.id,
                pri: job.pri,
                delay_secs: nanos_to_secs(job.delay_ns),
                ttr_secs: nanos_to_secs(job.ttr_ns).max(1),
                tube: job.tube,
                body: strip_c_body_trailer(job.body),
                state: job.state,
                created_unix: nanos_to_secs(job.created_at_ns),
                reserve_ct: job.reserve_ct,
                timeout_ct: job.timeout_ct,
                release_ct: job.release_ct,
                bury_ct: job.bury_ct,
                kick_ct: job.kick_ct,
            })
            .collect();
        replayed.sort_by_key(|job| job.id);
        Ok(replayed)
    }

    pub(crate) fn write(&mut self, event: &WalEvent) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.file, event)?;
        self.file.write_all(b"\n")?;
        self.records_written += 1;
        if self.fsync
            && (self.fsync_ms == 0
                || self.last_sync.elapsed() >= Duration::from_millis(self.fsync_ms))
        {
            self.file.sync_data()?;
            self.last_sync = Instant::now();
        }
        Ok(())
    }
}

#[derive(Clone)]
struct CJob {
    id: u64,
    pri: u32,
    delay_ns: u64,
    ttr_ns: u64,
    body_size: i32,
    created_at_ns: u64,
    reserve_ct: u32,
    timeout_ct: u32,
    release_ct: u32,
    bury_ct: u32,
    kick_ct: u32,
    state: String,
    tube: String,
    body: Vec<u8>,
}

fn c_binlog_files(dir: &Path) -> std::io::Result<Vec<(i32, PathBuf)>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix("binlog.") else {
            continue;
        };
        if let Ok(seq) = suffix.parse::<i32>() {
            files.push((seq, entry.path()));
        }
    }
    Ok(files)
}

fn replay_c_binlog_file(
    path: &Path,
    max_job_size: usize,
    jobs: &mut HashMap<u64, CJob>,
) -> std::io::Result<()> {
    let mut file = File::open(path)?;
    let Some(version) = read_i32(&mut file)? else {
        return Ok(());
    };
    match version {
        C_WAL_VERSION => replay_c_v7(&mut file, max_job_size, jobs),
        C_WAL_VERSION_5 => replay_c_v5(&mut file, max_job_size, jobs),
        _ => Ok(()),
    }
}

fn replay_c_v7(
    file: &mut File,
    max_job_size: usize,
    jobs: &mut HashMap<u64, CJob>,
) -> std::io::Result<()> {
    loop {
        let Some(namelen) = read_i32(file)? else {
            return Ok(());
        };
        if !(0..MAX_TUBE_NAME_LEN as i32).contains(&namelen) {
            return Ok(());
        }
        let tube = read_tube_name(file, namelen as usize)?;
        let Some(buf) = read_exact_or_eof(file, C_JOBREC_SIZE)? else {
            return Ok(());
        };
        let Some(mut job) = parse_c_jobrec_v7(&buf) else {
            return Ok(());
        };
        apply_c_record(file, max_job_size, jobs, &tube, &mut job)?;
    }
}

fn replay_c_v5(
    file: &mut File,
    max_job_size: usize,
    jobs: &mut HashMap<u64, CJob>,
) -> std::io::Result<()> {
    loop {
        let Some(namelen) = read_usize_native(file)? else {
            return Ok(());
        };
        if namelen >= MAX_TUBE_NAME_LEN {
            return Ok(());
        }
        let tube = read_tube_name(file, namelen)?;
        let Some(buf) = read_exact_or_eof(file, C_JOBREC5_SIZE)? else {
            return Ok(());
        };
        let Some(mut job) = parse_c_jobrec_v5(&buf) else {
            return Ok(());
        };
        apply_c_record(file, max_job_size, jobs, &tube, &mut job)?;
    }
}

fn apply_c_record(
    file: &mut File,
    max_job_size: usize,
    jobs: &mut HashMap<u64, CJob>,
    tube: &str,
    job: &mut CJob,
) -> std::io::Result<()> {
    if job.id == 0 {
        return Ok(());
    }
    if job.state == "invalid" {
        jobs.remove(&job.id);
        return Ok(());
    }
    if tube.is_empty() && !jobs.contains_key(&job.id) {
        return Ok(());
    }
    if job.body_size < 0 || job.body_size as usize > max_job_size + 2 {
        return Ok(());
    }
    if !tube.is_empty() {
        let Some(body) = read_exact_or_eof(file, job.body_size as usize)? else {
            return Ok(());
        };
        job.tube = tube.to_string();
        job.body = body;
    } else if let Some(old) = jobs.get(&job.id) {
        job.tube = old.tube.clone();
        job.body = old.body.clone();
    }
    jobs.insert(job.id, job.clone());
    Ok(())
}

fn parse_c_jobrec_v7(buf: &[u8]) -> Option<CJob> {
    let _deadline_at_ns = le_u64(buf, 48)?;
    Some(CJob {
        id: le_u64(buf, 0)?,
        pri: le_u32(buf, 8)?,
        delay_ns: le_u64(buf, 16)?,
        ttr_ns: le_u64(buf, 24)?,
        body_size: le_i32(buf, 32)?,
        created_at_ns: le_u64(buf, 40)?,
        reserve_ct: le_u32(buf, 56)?,
        timeout_ct: le_u32(buf, 60)?,
        release_ct: le_u32(buf, 64)?,
        bury_ct: le_u32(buf, 68)?,
        kick_ct: le_u32(buf, 72)?,
        state: c_state(buf.get(76).copied()?),
        tube: String::new(),
        body: Vec::new(),
    })
}

fn parse_c_jobrec_v5(buf: &[u8]) -> Option<CJob> {
    let _deadline_at_ns = le_u64(buf, 48)? * 1000;
    Some(CJob {
        id: le_u64(buf, 0)?,
        pri: le_u32(buf, 8)?,
        delay_ns: le_u64(buf, 16)? * 1000,
        ttr_ns: le_u64(buf, 24)? * 1000,
        body_size: le_i32(buf, 32)?,
        created_at_ns: le_u64(buf, 40)? * 1000,
        reserve_ct: le_u32(buf, 56)?,
        timeout_ct: le_u32(buf, 60)?,
        release_ct: le_u32(buf, 64)?,
        bury_ct: le_u32(buf, 68)?,
        kick_ct: le_u32(buf, 72)?,
        state: c_state(buf.get(76).copied()?),
        tube: String::new(),
        body: Vec::new(),
    })
}

fn c_state(state: u8) -> String {
    match state {
        0 => "invalid",
        2 => "ready",
        3 => "buried",
        4 => "delayed",
        _ => "ready",
    }
    .to_string()
}

fn strip_c_body_trailer(mut body: Vec<u8>) -> Vec<u8> {
    if body.ends_with(b"\r\n") {
        body.truncate(body.len() - 2);
    }
    body
}

fn read_tube_name(file: &mut File, len: usize) -> std::io::Result<String> {
    if len == 0 {
        return Ok(String::new());
    }
    let Some(bytes) = read_exact_or_eof(file, len)? else {
        return Ok(String::new());
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_i32(file: &mut File) -> std::io::Result<Option<i32>> {
    let Some(buf) = read_exact_or_eof(file, 4)? else {
        return Ok(None);
    };
    Ok(Some(i32::from_le_bytes(buf.try_into().unwrap())))
}

fn read_usize_native(file: &mut File) -> std::io::Result<Option<usize>> {
    let Some(buf) = read_exact_or_eof(file, std::mem::size_of::<usize>())? else {
        return Ok(None);
    };
    let mut bytes = [0_u8; std::mem::size_of::<usize>()];
    bytes.copy_from_slice(&buf);
    Ok(Some(usize::from_le_bytes(bytes)))
}

fn read_exact_or_eof(file: &mut File, len: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0_u8; len];
    match file.read_exact(&mut buf) {
        Ok(()) => Ok(Some(buf)),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err),
    }
}

fn le_u64(buf: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        buf.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn le_u32(buf: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        buf.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn le_i32(buf: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        buf.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn nanos_to_secs(nanos: u64) -> u64 {
    nanos / 1_000_000_000
}
