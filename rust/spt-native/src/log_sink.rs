//! The file sink behind `FileLogHandler`.
//!
//! One background writer thread per configured log file, owning the file handle, the day/size
//! rolling and the retention sweep. On the C# side those were split between ZLogger's rolling
//! provider (which chose filenames) and `LogFileRollMonitor` (which polled the directory every
//! minute re-deriving the same filenames to decide what to delete); here one owner does both, so
//! retention runs exactly when a roll happens instead of on a timer.
//!
//! The configured name is always the live file: a server start, a size roll and a date change each
//! move the previous file aside to the next free `name.N.ext` and open a fresh one, so `spt.log`
//! only ever holds the current run. The single exception is a second handler opening a path this
//! process already freshened - see `freshened_paths`.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

/// A handle to one log file's writer thread. Lines are handed over a channel, so a logging call
/// on a request thread costs a send and never touches the filesystem.
pub struct FileSink {
    sender: Option<Sender<Vec<u8>>>,
    worker: Option<JoinHandle<()>>,
}

impl FileSink {
    pub fn open(
        dir: &str,
        pattern: &str,
        max_file_size_mb: u32,
        max_rolling_files: u32,
    ) -> io::Result<Self> {
        let mut writer = Writer::open(dir, pattern, max_file_size_mb, max_rolling_files)?;
        let (sender, receiver) = channel::<Vec<u8>>();
        let worker = std::thread::Builder::new()
            .name("spt-log-sink".to_owned())
            .spawn(move || run(&mut writer, &receiver))?;

        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    /// Queues one line. A write that cannot be delivered is dropped rather than propagated: losing
    /// a log line is not worth failing the request that emitted it.
    pub fn write(&self, line: Vec<u8>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(line);
        }
    }

    /// Flushes and joins the writer thread. Dropping the sender is what tells the thread to stop,
    /// so this must run before the process exits or buffered lines are lost.
    pub fn close(mut self) {
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// `spt_log_write` shares one sink across every logging thread through a raw pointer, which opts
/// out of the checking that would otherwise catch a field going non-`Sync`. Assert it here.
const _: fn() = || {
    fn assert_sync<T: Sync>() {}
    assert_sync::<FileSink>();
};

/// Blocks for a line, then drains everything else that queued up while blocked, so a burst of
/// lines costs one flush instead of one per line. Returns when every sender is gone.
fn run(writer: &mut Writer, receiver: &Receiver<Vec<u8>>) {
    while let Ok(first) = receiver.recv() {
        let _ = writer.write_line(&first);
        while let Ok(next) = receiver.try_recv() {
            let _ = writer.write_line(&next);
        }
        let _ = writer.file.flush();
    }

    let _ = writer.file.flush();
}

struct Writer {
    dir: PathBuf,
    pattern: String,
    /// 0 means never roll on size.
    max_size: u64,
    /// 0 means keep every archived file.
    max_rolling: usize,
    date: String,
    file: BufWriter<File>,
    written: u64,
}

impl Writer {
    fn open(
        dir: &str,
        pattern: &str,
        max_file_size_mb: u32,
        max_rolling_files: u32,
    ) -> io::Result<Self> {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir)?;

        let date = utc_date();
        let file = open_live(&dir, &replace_date(pattern, &date))?;
        let written = file.metadata()?.len();

        let writer = Self {
            dir,
            pattern: pattern.to_owned(),
            max_size: u64::from(max_file_size_mb) * 1024 * 1024,
            max_rolling: max_rolling_files as usize,
            date,
            file: BufWriter::new(file),
            written,
        };
        writer.cleanup();

        Ok(writer)
    }

    fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        let today = utc_date();
        if today != self.date {
            self.date = today;
            self.roll()?;
        } else if self.max_size > 0
            && self.written > 0
            && self.written + line.len() as u64 + 1 > self.max_size
        {
            self.roll()?;
        }

        self.file.write_all(line)?;
        self.file.write_all(b"\n")?;
        self.written += line.len() as u64 + 1;

        Ok(())
    }

    /// Archives the live file and opens a fresh one under the same name. A date change lands here
    /// too: `%DATE%` has already moved the live name on, so the archive step finds nothing to move
    /// and this is just the new day's file being created.
    fn roll(&mut self) -> io::Result<()> {
        self.file.flush()?;

        let file = open_rolled(&self.dir, &replace_date(&self.pattern, &self.date))?;
        self.file = BufWriter::new(file);
        self.written = 0;
        self.cleanup();

        Ok(())
    }

    /// Deletes all but the `max_rolling` most recent files of the *current* date's set, counting
    /// the live file as one of them.
    ///
    /// Scoping the sweep to one date is what the C# `LogFileRollMonitor` did, and is preserved
    /// deliberately: widening it to every date in the directory would start deleting previous
    /// days' logs that installs today keep.
    fn cleanup(&self) {
        if self.max_rolling == 0 {
            return;
        }

        let file_name = replace_date(&self.pattern, &self.date);
        let (stem, extension) = split_extension(&file_name);

        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };

        // The live file is always the newest and archives always take the next free sequence, so
        // rank alone is an exact recency order - no filesystem timestamp needed, and none of the
        // ties a second-resolution one produces during a burst of rolls.
        let mut files: Vec<(u32, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                let rank = if name == file_name {
                    u32::MAX
                } else {
                    archived_sequence(name, stem, extension)?
                };

                Some((rank, entry.path()))
            })
            .collect();

        if files.len() <= self.max_rolling {
            return;
        }

        files.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));

        for (_, path) in files.iter().skip(self.max_rolling) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Paths this process has already started a fresh file for.
///
/// A prepatcher mod hands `SPTarkov.Common` a second copy in its own `AssemblyLoadContext`, so one
/// server start builds two `FileLogHandler`s aimed at the same file. This library is loaded once
/// per process however many managed copies exist, which makes it the only place that can tell that
/// second open apart from a genuine restart.
fn freshened_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static FRESHENED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

    FRESHENED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether this is the process's first open of `path`. A poisoned lock answers yes, which at worst
/// archives one extra time rather than silently appending to a previous run's file.
fn claim_first_open(path: PathBuf) -> bool {
    freshened_paths()
        .lock()
        .map(|mut paths| paths.insert(path))
        .unwrap_or(true)
}

/// Opens the file this run logs to: archived and started empty on the process's first open,
/// appended to on any later one so a prepatcher's second handler shares the same file.
fn open_live(dir: &Path, file_name: &str) -> io::Result<File> {
    if claim_first_open(dir.join(file_name)) {
        return archive_and_truncate(dir, file_name);
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file_name))
}

/// A roll always starts a new file. Claiming the path keeps a later handler appending to what this
/// roll just started instead of archiving it out from under us.
fn open_rolled(dir: &Path, file_name: &str) -> io::Result<File> {
    claim_first_open(dir.join(file_name));

    archive_and_truncate(dir, file_name)
}

fn archive_and_truncate(dir: &Path, file_name: &str) -> io::Result<File> {
    archive(dir, file_name);

    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dir.join(file_name))
}

/// Renames the live file to the next free `name.N.ext`. An empty one is left to be truncated in
/// place, so a burst of restarts that log nothing does not spend the retention window on blanks.
fn archive(dir: &Path, file_name: &str) {
    let live = dir.join(file_name);

    match fs::metadata(&live) {
        Ok(metadata) if metadata.len() > 0 => {}
        _ => return,
    }

    let (stem, extension) = split_extension(file_name);
    let sequence = next_archive_sequence(dir, stem, extension);

    let _ = fs::rename(&live, dir.join(format!("{stem}.{sequence}{extension}")));
}

fn next_archive_sequence(dir: &Path, stem: &str, extension: &str) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 1;
    };

    entries
        .flatten()
        .filter_map(|entry| archived_sequence(entry.file_name().to_str()?, stem, extension))
        .max()
        .map_or(1, |highest| highest.saturating_add(1))
}

/// Case-insensitive `%DATE%` substitution, matching the C# `StringComparison.OrdinalIgnoreCase`.
fn replace_date(pattern: &str, date: &str) -> String {
    const TOKEN: &str = "%date%";

    let lowered = pattern.to_ascii_lowercase();
    let mut out = String::with_capacity(pattern.len());
    let mut at = 0;

    while let Some(found) = lowered[at..].find(TOKEN) {
        let start = at + found;
        out.push_str(&pattern[at..start]);
        out.push_str(date);
        at = start + TOKEN.len();
    }
    out.push_str(&pattern[at..]);

    out
}

/// Splits at the last dot, extension first-class with its dot, matching
/// `Path.GetFileNameWithoutExtension`/`GetExtension`.
fn split_extension(file_name: &str) -> (&str, &str) {
    match file_name.rfind('.') {
        Some(index) => file_name.split_at(index),
        None => (file_name, ""),
    }
}

/// The `N` of an archived `stem.N.ext`, or `None` if the name is not one.
fn archived_sequence(file_name: &str, stem: &str, extension: &str) -> Option<u32> {
    let rest = file_name.strip_prefix(stem)?.strip_prefix('.')?;
    let sequence = rest.strip_suffix(extension)?;

    sequence.parse().ok()
}

fn utc_date() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);

    format!("{year:04}{month:02}{day:02}")
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch to a proleptic Gregorian date.
/// Only the date is needed, so this is cheaper than taking on a calendar crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11016), (2000, 2, 29));
        assert_eq!(civil_from_days(20681), (2026, 8, 16));
    }

    #[test]
    fn replaces_date_case_insensitively() {
        assert_eq!(replace_date("spt%DATE%.log", "20260816"), "spt20260816.log");
        assert_eq!(replace_date("spt%date%.log", "20260816"), "spt20260816.log");
        assert_eq!(replace_date("spt.log", "20260816"), "spt.log");
    }

    #[test]
    fn recognises_only_numeric_archive_suffixes() {
        assert_eq!(archived_sequence("spt.4.log", "spt", ".log"), Some(4));
        assert_eq!(archived_sequence("spt.log", "spt", ".log"), None);
        assert_eq!(archived_sequence("spt.old.log", "spt", ".log"), None);
        assert_eq!(archived_sequence("other.1.log", "spt", ".log"), None);
    }

    fn live_path(dir: &TempDir) -> PathBuf {
        dir.path().join(replace_date("spt%DATE%.log", &utc_date()))
    }

    fn archive_path(dir: &TempDir, sequence: u32) -> PathBuf {
        let file_name = replace_date("spt%DATE%.log", &utc_date());
        let (stem, extension) = split_extension(&file_name);

        dir.path().join(format!("{stem}.{sequence}{extension}"))
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    /// The freshened-path registry is per process, so an in-process test that means "the server
    /// started again" has to say so explicitly. Scoped to this test's own directory, since tests
    /// run in parallel and share the registry.
    fn simulate_restart(dir: &TempDir) {
        freshened_paths()
            .lock()
            .unwrap()
            .retain(|path| !path.starts_with(dir.path()));
    }

    #[test]
    fn each_start_archives_the_previous_file_and_opens_a_fresh_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        for run in 0..3 {
            simulate_restart(&dir);
            let sink = FileSink::open(path, "spt%DATE%.log", 10, 5).unwrap();
            sink.write(format!("run {run}").into_bytes());
            sink.close();
        }

        // Newest run in the live file, the two before it archived oldest-sequence-first.
        assert_eq!(read(&live_path(&dir)), "run 2\n");
        assert_eq!(read(&archive_path(&dir, 1)), "run 0\n");
        assert_eq!(read(&archive_path(&dir, 2)), "run 1\n");
    }

    /// A prepatcher mod's second copy of SPTarkov.Common opens the same target within one start;
    /// both phases have to land in that start's single file.
    #[test]
    fn a_second_handler_in_the_same_process_shares_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        simulate_restart(&dir);

        let prepatch = FileSink::open(path, "spt%DATE%.log", 10, 5).unwrap();
        prepatch.write(b"prepatch".to_vec());
        prepatch.close();

        let main = FileSink::open(path, "spt%DATE%.log", 10, 5).unwrap();
        main.write(b"main".to_vec());
        main.close();

        assert_eq!(read(&live_path(&dir)), "prepatch\nmain\n");
        assert!(
            !archive_path(&dir, 1).exists(),
            "the second handler must not archive the file the first just started"
        );
    }

    #[test]
    fn a_start_that_logs_nothing_does_not_spend_an_archive_slot() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        simulate_restart(&dir);
        let sink = FileSink::open(path, "spt%DATE%.log", 10, 5).unwrap();
        sink.write(b"kept".to_vec());
        sink.close();

        for _ in 0..3 {
            simulate_restart(&dir);
            FileSink::open(path, "spt%DATE%.log", 10, 5)
                .unwrap()
                .close();
        }

        assert_eq!(read(&archive_path(&dir, 1)), "kept\n");
        assert!(!archive_path(&dir, 2).exists());
        assert_eq!(read(&live_path(&dir)), "");
    }

    #[test]
    fn rolls_on_size_and_keeps_only_the_retention_window() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        // 1 MB limit, 2 files kept: write enough 64 KiB lines to force several rolls.
        let mut writer = Writer::open(path, "spt%DATE%.log", 1, 2).unwrap();
        let line = vec![b'x'; 64 * 1024];
        for _ in 0..48 {
            writer.write_line(&line).unwrap();
        }
        writer.file.flush().unwrap();

        let remaining = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 2, "retention should have pruned to 2 files");

        // A prune never takes the live file, and takes the lowest sequence - the oldest - first.
        assert!(live_path(&dir).exists());
        assert!(
            !archive_path(&dir, 1).exists(),
            "the oldest archive should have gone first"
        );
    }

    #[test]
    fn close_flushes_every_queued_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        let sink = FileSink::open(path, "spt%DATE%.log", 0, 0).unwrap();
        for index in 0..1000 {
            sink.write(format!("line {index}").into_bytes());
        }
        sink.close();

        assert_eq!(read(&live_path(&dir)).lines().count(), 1000);
    }
}
