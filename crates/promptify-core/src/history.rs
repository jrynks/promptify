use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::pipeline::Mode;
use crate::profiles::Example;

/// One saved job. Never contains surrounding screen text or audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub id: u64,
    pub created_ms: u64,
    pub mode: Mode,
    pub profile_id: String,
    pub app_key: String,
    pub transcript: String,
    pub output: String,
    /// False when the output was shown in the overlay instead of pasted.
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHistoryEntry {
    pub mode: Mode,
    pub profile_id: String,
    pub app_key: String,
    pub transcript: String,
    pub output: String,
    pub inserted: bool,
}

#[derive(Debug, Clone)]
pub struct HistoryLimits {
    pub max_entries: usize,
    pub max_examples: usize,
    /// Longer past jobs are skipped as examples rather than cut mid-thought.
    pub max_example_chars: usize,
    pub follow_up_window: Duration,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self { max_entries: 500, max_examples: 3, max_example_chars: 1200, follow_up_window: Duration::from_secs(10 * 60) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousPrompt {
    pub text: String,
    pub minutes_ago: u64,
}

/// What past jobs contribute to a new prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryContext {
    /// The user's own accepted spoken-request → prompt pairs for this profile, oldest first.
    pub examples: Vec<Example>,
    /// The last prompt pasted into the same app, for follow-ups like "make it shorter".
    pub previous: Option<PreviousPrompt>,
}

pub fn select_context(entries: &[HistoryEntry], profile_id: &str, app_key: &str, now_ms: u64, limits: &HistoryLimits) -> HistoryContext {
    let usable = |e: &&HistoryEntry| e.mode == Mode::Prompt && e.inserted;
    let previous_entry = entries
        .iter()
        .rev()
        .filter(usable)
        .find(|e| e.app_key == app_key)
        .filter(|e| now_ms.saturating_sub(e.created_ms) <= limits.follow_up_window.as_millis() as u64);
    let previous = previous_entry.map(|e| PreviousPrompt {
        text: e.output.clone(),
        minutes_ago: now_ms.saturating_sub(e.created_ms) / 60_000,
    });

    let mut examples: Vec<Example> = entries
        .iter()
        .rev()
        .filter(usable)
        .filter(|e| e.profile_id == profile_id)
        .filter(|e| previous_entry.is_none_or(|p| p.id != e.id))
        .filter(|e| e.transcript.chars().count() <= limits.max_example_chars && e.output.chars().count() <= limits.max_example_chars)
        .take(limits.max_examples)
        .map(|e| Example { said: e.transcript.clone(), prompt: e.output.clone() })
        .collect();
    examples.reverse();
    HistoryContext { examples, previous }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadReport {
    pub loaded: usize,
    /// Lines that were not valid entries (e.g. a torn write) and were dropped.
    pub skipped: usize,
}

/// Append-only JSON Lines history, bounded to `max_entries` and cached in memory.
pub struct HistoryLog {
    path: PathBuf,
    limits: HistoryLimits,
    enabled: AtomicBool,
    local: Mutex<()>,
}

/// Holds the inter-process lock; the app and the CLI can share one history file.
struct Locked<'a> {
    _local: std::sync::MutexGuard<'a, ()>,
    file: File,
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl HistoryLog {
    pub fn open(path: PathBuf, limits: HistoryLimits, enabled: bool) -> io::Result<(Self, LoadReport)> {
        let log = Self { path, limits, enabled: AtomicBool::new(enabled), local: Mutex::new(()) };
        let _lock = log.lock()?;
        let (mut entries, mut report) = log.load()?;
        let over = entries.len().saturating_sub(log.limits.max_entries);
        entries.drain(..over);
        if report.skipped > 0 || over > 0 {
            log.rewrite(&entries)?;
        }
        report.loaded = entries.len();
        drop(_lock);
        Ok((log, report))
    }

    fn lock(&self) -> io::Result<Locked<'_>> {
        let local = self.local.lock().unwrap();
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.path.with_extension("jsonl.lock"))?;
        file.lock()?;
        Ok(Locked { _local: local, file })
    }

    fn load(&self) -> io::Result<(Vec<HistoryEntry>, LoadReport)> {
        match File::open(&self.path) {
            Ok(file) => read_entries(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Vec::new(), LoadReport { loaded: 0, skipped: 0 })),
            Err(e) => Err(e),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Reads the file each time so entries written by another process are visible.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        let loaded = self.lock().and_then(|_lock| self.load());
        loaded.map(|(entries, _)| entries).unwrap_or_default()
    }

    pub fn context(&self, profile_id: &str, app_key: &str) -> HistoryContext {
        if !self.is_enabled() {
            return HistoryContext::default();
        }
        select_context(&self.entries(), profile_id, app_key, now_ms(), &self.limits)
    }

    /// Returns the stored entry, or `None` when history is turned off.
    pub fn record(&self, new: NewHistoryEntry) -> io::Result<Option<HistoryEntry>> {
        if !self.is_enabled() {
            return Ok(None);
        }
        let _lock = self.lock()?;
        let (mut entries, report) = self.load()?;
        let now = now_ms();
        // Time-based ids stay unique across processes and after a clear.
        let id = entries.iter().map(|e| e.id + 1).max().unwrap_or(0).max(now);
        let entry = HistoryEntry {
            id,
            created_ms: now,
            mode: new.mode,
            profile_id: new.profile_id,
            app_key: new.app_key,
            transcript: new.transcript,
            output: new.output,
            inserted: new.inserted,
        };
        entries.push(entry.clone());
        if entries.len() > self.limits.max_entries || report.skipped > 0 {
            let over = entries.len().saturating_sub(self.limits.max_entries);
            entries.drain(..over);
            self.rewrite(&entries)?;
        } else {
            append_line(&self.path, &entry)?;
        }
        Ok(Some(entry))
    }

    pub fn delete(&self, id: u64) -> io::Result<bool> {
        let _lock = self.lock()?;
        let (entries, _) = self.load()?;
        let next: Vec<HistoryEntry> = entries.iter().filter(|e| e.id != id).cloned().collect();
        if next.len() == entries.len() {
            return Ok(false);
        }
        self.rewrite(&next)?;
        Ok(true)
    }

    pub fn clear(&self) -> io::Result<()> {
        let _lock = self.lock()?;
        self.rewrite(&[])
    }

    fn rewrite(&self, entries: &[HistoryEntry]) -> io::Result<()> {
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&tmp)?;
            for entry in entries {
                writeln!(file, "{}", serde_json::to_string(entry)?)?;
            }
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
    }
}

fn read_entries(file: File) -> io::Result<(Vec<HistoryEntry>, LoadReport)> {
    let mut entries = Vec::new();
    let mut skipped = 0;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEntry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(_) => skipped += 1,
        }
    }
    Ok((entries, LoadReport { loaded: 0, skipped }))
}

fn append_line(path: &Path, entry: &HistoryEntry) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(format!("{}\n", serde_json::to_string(entry)?).as_bytes())?;
    file.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn entry(id: u64, created_ms: u64, profile: &str, app: &str, mode: Mode, inserted: bool) -> HistoryEntry {
        HistoryEntry {
            id,
            created_ms,
            mode,
            profile_id: profile.into(),
            app_key: app.into(),
            transcript: format!("said {id}"),
            output: format!("prompt {id}"),
            inserted,
        }
    }

    fn new(transcript: &str) -> NewHistoryEntry {
        NewHistoryEntry {
            mode: Mode::Prompt,
            profile_id: "claude".into(),
            app_key: "claude.ai".into(),
            transcript: transcript.into(),
            output: format!("P: {transcript}"),
            inserted: true,
        }
    }

    #[test]
    fn examples_are_same_profile_accepted_prompts_oldest_first() {
        let entries = vec![
            entry(1, 0, "claude", "claude.ai", Mode::Prompt, true),
            entry(2, MIN, "chatgpt", "chatgpt.com", Mode::Prompt, true),
            entry(3, 2 * MIN, "claude", "claude.ai", Mode::Prompt, true),
            entry(4, 3 * MIN, "claude", "claude.ai", Mode::Prompt, true),
            entry(5, 4 * MIN, "claude", "claude.ai", Mode::Dictation, true),
            entry(6, 5 * MIN, "claude", "claude.ai", Mode::Prompt, false),
        ];
        let limits = HistoryLimits { max_examples: 2, ..Default::default() };
        let ctx = select_context(&entries, "claude", "other-app", 100 * MIN, &limits);
        let said: Vec<_> = ctx.examples.iter().map(|e| e.said.as_str()).collect();
        assert_eq!(said, ["said 3", "said 4"]);
        assert_eq!(ctx.previous, None);
    }

    #[test]
    fn previous_prompt_only_within_window_and_same_app() {
        let entries = vec![entry(1, 0, "claude", "claude.ai", Mode::Prompt, true), entry(2, MIN, "claude", "claude.ai", Mode::Prompt, true)];
        let limits = HistoryLimits::default();
        let ctx = select_context(&entries, "claude", "claude.ai", 4 * MIN, &limits);
        assert_eq!(ctx.previous, Some(PreviousPrompt { text: "prompt 2".into(), minutes_ago: 3 }));
        assert_eq!(ctx.examples.len(), 1, "the previous prompt is not duplicated as an example");
        assert_eq!(select_context(&entries, "claude", "claude.ai", 30 * MIN, &limits).previous, None);
        assert_eq!(select_context(&entries, "claude", "chatgpt.com", 4 * MIN, &limits).previous, None);
    }

    #[test]
    fn overlong_entries_are_skipped_not_cut() {
        let mut long = entry(1, 0, "claude", "x", Mode::Prompt, true);
        long.output = "y".repeat(50);
        let limits = HistoryLimits { max_example_chars: 10, ..Default::default() };
        assert!(select_context(&[long], "claude", "z", 0, &limits).examples.is_empty());
    }

    #[test]
    fn persists_reloads_and_bounds_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let limits = HistoryLimits { max_entries: 2, ..Default::default() };
        let (log, _) = HistoryLog::open(path.clone(), limits.clone(), true).unwrap();
        for text in ["a", "b", "c"] {
            log.record(new(text)).unwrap();
        }
        assert_eq!(log.entries().len(), 2, "the live log is bounded, not only on reopen");
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
        let (reopened, report) = HistoryLog::open(path.clone(), limits, true).unwrap();
        let said: Vec<_> = reopened.entries().into_iter().map(|e| e.transcript).collect();
        assert_eq!(said, ["b", "c"]);
        assert_eq!(report, LoadReport { loaded: 2, skipped: 0 });
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    }

    #[test]
    fn torn_lines_are_dropped_and_compacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let good = serde_json::to_string(&entry(1, 0, "claude", "x", Mode::Prompt, true)).unwrap();
        fs::write(&path, format!("{good}\n{{\"id\":2,\"crea")).unwrap();
        let (log, report) = HistoryLog::open(path.clone(), HistoryLimits::default(), true).unwrap();
        assert_eq!(report, LoadReport { loaded: 1, skipped: 1 });
        assert_eq!(log.entries().len(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{good}\n"));
    }

    #[test]
    fn disabled_history_neither_records_nor_supplies_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let (log, _) = HistoryLog::open(path.clone(), HistoryLimits::default(), true).unwrap();
        log.record(new("kept")).unwrap();
        log.set_enabled(false);
        assert_eq!(log.record(new("dropped")).unwrap(), None);
        assert_eq!(log.context("claude", "claude.ai"), HistoryContext::default());
        assert_eq!(log.entries().len(), 1);
        assert!(!fs::read_to_string(&path).unwrap().contains("dropped"));
    }

    #[test]
    fn two_handles_share_one_file_without_losing_or_reusing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let limits = HistoryLimits { max_entries: 3, ..Default::default() };
        let (app, _) = HistoryLog::open(path.clone(), limits.clone(), true).unwrap();
        let (cli, _) = HistoryLog::open(path.clone(), limits, true).unwrap();
        let a = app.record(new("from app")).unwrap().unwrap();
        let b = cli.record(new("from cli")).unwrap().unwrap();
        app.record(new("app again")).unwrap();
        cli.record(new("cli again")).unwrap();
        let said: Vec<_> = app.entries().into_iter().map(|e| e.transcript).collect();
        assert_eq!(said, ["from cli", "app again", "cli again"]);
        assert!(b.id > a.id);
        let middle = app.entries()[1].id;
        assert!(cli.delete(middle).unwrap());
        assert_eq!(cli.entries().len(), 2);
        app.clear().unwrap();
        let after = cli.record(new("after clear")).unwrap().unwrap();
        assert!(after.id > b.id, "ids are never reused after a clear");
    }

    #[test]
    fn concurrent_writers_through_separate_handles_keep_unique_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let handles: Vec<_> = (0..4).map(|_| std::sync::Arc::new(HistoryLog::open(path.clone(), HistoryLimits::default(), true).unwrap().0)).collect();
        let threads: Vec<_> = handles
            .iter()
            .cloned()
            .enumerate()
            .map(|(t, log)| std::thread::spawn(move || (0..25).for_each(|i| drop(log.record(new(&format!("{t}-{i}"))).unwrap()))))
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let entries = handles[0].entries();
        assert_eq!(entries.len(), 100);
        let ids: std::collections::BTreeSet<_> = entries.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 100, "every entry has a unique id");
    }

    #[test]
    fn delete_and_clear_rewrite_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let (log, _) = HistoryLog::open(path.clone(), HistoryLimits::default(), true).unwrap();
        let first = log.record(new("one")).unwrap().unwrap();
        log.record(new("two")).unwrap();
        assert!(log.delete(first.id).unwrap());
        assert!(!log.delete(first.id).unwrap());
        assert!(!fs::read_to_string(&path).unwrap().contains("\"one\""));
        log.clear().unwrap();
        assert!(log.entries().is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }
}
