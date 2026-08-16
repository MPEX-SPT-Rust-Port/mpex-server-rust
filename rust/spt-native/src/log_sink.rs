//! The file sink behind `FileLogHandler`.
//!
//! One background writer thread per configured log file, owning the file handle, the rotation and
//! the archive cap. On the C# side those were split between ZLogger's rolling provider, which chose
//! filenames but has no retention option at all, and `LogFileRollMonitor`, which polled the
//! directory every minute re-deriving the same filenames to decide what to delete. Here one owner
//! does both, so the cap is enforced exactly when a rotation happens instead of on a timer.
//!
//! The configured name is always the live file: a server start, a size roll and a date change each
//! cascade the archive set down one index - `name.1.ext` becomes `name.2.ext` and so on, with the
//! highest index deleted rather than shifted - and open a fresh live file, so `spt.log` only ever
//! holds the current run. The single exception is a second handler opening a path this process
//! already freshened - see `freshened_paths`.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

/// A handle to one log file's writer thread. Lines are handed over a channel, so a logging call
/// on a request thread never blocks on the writer thread's filesystem work.
pub struct FileSink {
    sender: Option<SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<()>>,
}

/// The number of files kept when the config does not set one: the live file plus `.1`..`.9`.
const DEFAULT_MAX_ROLLING: usize = 10;

/// Lines queued before writes start being dropped. A stalled disk must not grow the queue without
/// limit, and a log line is not worth failing the request that emitted it.
const QUEUE_CAPACITY: usize = 8192;

impl FileSink {
    pub fn open(
        dir: &str,
        pattern: &str,
        max_file_size_mb: u32,
        max_rolling_files: u32,
    ) -> io::Result<Self> {
        let mut writer = Writer::open(dir, pattern, max_file_size_mb, max_rolling_files)?;
        let (sender, receiver) = sync_channel::<Vec<u8>>(QUEUE_CAPACITY);
        let worker = std::thread::Builder::new()
            .name("spt-log-sink".to_owned())
            .spawn(move || run(&mut writer, &receiver))?;

        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    /// Queues one line. A write that cannot be delivered - the queue is full, or the writer thread
    /// is gone - is dropped rather than propagated: losing a log line is not worth failing the
    /// request that emitted it, and it is certainly not worth blocking it.
    pub fn write(&self, line: Vec<u8>) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(line);
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
    /// Never 0 - `Writer::open` substitutes `DEFAULT_MAX_ROLLING`.
    max_rolling: usize,
    date: i64,
    file: BufWriter<File>,
    written: u64,
    /// Set when the live file's last roll appended instead of renaming - a failed rename leaves
    /// `written` at its already-over-cap length, which would otherwise satisfy the size check again
    /// on the very next line and re-run the cascade once per line, deleting one archive each time.
    roll_blocked: bool,
    /// Counts `roll()` invocations. Test-only: proves a blocked roll is not retried on every
    /// subsequent line, which the final on-disk bytes cannot show - an appended-in-place roll
    /// leaves the same content on disk whether it ran once or a hundred times.
    #[cfg(test)]
    roll_calls: u32,
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

        let date = utc_days();
        let max_rolling = if max_rolling_files == 0 {
            DEFAULT_MAX_ROLLING
        } else {
            max_rolling_files as usize
        };
        let file = open_live(
            &dir,
            &replace_date(pattern, &format_date(date)),
            max_rolling,
        )?;
        let written = file.metadata()?.len();

        Ok(Self {
            dir,
            pattern: pattern.to_owned(),
            max_size: u64::from(max_file_size_mb) * 1024 * 1024,
            max_rolling,
            date,
            file: BufWriter::new(file),
            written,
            // Not `written > 0`: `open_live` deliberately appends to a non-empty file when a
            // second handler in this process opens a path it already freshened (see
            // `freshened_paths`), which is normal, not a blocked rename. Starting `false` costs at
            // most one extra cascade attempt on a genuinely blocked start.
            roll_blocked: false,
            #[cfg(test)]
            roll_calls: 0,
        })
    }

    fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        let today = utc_days();
        if today != self.date {
            self.date = today;
            self.roll()?;
        } else if self.max_size > 0
            && self.written > 0
            && self.written + line.len() as u64 + 1 > self.max_size
            && !self.roll_blocked
        {
            self.roll()?;
        }

        self.file.write_all(line)?;
        self.file.write_all(b"\n")?;
        self.written += line.len() as u64 + 1;

        Ok(())
    }

    /// Cascades the archive set and opens a fresh live file. A date change lands here too: with the
    /// date out of the configured name a new day is an ordinary rotation, not a new filename.
    fn roll(&mut self) -> io::Result<()> {
        #[cfg(test)]
        {
            self.roll_calls += 1;
        }
        self.file.flush()?;

        let file = open_rolled(
            &self.dir,
            &replace_date(&self.pattern, &format_date(self.date)),
            self.max_rolling,
        )?;
        self.written = file.metadata()?.len();
        // A successful archive always leaves an empty live file, so a non-zero length here means
        // the rename was blocked and we appended instead. Re-running the cascade on every line
        // would delete one archive per line, so hold off until a roll succeeds or the date turns.
        self.roll_blocked = self.written > 0;
        self.file = BufWriter::new(file);

        Ok(())
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

/// Opens the file this run logs to: cascaded and started empty on the process's first open,
/// appended to on any later one so a prepatcher's second handler shares the same file.
fn open_live(dir: &Path, file_name: &str, max_rolling: usize) -> io::Result<File> {
    if claim_first_open(dir.join(file_name)) {
        return archive_and_open(dir, file_name, max_rolling);
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file_name))
}

/// A roll always starts a new file. Claiming the path keeps a later handler appending to what this
/// roll just started instead of cascading it out from under us.
fn open_rolled(dir: &Path, file_name: &str, max_rolling: usize) -> io::Result<File> {
    claim_first_open(dir.join(file_name));

    archive_and_open(dir, file_name, max_rolling)
}

fn archive_and_open(dir: &Path, file_name: &str, max_rolling: usize) -> io::Result<File> {
    let archived = cascade(dir, file_name, max_rolling);

    OpenOptions::new()
        .create(true)
        .write(true)
        // A rename that failed - on Windows anything holding the file open is enough - must not
        // cost the log: append to what is there rather than truncating it away.
        .truncate(archived)
        .append(!archived)
        .open(dir.join(file_name))
}

/// Shifts the archive set down one index and frees `.1` for the live file. The highest index is
/// deleted rather than moved, so the set never grows past `max_rolling` files counting the live
/// one, and no directory scan is needed to decide what to keep.
///
/// Returns whether the live file was moved aside. A `false` means the caller must append to it
/// instead of truncating it.
fn cascade(dir: &Path, file_name: &str, max_rolling: usize) -> bool {
    // A cap of one leaves no room for an archive: the live file is simply started over.
    if max_rolling <= 1 {
        return true;
    }

    let live = dir.join(file_name);

    match fs::metadata(&live) {
        Ok(metadata) if metadata.len() > 0 => {}
        // Nothing worth keeping. Truncating an empty or absent file in place costs no slot, so a
        // burst of restarts that log nothing does not spend the retention window on blanks.
        _ => return true,
    }

    let (stem, extension) = split_extension(file_name);
    let indexed = |index: usize| dir.join(format!("{stem}.{index}{extension}"));

    let highest = max_rolling - 1;
    let _ = fs::remove_file(indexed(highest));

    for index in (1..highest).rev() {
        let _ = fs::rename(indexed(index), indexed(index + 1));
    }

    // ponytail: indices at or above `highest` left by a config change that lowered the cap are
    // not swept - delete them by hand, or widen this to a scan if it ever matters.
    fs::rename(&live, indexed(1)).is_ok()
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

fn utc_days() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    (seconds / 86_400) as i64
}

fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);

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
    fn format_date_matches_known_days() {
        assert_eq!(format_date(0), "19700101");
        assert_eq!(format_date(11016), "20000229");
        assert_eq!(format_date(20681), "20260816");
    }

    #[test]
    fn replaces_date_case_insensitively() {
        assert_eq!(replace_date("spt%DATE%.log", "20260816"), "spt20260816.log");
        assert_eq!(replace_date("spt%date%.log", "20260816"), "spt20260816.log");
        assert_eq!(replace_date("spt.log", "20260816"), "spt.log");
    }

    fn live_path(dir: &TempDir) -> PathBuf {
        dir.path()
            .join(replace_date("spt%DATE%.log", &format_date(utc_days())))
    }

    fn archive_path(dir: &TempDir, sequence: u32) -> PathBuf {
        let file_name = replace_date("spt%DATE%.log", &format_date(utc_days()));
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
    fn each_start_cascades_the_previous_files_down_one_index() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        for run in 0..3 {
            simulate_restart(&dir);
            let sink = FileSink::open(path, "spt%DATE%.log", 10, 10).unwrap();
            sink.write(format!("run {run}").into_bytes());
            sink.close();
        }

        // Newest run in the live file, and each older run one index further down.
        assert_eq!(read(&live_path(&dir)), "run 2\n");
        assert_eq!(read(&archive_path(&dir, 1)), "run 1\n");
        assert_eq!(read(&archive_path(&dir, 2)), "run 0\n");
    }

    #[test]
    fn rotating_at_the_cap_deletes_the_highest_index() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        // A cap of 3 leaves room for the live file plus .1 and .2.
        for run in 0..5 {
            simulate_restart(&dir);
            let sink = FileSink::open(path, "spt%DATE%.log", 10, 3).unwrap();
            sink.write(format!("run {run}").into_bytes());
            sink.close();
        }

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
        assert_eq!(read(&live_path(&dir)), "run 4\n");
        assert_eq!(read(&archive_path(&dir, 1)), "run 3\n");
        assert_eq!(read(&archive_path(&dir, 2)), "run 2\n");
        assert!(
            !archive_path(&dir, 3).exists(),
            "the cap must delete the highest index, not grow past it"
        );
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
    fn rolls_on_size_within_the_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        // 1 MB limit, 3 files kept: write enough 64 KiB lines to force several rolls.
        let mut writer = Writer::open(path, "spt%DATE%.log", 1, 3).unwrap();
        let line = vec![b'x'; 64 * 1024];
        for _ in 0..48 {
            writer.write_line(&line).unwrap();
        }
        writer.file.flush().unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
        assert!(live_path(&dir).exists());
        assert!(!archive_path(&dir, 3).exists());
    }

    /// Occupies the archive slot with a non-empty directory, which no rename can replace,
    /// standing in for the real-world case: something on Windows still holding the file open.
    /// The previous run's log must survive that, not be truncated away.
    #[test]
    fn a_failed_rename_appends_instead_of_truncating() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        // A cap of 2 leaves `.1` as the only archive slot, so the shift loop is empty and the
        // blocker below cannot be relocated out of the way ahead of the live file's rename.
        simulate_restart(&dir);
        let sink = FileSink::open(path, "spt%DATE%.log", 10, 2).unwrap();
        sink.write(b"first run".to_vec());
        sink.close();

        // Occupy the destination with a non-empty directory, which no rename can replace.
        let blocked = archive_path(&dir, 1);
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("occupied"), b"x").unwrap();

        simulate_restart(&dir);
        let sink = FileSink::open(path, "spt%DATE%.log", 10, 2).unwrap();
        sink.write(b"second run".to_vec());
        sink.close();

        assert_eq!(read(&live_path(&dir)), "first run\nsecond run\n");
    }

    /// A blocked roll appends in place, so the bytes on disk look identical whether the cascade
    /// ran once or on every line since - a content assertion alone cannot catch a regression here.
    /// `roll_calls` is the one deterministic signal: unfixed, every one of the oversized lines
    /// below re-enters `roll()` and re-runs the cascade (the review's "eight lines is enough to
    /// wipe a 10-file archive set"); fixed, `roll_blocked` gates the size check after the first.
    #[test]
    fn a_blocked_rename_does_not_re_cascade_every_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        // Cap of 2 leaves `.1` as the only archive slot and empties the shift loop - same
        // unmovable-blocker construction as `a_failed_rename_appends_instead_of_truncating`,
        // just tripped by size instead of a restart.
        let mut writer = Writer::open(path, "spt%DATE%.log", 1, 2).unwrap();
        let line = vec![b'x'; 64 * 1024];

        // Fill to just under the 1 MiB cap without rolling yet.
        for _ in 0..15 {
            writer.write_line(&line).unwrap();
        }

        // Occupy the only archive slot before the size check ever trips.
        let blocked = archive_path(&dir, 1);
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("occupied"), b"x").unwrap();

        // Each of these individually exceeds what is left under the cap, so every one is a
        // fresh chance for the unfixed code to re-cascade.
        for _ in 0..20 {
            writer.write_line(&line).unwrap();
        }
        writer.file.flush().unwrap();

        assert_eq!(
            writer.roll_calls, 1,
            "a blocked roll must not be retried on every subsequent line"
        );
        assert!(
            blocked.join("occupied").exists(),
            "the blocked archive slot must survive untouched"
        );
        assert_eq!(
            read(&live_path(&dir)).lines().count(),
            35,
            "every line must still land in the live file"
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
