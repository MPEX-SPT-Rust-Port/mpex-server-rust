//! The C# logging pipeline behind `SPTLoggerDispatcher`: `SptLoggerConfiguration` parsing, filter
//! matching, format expansion, and the console/file sinks. C# builds an `SptLogMessage` and hands
//! it across `spt_log_emit`; everything downstream lives here.

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
}
