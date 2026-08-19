//! The C# logging pipeline behind `SPTLoggerDispatcher`: `SptLoggerConfiguration` parsing, filter
//! matching, format expansion, and the console/file sinks. C# builds an `SptLogMessage` and hands
//! it across `spt_log_emit`; everything downstream lives here.

use std::io::Write;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use crate::log_sink::{FileSink, civil_from_days};
use serde::{Deserialize, Deserializer};

/// Matches one of `variants` case-insensitively, the way C#'s `JsonStringEnumConverter` reads the
/// three converter-backed enums below. The `type` tag of a `loggers` entry is deliberately *not*
/// routed through here: C#'s hand-written `BaseSptLoggerReferenceConverter` is case-sensitive.
fn deserialize_case_insensitive<'de, D, T>(
    deserializer: D,
    variants: &[(&str, T)],
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Copy,
{
    let text = String::deserialize(deserializer)?;

    variants
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&text))
        .map(|(_, value)| *value)
        .ok_or_else(|| {
            let names: Vec<&str> = variants.iter().map(|(name, _)| *name).collect();
            serde::de::Error::custom(format!(
                "unknown value '{text}', expected one of {}",
                names.join(", ")
            ))
        })
}

/// Mirrors Microsoft.Extensions.Logging.LogLevel: the declaration order is the numeric order the
/// C# side sends across the boundary, and `can_log` is `messageLevel >= loggerLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Information,
    Warning,
    Error,
    Critical,
    None,
}

impl LogLevel {
    pub fn from_i32(value: i32) -> Option<LogLevel> {
        match value {
            0 => Some(LogLevel::Trace),
            1 => Some(LogLevel::Debug),
            2 => Some(LogLevel::Information),
            3 => Some(LogLevel::Warning),
            4 => Some(LogLevel::Error),
            5 => Some(LogLevel::Critical),
            6 => Some(LogLevel::None),
            _ => Option::None,
        }
    }

    /// The `%level%` rendering, matching C#'s `LogLevel.ToString()`.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "Trace",
            LogLevel::Debug => "Debug",
            LogLevel::Information => "Information",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
            LogLevel::Critical => "Critical",
            LogLevel::None => "None",
        }
    }
}

impl<'de> Deserialize<'de> for LogLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<LogLevel, D::Error> {
        deserialize_case_insensitive(
            deserializer,
            &[
                ("Trace", LogLevel::Trace),
                ("Debug", LogLevel::Debug),
                ("Information", LogLevel::Information),
                ("Warning", LogLevel::Warning),
                ("Error", LogLevel::Error),
                ("Critical", LogLevel::Critical),
                ("None", LogLevel::None),
            ],
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SptLoggerFilterType {
    Exclude,
    Include,
}

impl<'de> Deserialize<'de> for SptLoggerFilterType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<SptLoggerFilterType, D::Error> {
        deserialize_case_insensitive(
            deserializer,
            &[
                ("Exclude", SptLoggerFilterType::Exclude),
                ("Include", SptLoggerFilterType::Include),
            ],
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchingType {
    Literal,
    Regex,
}

impl<'de> Deserialize<'de> for MatchingType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<MatchingType, D::Error> {
        deserialize_case_insensitive(
            deserializer,
            &[
                ("Literal", MatchingType::Literal),
                ("Regex", MatchingType::Regex),
            ],
        )
    }
}

#[derive(Debug, Deserialize)]
pub struct SptLoggerFilter {
    #[serde(rename = "type")]
    pub filter_type: SptLoggerFilterType,
    pub name: String,
    #[serde(rename = "matchingType")]
    pub matching_type: MatchingType,
}

/// One entry of the `loggers` array; the serde tag subsumes C#'s
/// `BaseSptLoggerReferenceConverter`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum SptLoggerReference {
    File {
        #[serde(rename = "logLevel")]
        log_level: LogLevel,
        format: String,
        #[serde(default)]
        filters: Vec<SptLoggerFilter>,
        #[serde(rename = "filePath")]
        file_path: String,
        #[serde(rename = "filePattern")]
        file_pattern: String,
        #[serde(rename = "maxFileSizeMB", default)]
        max_file_size_mb: u32,
        #[serde(rename = "maxRollingFiles", default)]
        max_rolling_files: u32,
    },
    Console {
        #[serde(rename = "logLevel")]
        log_level: LogLevel,
        format: String,
        #[serde(default)]
        filters: Vec<SptLoggerFilter>,
    },
}

/// The whole `sptLogger.json` document, exactly as C# reads it.
#[derive(Debug, Deserialize)]
pub struct SptLoggerConfiguration {
    pub loggers: Vec<SptLoggerReference>,
}

/// One log line as it crosses the FFI boundary, borrowed from the caller's buffers.
pub struct LogRecord<'a> {
    pub category: &'a str,
    pub message: &'a str,
    /// Pre-rendered by C# as `"{Exception.Message}\n{Exception.StackTrace}"`; empty when none.
    pub exception: &'a str,
    pub thread_name: &'a str,
    pub level: LogLevel,
    pub tid: i32,
    pub unix_millis: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FormatToken {
    Literal(String),
    Date,
    Time,
    Message,
    LoggerShort,
    Logger,
    Tid,
    Tname,
    Level,
}

/// Case-sensitive token substitution, mirroring the `string.Replace` chain in C#'s
/// `BaseSptLoggerReference.GetCompiledFormat`. `%loggerShort%` must be tried before `%logger%`,
/// exactly as the C# replacement order does.
const TOKENS: [(&str, FormatToken); 8] = [
    ("%date%", FormatToken::Date),
    ("%time%", FormatToken::Time),
    ("%message%", FormatToken::Message),
    ("%loggerShort%", FormatToken::LoggerShort),
    ("%logger%", FormatToken::Logger),
    ("%tid%", FormatToken::Tid),
    ("%tname%", FormatToken::Tname),
    ("%level%", FormatToken::Level),
];

pub fn compile_format(format: &str) -> Vec<FormatToken> {
    let mut tokens = Vec::new();
    let mut literal = String::new();
    let mut rest = format;

    'outer: while !rest.is_empty() {
        if rest.starts_with('%') {
            for (text, token) in TOKENS {
                if rest.starts_with(text) {
                    if !literal.is_empty() {
                        tokens.push(FormatToken::Literal(std::mem::take(&mut literal)));
                    }
                    tokens.push(token);
                    rest = &rest[text.len()..];
                    continue 'outer;
                }
            }
        }

        let mut chars = rest.chars();
        literal.push(chars.next().expect("rest is non-empty"));
        rest = chars.as_str();
    }

    if !literal.is_empty() {
        tokens.push(FormatToken::Literal(literal));
    }

    tokens
}

/// Renders one line without its trailing newline - the sink owns the terminator, matching how the
/// C# handlers passed unterminated lines to the sink.
pub fn render(tokens: &[FormatToken], record: &LogRecord) -> String {
    let days = record.unix_millis.div_euclid(86_400_000);
    let ms_of_day = record.unix_millis.rem_euclid(86_400_000);
    let mut out = String::new();

    for token in tokens {
        match token {
            FormatToken::Literal(text) => out.push_str(text),
            FormatToken::Date => {
                let (year, month, day) = civil_from_days(days);
                out.push_str(&format!("{year:04}-{month:02}-{day:02}"));
            }
            FormatToken::Time => {
                let (hours, minutes) = (ms_of_day / 3_600_000, ms_of_day % 3_600_000 / 60_000);
                let (seconds, millis) = (ms_of_day % 60_000 / 1_000, ms_of_day % 1_000);
                out.push_str(&format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}"));
            }
            FormatToken::Message => out.push_str(record.message),
            FormatToken::LoggerShort => {
                let short = match record.category.rfind('.') {
                    Some(index) => &record.category[index + 1..],
                    Option::None => record.category,
                };
                out.push_str(short);
            }
            FormatToken::Logger => out.push_str(record.category),
            FormatToken::Tid => out.push_str(&record.tid.to_string()),
            FormatToken::Tname => out.push_str(record.thread_name),
            FormatToken::Level => out.push_str(record.level.as_str()),
        }
    }

    if !record.exception.is_empty() {
        out.push('\n');
        out.push_str(record.exception);
    }

    out
}

enum CompiledMatcher {
    Literal(String),
    Regex(regex_lite::Regex),
    /// An empty name, or a pattern regex-lite cannot compile (.NET-only syntax). Warned about
    /// once at init; a broken filter must not block startup.
    Never,
}

pub struct CompiledFilter {
    pub filter_type: SptLoggerFilterType,
    matcher: CompiledMatcher,
}

impl CompiledFilter {
    pub fn compile(filter: &SptLoggerFilter) -> CompiledFilter {
        let matcher = if filter.name.is_empty() {
            CompiledMatcher::Never
        } else {
            match filter.matching_type {
                MatchingType::Literal => CompiledMatcher::Literal(filter.name.clone()),
                MatchingType::Regex => match regex_lite::Regex::new(&filter.name) {
                    Ok(regex) => CompiledMatcher::Regex(regex),
                    Err(error) => {
                        eprintln!(
                            "sptLogger.json filter '{}' is not a supported regex and will never match: {error}",
                            filter.name
                        );
                        CompiledMatcher::Never
                    }
                },
            }
        };

        CompiledFilter {
            filter_type: filter.filter_type,
            matcher,
        }
    }

    pub fn matches(&self, category: &str) -> bool {
        if category.is_empty() {
            return false;
        }

        match &self.matcher {
            CompiledMatcher::Literal(name) => name == category,
            CompiledMatcher::Regex(regex) => regex.is_match(category),
            CompiledMatcher::Never => false,
        }
    }
}

/// The per-logger decision `SPTLoggerDispatcher.Log` makes: any exclude match drops the line; if
/// include filters exist at least one must match; then the level gate.
pub fn should_emit(
    level: LogLevel,
    filters: &[CompiledFilter],
    record_level: LogLevel,
    category: &str,
) -> bool {
    let excludes = filters
        .iter()
        .filter(|f| f.filter_type == SptLoggerFilterType::Exclude);
    let mut includes = filters
        .iter()
        .filter(|f| f.filter_type == SptLoggerFilterType::Include)
        .peekable();

    for exclude in excludes {
        if exclude.matches(category) {
            return false;
        }
    }

    if includes.peek().is_some() && !includes.any(|f| f.matches(category)) {
        return false;
    }

    record_level >= level
}

/// Console lines queued before writes start being dropped; same policy as the file sink's queue.
const CONSOLE_QUEUE_CAPACITY: usize = 8192;

/// Boxed so a test can substitute the terminal; production always hands over `std::io::stdout()`.
type ConsoleWriter = Box<dyn Write + Send>;

/// What travels through the console channel. `Line` is a rendered log line (the sink appends the
/// newline, and a full queue drops it — same policy as before). `Raw` is a verbatim byte
/// passthrough from `spt_console_write` — no newline synthesis, and senders block rather than
/// drop, because a prompt must reach the terminal. `Flush` is a drain barrier: the writer thread
/// flushes and acks, so a stdin read can wait until everything queued before it is visible. Its ack
/// channel must have capacity >= 1, never a rendezvous channel: on capacity 0 a requester that gave
/// up while still holding its `Receiver` alive would park the writer thread inside `ack.send`.
// `Raw` and `Flush` are constructed by the FFI console entry points, which land in a later commit;
// until then only tests build them.
#[allow(dead_code)]
pub(crate) enum ConsoleMessage {
    Line(Vec<u8>),
    Raw(Vec<u8>),
    Flush(SyncSender<()>),
}

/// Stdout twin of `FileSink`: a writer thread behind a bounded channel, so a blocked terminal
/// stalls the writer thread, not the logging call.
struct ConsoleSink {
    sender: Option<SyncSender<ConsoleMessage>>,
    worker: Option<JoinHandle<()>>,
}

impl ConsoleSink {
    fn open() -> std::io::Result<ConsoleSink> {
        ConsoleSink::open_with(Box::new(std::io::stdout()))
    }

    fn open_with(out: ConsoleWriter) -> std::io::Result<ConsoleSink> {
        let (sender, receiver) = sync_channel::<ConsoleMessage>(CONSOLE_QUEUE_CAPACITY);
        let worker = std::thread::Builder::new()
            .name("spt-log-console".to_owned())
            .spawn(move || console_run(&receiver, out))?;

        Ok(ConsoleSink {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn write(&self, line: Vec<u8>) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(ConsoleMessage::Line(line));
        }
    }

    fn close(mut self) {
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Drains bursts the same way the file writer does: block for one message, drain the backlog, one
/// flush per burst. `Stdout`'s internal locking replaces the old burst-scoped `lock()` — and
/// `spt_console_write`'s raw bytes now travel through this same queue, which is the point: one
/// FIFO is what orders raw writes against log lines.
fn console_run(receiver: &Receiver<ConsoleMessage>, mut out: ConsoleWriter) {
    while let Ok(first) = receiver.recv() {
        write_console_message(&mut out, first);
        while let Ok(next) = receiver.try_recv() {
            write_console_message(&mut out, next);
        }
        let _ = out.flush();
    }
}

fn write_console_message(out: &mut ConsoleWriter, message: ConsoleMessage) {
    match message {
        ConsoleMessage::Line(line) => {
            let _ = out.write_all(&line);
            let _ = out.write_all(b"\n");
        }
        ConsoleMessage::Raw(bytes) => {
            let _ = out.write_all(&bytes);
        }
        ConsoleMessage::Flush(ack) => {
            let _ = out.flush();
            let _ = ack.send(());
        }
    }
}

enum Sink {
    File(FileSink),
    Console(ConsoleSink),
}

struct LoggerEntry {
    level: LogLevel,
    filters: Vec<CompiledFilter>,
    tokens: Vec<FormatToken>,
    sink: Sink,
}

/// The whole pipeline: what `SPTLoggerDispatcher` + the handlers were on the C# side.
pub struct Logger {
    entries: Vec<LoggerEntry>,
}

// The FFI layer keeps one `Logger` in a process-global and emits from every request thread.
const _: fn() = || {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Logger>();
};

impl Logger {
    /// Parses the raw `sptLogger.json` bytes and opens every sink. Unparseable JSON is the only
    /// fatal error; a file target that cannot be opened is reported to stderr and skipped, the
    /// same per-target disable the C# `FileLogHandler` applied.
    pub fn from_json(bytes: &[u8]) -> Result<Logger, String> {
        // C# reads the same file through `JsonSerializer.Deserialize(Stream)`, which skips a UTF-8
        // BOM; serde_json does not.
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let config: SptLoggerConfiguration =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;

        let mut entries = Vec::new();
        for reference in config.loggers {
            let (level, format, filters, sink) = match reference {
                SptLoggerReference::File {
                    log_level,
                    format,
                    filters,
                    file_path,
                    file_pattern,
                    max_file_size_mb,
                    max_rolling_files,
                } => {
                    match FileSink::open(
                        &file_path,
                        &file_pattern,
                        max_file_size_mb,
                        max_rolling_files,
                    ) {
                        Ok(sink) => (log_level, format, filters, Sink::File(sink)),
                        Err(error) => {
                            eprintln!(
                                "Failed to open log file '{file_path}{file_pattern}': {error}. File logging for this target is disabled."
                            );
                            continue;
                        }
                    }
                }
                SptLoggerReference::Console {
                    log_level,
                    format,
                    filters,
                } => match ConsoleSink::open() {
                    Ok(sink) => (log_level, format, filters, Sink::Console(sink)),
                    Err(error) => {
                        eprintln!(
                            "Failed to start the console log writer: {error}. Console logging is disabled."
                        );
                        continue;
                    }
                },
            };

            entries.push(LoggerEntry {
                level,
                filters: filters.iter().map(CompiledFilter::compile).collect(),
                tokens: compile_format(&format),
                sink,
            });
        }

        Ok(Logger { entries })
    }

    pub fn emit(&self, record: &LogRecord) {
        for entry in &self.entries {
            if !should_emit(entry.level, &entry.filters, record.level, record.category) {
                continue;
            }

            let line = render(&entry.tokens, record).into_bytes();
            match &entry.sink {
                Sink::File(sink) => sink.write(line),
                Sink::Console(sink) => sink.write(line),
            }
        }
    }

    /// A clone of the first console sink's channel, for `spt_console_write`'s raw passthrough.
    /// None when no console logger is configured (or every one failed to open).
    ///
    /// A caller that stores this clone must drop it before `Logger::close`: a live sender keeps the
    /// channel connected, so the sink's `worker.join()` waits on a `recv` that never disconnects and
    /// wedges shutdown and reconfigure. A clone that outlives its `Logger` is also useless — every
    /// `send` fails once the writer is gone, so raw writes would silently vanish.
    // The FFI caller lands in a later commit; until then this is reachable only from tests.
    #[allow(dead_code)]
    pub(crate) fn console_sender(&self) -> Option<SyncSender<ConsoleMessage>> {
        self.entries.iter().find_map(|entry| match &entry.sink {
            Sink::Console(sink) => sink.sender.clone(),
            Sink::File(_) => Option::None,
        })
    }

    /// The `IsLogEnabled` gate: does any configured target's level admit `level`? Level-only by
    /// design — the C# original never consulted filters, and neither does this.
    pub fn enabled(&self, level: LogLevel) -> bool {
        self.entries.iter().any(|entry| level >= entry.level)
    }

    /// Flushes and joins every writer thread.
    pub fn close(self) {
        for entry in self.entries {
            match entry.sink {
                Sink::File(sink) => sink.close(),
                Sink::Console(sink) => sink.close(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `loggers` entries mirror SPTarkov.Server/sptLogger.json.
    const REAL_CONFIG: &str = r#"{
        "loggers": [
            {
                "type": "File",
                "logLevel": "Information",
                "format": "[%date% %time%][%level%][%logger%] %message%",
                "filePath": "./user/logs/spt/",
                "filePattern": "spt.log",
                "maxFileSizeMB": 8,
                "maxRollingFiles": 10,
                "filters": [
                    { "type": "Exclude", "name": ".*RequestLogger", "matchingType": "Regex" }
                ]
            },
            {
                "type": "Console",
                "logLevel": "Information",
                "format": "%message%",
                "filters": [
                    { "type": "Exclude", "name": "Microsoft.*", "matchingType": "Regex" }
                ]
            }
        ]
    }"#;

    #[test]
    fn parses_the_shipped_config_shape() {
        let config: SptLoggerConfiguration = serde_json::from_str(REAL_CONFIG).unwrap();
        assert_eq!(config.loggers.len(), 2);

        let SptLoggerReference::File {
            log_level,
            format,
            filters,
            file_path,
            file_pattern,
            max_file_size_mb,
            max_rolling_files,
        } = &config.loggers[0]
        else {
            panic!("first logger should be a File reference");
        };
        assert_eq!(*log_level, LogLevel::Information);
        assert_eq!(format, "[%date% %time%][%level%][%logger%] %message%");
        assert_eq!(file_path, "./user/logs/spt/");
        assert_eq!(file_pattern, "spt.log");
        assert_eq!(*max_file_size_mb, 8);
        assert_eq!(*max_rolling_files, 10);
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].filter_type, SptLoggerFilterType::Exclude);
        assert_eq!(filters[0].name, ".*RequestLogger");
        assert_eq!(filters[0].matching_type, MatchingType::Regex);

        let SptLoggerReference::Console { filters, .. } = &config.loggers[1] else {
            panic!("second logger should be a Console reference");
        };
        assert_eq!(filters.len(), 1);
    }

    #[test]
    fn missing_filters_and_sizes_default() {
        let json = r#"{ "loggers": [ { "type": "File", "logLevel": "Debug",
            "format": "%message%", "filePath": "./x/", "filePattern": "a.log" } ] }"#;
        let config: SptLoggerConfiguration = serde_json::from_str(json).unwrap();
        let SptLoggerReference::File {
            filters,
            max_file_size_mb,
            max_rolling_files,
            ..
        } = &config.loggers[0]
        else {
            panic!("expected a File reference");
        };
        assert!(filters.is_empty());
        assert_eq!(*max_file_size_mb, 0);
        assert_eq!(*max_rolling_files, 0);
    }

    #[test]
    fn an_unknown_logger_type_is_an_error() {
        let json = r#"{ "loggers": [ { "type": "Syslog", "logLevel": "Debug", "format": "x" } ] }"#;
        assert!(serde_json::from_str::<SptLoggerConfiguration>(json).is_err());
    }

    #[test]
    fn log_level_order_and_int_mapping_match_microsoft() {
        // Microsoft.Extensions.Logging.LogLevel: Trace=0 .. Critical=5, None=6.
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Information);
        assert!(LogLevel::Information < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Critical);
        assert!(LogLevel::Critical < LogLevel::None);
        assert_eq!(LogLevel::from_i32(0), Some(LogLevel::Trace));
        assert_eq!(LogLevel::from_i32(5), Some(LogLevel::Critical));
        assert_eq!(LogLevel::from_i32(6), Some(LogLevel::None));
        assert_eq!(LogLevel::from_i32(7), None);
        assert_eq!(LogLevel::from_i32(-1), None);
        assert_eq!(LogLevel::Information.as_str(), "Information");
    }

    fn record<'a>(category: &'a str, message: &'a str) -> LogRecord<'a> {
        LogRecord {
            category,
            message,
            exception: "",
            thread_name: "main",
            level: LogLevel::Information,
            tid: 7,
            // 2026-08-16 17:10:05.123 UTC
            unix_millis: 1_786_900_205_123,
        }
    }

    #[test]
    fn renders_the_shipped_file_format() {
        let tokens = compile_format("[%date% %time%][%level%][%logger%] %message%");
        let line = render(
            &tokens,
            &record("SPTarkov.Server.Core.Utils.App", "started"),
        );
        assert_eq!(
            line,
            "[2026-08-16 17:10:05.123][Information][SPTarkov.Server.Core.Utils.App] started"
        );
    }

    #[test]
    fn logger_short_takes_the_segment_after_the_last_dot() {
        let tokens = compile_format("%loggerShort%|%logger%");
        let line = render(&tokens, &record("A.B.C", "x"));
        assert_eq!(line, "C|A.B.C");

        let line = render(&tokens, &record("NoDots", "x"));
        assert_eq!(line, "NoDots|NoDots");
    }

    #[test]
    fn tokens_are_case_sensitive_and_unknown_text_passes_through() {
        // C#'s string.Replace is ordinal case-sensitive: %DATE% is not a token.
        let tokens = compile_format("%DATE% %tid% %tname% %notatoken%");
        let line = render(&tokens, &record("L", "x"));
        assert_eq!(line, "%DATE% 7 main %notatoken%");
    }

    #[test]
    fn exception_text_is_appended_after_the_formatted_line() {
        let tokens = compile_format("%message%");
        let mut r = record("L", "boom");
        r.exception = "kaput\n   at Frame";
        let line = render(&tokens, &r);
        assert_eq!(line, "boom\nkaput\n   at Frame");
    }

    #[test]
    fn empty_thread_name_renders_empty() {
        let tokens = compile_format("[%tname%]");
        let mut r = record("L", "x");
        r.thread_name = "";
        assert_eq!(render(&tokens, &r), "[]");
    }

    fn filter(
        filter_type: SptLoggerFilterType,
        name: &str,
        matching: MatchingType,
    ) -> CompiledFilter {
        CompiledFilter::compile(&SptLoggerFilter {
            filter_type,
            name: name.to_string(),
            matching_type: matching,
        })
    }

    #[test]
    fn literal_filters_match_the_whole_category_exactly() {
        let f = filter(SptLoggerFilterType::Include, "A.B", MatchingType::Literal);
        assert!(f.matches("A.B"));
        assert!(!f.matches("A.B.C"));
        assert!(!f.matches("a.b"));
    }

    #[test]
    fn regex_filters_search_like_dotnet_ismatch() {
        // .NET Regex.IsMatch is an unanchored search; ".*RequestLogger" hits anywhere.
        let f = filter(
            SptLoggerFilterType::Exclude,
            ".*RequestLogger",
            MatchingType::Regex,
        );
        assert!(f.matches("SPTarkov.Server.Core.Utils.Logger.RequestLogger"));
        assert!(!f.matches("SPTarkov.Server.Core.Utils.App"));
    }

    #[test]
    fn empty_names_and_empty_categories_never_match() {
        let f = filter(SptLoggerFilterType::Include, "", MatchingType::Literal);
        assert!(!f.matches("anything"));
        let f = filter(SptLoggerFilterType::Include, "x", MatchingType::Literal);
        assert!(!f.matches(""));
    }

    #[test]
    fn an_uncompilable_regex_never_matches() {
        // Lookarounds are .NET-only syntax that regex-lite rejects at compile time.
        let f = filter(
            SptLoggerFilterType::Include,
            "(?=peek)",
            MatchingType::Regex,
        );
        assert!(!f.matches("peek"));
    }

    #[test]
    fn should_emit_applies_exclude_then_include_gate_then_level() {
        let level = LogLevel::Information;
        let cat = "SPTarkov.Server.Core.Utils.App";

        // No filters: only the level decides.
        assert!(should_emit(level, &[], LogLevel::Warning, cat));
        assert!(!should_emit(level, &[], LogLevel::Debug, cat));

        // An exclude match wins over everything.
        let exclude = vec![filter(
            SptLoggerFilterType::Exclude,
            ".*App",
            MatchingType::Regex,
        )];
        assert!(!should_emit(level, &exclude, LogLevel::Error, cat));

        // Any include filter present gates: only matching categories pass.
        let include = vec![filter(
            SptLoggerFilterType::Include,
            ".*RequestLogger",
            MatchingType::Regex,
        )];
        assert!(!should_emit(level, &include, LogLevel::Error, cat));
        assert!(should_emit(
            level,
            &include,
            LogLevel::Error,
            "Web.RequestLogger"
        ));

        // Exclude beats include when both match.
        let both = vec![
            filter(SptLoggerFilterType::Include, ".*App", MatchingType::Regex),
            filter(SptLoggerFilterType::Exclude, ".*App", MatchingType::Regex),
        ];
        assert!(!should_emit(level, &both, LogLevel::Error, cat));
    }

    fn file_config(dir: &std::path::Path, level: &str, filters: &str) -> String {
        format!(
            r#"{{ "loggers": [ {{ "type": "File", "logLevel": "{level}",
                "format": "[%level%] %message%", "filePath": {path:?}, "filePattern": "test.log",
                "maxFileSizeMB": 10, "maxRollingFiles": 10, "filters": [{filters}] }} ] }}"#,
            path = dir.display().to_string(),
        )
    }

    #[test]
    fn emit_writes_matching_lines_and_drops_filtered_ones() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = file_config(
            dir.path(),
            "Information",
            r#"{ "type": "Exclude", "name": "Noise.*", "matchingType": "Regex" }"#,
        );
        let logger = Logger::from_json(config.as_bytes()).unwrap();

        logger.emit(&LogRecord {
            level: LogLevel::Information,
            ..record("App", "kept")
        });
        logger.emit(&LogRecord {
            level: LogLevel::Debug,
            ..record("App", "below level")
        });
        logger.emit(&LogRecord {
            level: LogLevel::Error,
            ..record("Noise.Chatter", "excluded")
        });
        logger.close();

        let contents = std::fs::read_to_string(dir.path().join("test.log")).unwrap();
        assert_eq!(contents, "[Information] kept\n");
    }

    #[test]
    fn a_bom_prefixed_lowercase_config_parses_like_the_pascal_case_one() {
        // C# reads the file with `JsonSerializer.Deserialize(Stream)`: it skips a UTF-8 BOM, and
        // its `JsonStringEnumConverter` matches these three enums case-insensitively.
        fn run(
            dir: &std::path::Path,
            level: &str,
            kind: &str,
            matching: &str,
            bom: bool,
        ) -> String {
            let config = file_config(
                dir,
                level,
                &format!(
                    r#"{{ "type": "{kind}", "name": "Noise.*", "matchingType": "{matching}" }}"#
                ),
            );
            let mut bytes: Vec<u8> = if bom {
                b"\xEF\xBB\xBF".to_vec()
            } else {
                Vec::new()
            };
            bytes.extend_from_slice(config.as_bytes());

            let logger = Logger::from_json(&bytes).unwrap();
            logger.emit(&record("App", "kept"));
            logger.emit(&LogRecord {
                level: LogLevel::Debug,
                ..record("App", "below level")
            });
            logger.emit(&LogRecord {
                level: LogLevel::Error,
                ..record("Noise.Chatter", "excluded")
            });
            logger.close();

            std::fs::read_to_string(dir.join("test.log")).unwrap()
        }

        let pascal_dir = tempfile::TempDir::new().unwrap();
        let lower_dir = tempfile::TempDir::new().unwrap();
        let pascal = run(pascal_dir.path(), "Information", "Exclude", "Regex", false);
        assert_eq!(pascal, "[Information] kept\n");
        assert_eq!(
            run(lower_dir.path(), "information", "exclude", "regex", true),
            pascal
        );

        // An unknown value still fails, naming the offending string…
        let bad = file_config(pascal_dir.path(), "Verbose", "");
        let Err(error) = Logger::from_json(bad.as_bytes()) else {
            panic!("an unknown log level must not parse");
        };
        assert!(error.contains("'Verbose'"), "unhelpful error: {error}");

        // …and the `type` tag stays case-sensitive, matching C#'s hand-written converter.
        let bad =
            file_config(pascal_dir.path(), "Information", "").replace(r#""File""#, r#""file""#);
        assert!(Logger::from_json(bad.as_bytes()).is_err());
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(Logger::from_json(b"not json").is_err());
    }

    #[test]
    fn an_unopenable_file_target_is_skipped_not_fatal() {
        let dir = tempfile::TempDir::new().unwrap();
        // A file where the log directory should be makes FileSink::open fail.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let config = file_config(&blocker.join("logs"), "Information", "");

        let logger = Logger::from_json(config.as_bytes()).unwrap();
        assert!(logger.entries.is_empty());
        logger.emit(&record("App", "goes nowhere, must not panic"));
        logger.close();
    }

    #[test]
    fn midnight_and_epoch_render_correctly() {
        let tokens = compile_format("%date% %time%");
        let mut r = record("L", "x");
        r.unix_millis = 0;
        assert_eq!(render(&tokens, &r), "1970-01-01 00:00:00.000");
    }

    use std::io::Write;
    use std::sync::{Arc, Mutex};

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn console_lines_arrive_in_order_and_close_flushes() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = ConsoleSink::open_with(Box::new(SharedWriter(Arc::clone(&lines)))).unwrap();

        for index in 0..100 {
            sink.write(format!("line {index}").into_bytes());
        }
        sink.close();

        let text = String::from_utf8(lines.lock().unwrap().clone()).unwrap();
        let expected: String = (0..100).map(|index| format!("line {index}\n")).collect();
        assert_eq!(text, expected);
    }

    /// Blocks its first write until the test opens the gate, signalling when the worker has
    /// entered it — so the test knows the queue behind the worker is empty before flooding it.
    struct GatedWriter {
        gate: std::sync::mpsc::Receiver<()>,
        entered: std::sync::mpsc::Sender<()>,
        lines: Arc<Mutex<Vec<u8>>>,
        opened: bool,
    }

    impl Write for GatedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if !self.opened {
                let _ = self.entered.send(());
                let _ = self.gate.recv();
                self.opened = true;
            }
            self.lines.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The roadmap's "console output is asynchronous and drops on a full queue" divergence,
    /// previously unpinned. If `try_send` ever became a blocking `send`, the flood below would
    /// hang the test instead of dropping the overflow.
    #[test]
    fn a_console_burst_deeper_than_the_queue_drops_instead_of_blocking() {
        let (gate_sender, gate_receiver) = std::sync::mpsc::channel();
        let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = ConsoleSink::open_with(Box::new(GatedWriter {
            gate: gate_receiver,
            entered: entered_sender,
            lines: Arc::clone(&lines),
            opened: false,
        }))
        .unwrap();

        // The worker takes this line off the queue and blocks writing it.
        sink.write(b"first".to_vec());
        entered_receiver.recv().unwrap();

        // The queue is empty and the worker is stuck: exactly CONSOLE_QUEUE_CAPACITY of these
        // fit, the last 101 drop.
        for index in 0..(CONSOLE_QUEUE_CAPACITY + 101) {
            sink.write(format!("line {index}").into_bytes());
        }

        gate_sender.send(()).unwrap();
        sink.close();

        let written = lines.lock().unwrap().clone();
        let count = written
            .split(|&byte| byte == b'\n')
            .filter(|piece| !piece.is_empty())
            .count();
        assert_eq!(
            count,
            1 + CONSOLE_QUEUE_CAPACITY,
            "the burst beyond the queue capacity must drop, not block"
        );
    }

    #[test]
    fn raw_bytes_interleave_in_queue_order_without_newline_synthesis() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = ConsoleSink::open_with(Box::new(SharedWriter(Arc::clone(&lines)))).unwrap();

        sink.write(b"one".to_vec());
        // The clone must not outlive this statement: a live sender keeps the channel connected,
        // so `close`'s join would wait on a `recv` that never disconnects.
        sink.sender
            .clone()
            .unwrap()
            .send(ConsoleMessage::Raw(b">>> ".to_vec()))
            .unwrap();
        sink.write(b"two".to_vec());
        sink.close();

        let text = String::from_utf8(lines.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "one\n>>> two\n");
    }

    #[test]
    fn flush_acks_only_after_everything_queued_before_it_was_written() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = ConsoleSink::open_with(Box::new(SharedWriter(Arc::clone(&lines)))).unwrap();

        for index in 0..100 {
            sink.write(format!("line {index}").into_bytes());
        }
        let (ack_sender, ack_receiver) = sync_channel::<()>(1);
        sink.sender
            .clone()
            .unwrap()
            .send(ConsoleMessage::Flush(ack_sender))
            .unwrap();
        ack_receiver.recv().unwrap();

        // The ack fired, so all 100 lines must already be in the writer — before close.
        let text = String::from_utf8(lines.lock().unwrap().clone()).unwrap();
        let expected: String = (0..100).map(|index| format!("line {index}\n")).collect();
        assert_eq!(text, expected);
        sink.close();
    }

    /// The inverse of the Line drop test: Raw uses a blocking send, so a burst deeper than the
    /// queue delivers everything once the writer drains — a prompt must never be dropped.
    #[test]
    fn a_raw_burst_deeper_than_the_queue_blocks_and_delivers_everything() {
        let (gate_sender, gate_receiver) = std::sync::mpsc::channel();
        let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = ConsoleSink::open_with(Box::new(GatedWriter {
            gate: gate_receiver,
            entered: entered_sender,
            lines: Arc::clone(&lines),
            opened: false,
        }))
        .unwrap();

        sink.write(b"first".to_vec());
        entered_receiver.recv().unwrap();

        let sender = sink.sender.clone().unwrap();
        let flood = std::thread::spawn(move || {
            for index in 0..(CONSOLE_QUEUE_CAPACITY + 101) {
                sender
                    .send(ConsoleMessage::Raw(format!("raw {index}\n").into_bytes()))
                    .unwrap();
            }
        });

        gate_sender.send(()).unwrap();
        flood.join().unwrap();
        sink.close();

        let written = lines.lock().unwrap().clone();
        let count = written
            .split(|&byte| byte == b'\n')
            .filter(|piece| !piece.is_empty())
            .count();
        assert_eq!(count, 1 + CONSOLE_QUEUE_CAPACITY + 101);
    }

    #[test]
    fn enabled_is_the_level_only_gate_across_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        // Two targets at different levels, each with an exclude filter that suppresses every
        // category — so both halves of the contract have something to fail against.
        let config = format!(
            r#"{{ "loggers": [
                {{ "type": "File", "logLevel": "Warning", "format": "%message%",
                   "filePath": {path:?}, "filePattern": "warning.log",
                   "filters": [{{ "type": "Exclude", "name": ".*", "matchingType": "Regex" }}] }},
                {{ "type": "File", "logLevel": "Error", "format": "%message%",
                   "filePath": {path:?}, "filePattern": "error.log",
                   "filters": [{{ "type": "Exclude", "name": ".*", "matchingType": "Regex" }}] }}
            ] }}"#,
            path = dir.path().display().to_string(),
        );
        let logger = Logger::from_json(config.as_bytes()).unwrap();
        // Both targets must have opened, or the assertions below go vacuous.
        assert_eq!(logger.entries.len(), 2);

        // `any`, not `all`: the Warning target admits Warning even though the Error one does not.
        assert!(logger.enabled(LogLevel::Warning));
        assert!(logger.enabled(LogLevel::Critical));
        // Below every target's level, so no entry admits it.
        assert!(!logger.enabled(LogLevel::Information));
        // Filters deliberately do not participate: exclude-everything would drop these categories
        // in `emit`, yet `enabled` still says yes. This mirrors the C# IsLogEnabled contract.
        logger.close();
    }
}
