use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{Health, RecoveryKind, Signal};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    #[serde(with = "time::serde::rfc3339")]
    pub ts: time::OffsetDateTime,
    pub health: Health,
    pub signal: Option<Signal>,
    pub network: Option<String>,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "detail", rename_all = "snake_case")]
pub enum EventKind {
    AppStarted { version: String, os: String },
    ConnectivityLost { to: Health },
    ConnectionDegraded { failed: u8, slow: u8, window: u8 },
    RecoveryAction { kind: RecoveryKind, ok: bool, duration_ms: u64, error: Option<String> },
    ConnectivityRestored { downtime_ms: u64 },
    WifiOffByUser,
    WifiOnByUser,
    Paused,
    Resumed,
    CredentialImported,
    CredentialMissing { reason: String },
    PermissionMissing { which: String },
}

pub trait EventSink: Send {
    fn write(&mut self, event: &Event) -> std::io::Result<()>;
    fn recent_recovery_actions(&self, n: usize) -> Vec<Event>;
}

pub struct JsonlSink {
    path: PathBuf,
    old_path: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
}

impl JsonlSink {
    pub fn open(dir: &Path, max_bytes: u64) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("events.jsonl");
        let old_path = dir.join("events.1.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        // The file may already hold data from a previous run; rotation must count it.
        let size = file.metadata()?.len();
        Ok(Self { path, old_path, file, size, max_bytes })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        // rename replaces an existing events.1.jsonl, so only one old file is ever kept.
        fs::rename(&self.path, &self.old_path)?;
        self.file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.size = 0;
        Ok(())
    }

    fn read_actions(path: &Path) -> Vec<Event> {
        let Ok(file) = File::open(path) else { return Vec::new() };
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<Event>(&l).ok())
            .filter(|e| matches!(e.kind, EventKind::RecoveryAction { .. }))
            .collect()
    }
}

impl EventSink for JsonlSink {
    fn write(&mut self, event: &Event) -> std::io::Result<()> {
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        // size > 0 guard: a single oversized line must not rotate an empty file forever.
        if self.size > 0 && self.size + line.len() as u64 > self.max_bytes {
            // A failed rotation must not lose the event; keep appending to the current file.
            let _ = self.rotate();
        }
        self.file.write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        Ok(())
    }

    fn recent_recovery_actions(&self, n: usize) -> Vec<Event> {
        let mut actions = Self::read_actions(&self.path);
        if actions.len() < n {
            let mut older = Self::read_actions(&self.old_path);
            older.append(&mut actions);
            actions = older;
        }
        let skip = actions.len().saturating_sub(n);
        actions.drain(..skip);
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind) -> Event {
        Event {
            ts: time::OffsetDateTime::UNIX_EPOCH,
            health: Health::Healthy,
            signal: None,
            network: None,
            kind,
        }
    }

    fn action(ms: u64) -> Event {
        ev(EventKind::RecoveryAction { kind: RecoveryKind::Rejoin, ok: true, duration_ms: ms, error: None })
    }

    fn line_len(e: &Event) -> u64 {
        serde_json::to_string(e).unwrap().len() as u64 + 1
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ar-events-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn rotates_at_limit() {
        let dir = tmp("rotate");
        let l = line_len(&action(1));
        let mut s = JsonlSink::open(&dir, l * 2).unwrap();
        for i in 0..3 {
            s.write(&action(i)).unwrap();
        }
        assert_eq!(fs::read_to_string(dir.join("events.1.jsonl")).unwrap().lines().count(), 2);
        assert_eq!(fs::read_to_string(dir.join("events.jsonl")).unwrap().lines().count(), 1);
    }

    #[test]
    fn keeps_only_one_old_file() {
        let dir = tmp("one-old");
        let l = line_len(&action(1));
        let mut s = JsonlSink::open(&dir, l).unwrap();
        for i in 0..5 {
            s.write(&action(i)).unwrap();
        }
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2);
        // Each line fills the limit, so events.1.jsonl holds only the previous record.
        let old = fs::read_to_string(dir.join("events.1.jsonl")).unwrap();
        assert!(old.contains("\"duration_ms\":3"));
        assert!(!old.contains("\"duration_ms\":0"));
    }

    #[test]
    fn recent_spans_rotation_boundary() {
        let dir = tmp("span");
        let l = line_len(&action(1));
        let mut s = JsonlSink::open(&dir, l * 3).unwrap();
        for i in 0..5 {
            s.write(&action(i)).unwrap();
        }
        // 3 in events.1.jsonl, 2 in events.jsonl.
        let got: Vec<u64> = s
            .recent_recovery_actions(5)
            .into_iter()
            .map(|e| match e.kind {
                EventKind::RecoveryAction { duration_ms, .. } => duration_ms,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
        assert_eq!(s.recent_recovery_actions(3).len(), 3);
    }

    #[test]
    fn write_error_is_returned_not_panicked() {
        let dir = tmp("err");
        let mut s = JsonlSink::open(&dir, 1000).unwrap();
        // A read-only handle makes write_all fail without touching the filesystem layout.
        s.file = File::open(&s.path).unwrap();
        assert!(s.write(&action(1)).is_err());
    }
}
