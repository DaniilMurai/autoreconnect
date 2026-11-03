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
    file: File,
}

impl JsonlSink {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("events.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventSink for JsonlSink {
    fn write(&mut self, event: &Event) -> std::io::Result<()> {
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())
    }

    fn recent_recovery_actions(&self, n: usize) -> Vec<Event> {
        let Ok(file) = File::open(&self.path) else { return Vec::new() };
        let mut actions: Vec<Event> = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<Event>(&l).ok())
            .filter(|e| matches!(e.kind, EventKind::RecoveryAction { .. }))
            .collect();
        let skip = actions.len().saturating_sub(n);
        actions.drain(..skip);
        actions
    }
}
