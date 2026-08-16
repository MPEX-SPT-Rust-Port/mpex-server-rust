//! The C# logging pipeline behind `SPTLoggerDispatcher`: `SptLoggerConfiguration` parsing, filter
//! matching, format expansion, and the console/file sinks. C# builds an `SptLogMessage` and hands
//! it across `spt_log_emit`; everything downstream lives here.

use crate::log_sink::civil_from_days;
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
/// C# handlers passed unterminated lines to `spt_log_write`.
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

    #[test]
    fn midnight_and_epoch_render_correctly() {
        let tokens = compile_format("%date% %time%");
        let mut r = record("L", "x");
        r.unix_millis = 0;
        assert_eq!(render(&tokens, &r), "1970-01-01 00:00:00.000");
    }
}
