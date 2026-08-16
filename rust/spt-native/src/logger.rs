//! The C# logging pipeline behind `SPTLoggerDispatcher`: `SptLoggerConfiguration` parsing, filter
//! matching, format expansion, and the console/file sinks. C# builds an `SptLogMessage` and hands
//! it across `spt_log_emit`; everything downstream lives here.

use std::io::Write as _;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use crate::log_sink::{FileSink, civil_from_days};
use serde::Deserialize;

/// Mirrors Microsoft.Extensions.Logging.LogLevel: the declaration order is the numeric order the
/// C# side sends across the boundary, and `can_log` is `messageLevel >= loggerLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum SptLoggerFilterType {
    Exclude,
    Include,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum MatchingType {
    Literal,
    Regex,
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

/// Stdout twin of `FileSink`: a writer thread behind a bounded channel, so a blocked terminal
/// stalls the writer thread, not the logging call.
struct ConsoleSink {
    sender: Option<SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<()>>,
}

impl ConsoleSink {
    fn open() -> std::io::Result<ConsoleSink> {
        let (sender, receiver) = sync_channel::<Vec<u8>>(CONSOLE_QUEUE_CAPACITY);
        let worker = std::thread::Builder::new()
            .name("spt-log-console".to_owned())
            .spawn(move || console_run(&receiver))?;

        Ok(ConsoleSink {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn write(&self, line: Vec<u8>) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(line);
        }
    }

    fn close(mut self) {
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Drains bursts the same way the file writer does: block for one line, drain the backlog, one
/// flush per burst.
fn console_run(receiver: &Receiver<Vec<u8>>) {
    let stdout = std::io::stdout();

    while let Ok(first) = receiver.recv() {
        let mut out = stdout.lock();
        let _ = out.write_all(&first);
        let _ = out.write_all(b"\n");
        while let Ok(next) = receiver.try_recv() {
            let _ = out.write_all(&next);
            let _ = out.write_all(b"\n");
        }
        let _ = out.flush();
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
}
