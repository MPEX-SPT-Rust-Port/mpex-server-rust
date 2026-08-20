//! Terminal control for the Rust-owned console: Windows console setup, title, clear, and the
//! stdin line-ending strip. Escape sequences are returned as bytes rather than written here, so
//! the FFI layer routes them through the console sink's queue — a direct write could land in the
//! middle of a log line the writer thread is emitting.

use std::io::IsTerminal;

#[cfg(windows)]
mod win {
    pub const CP_UTF8: u32 = 65001;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    // Hand-declared instead of a windows-sys dependency: five stable kernel32 signatures.
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        pub fn SetConsoleOutputCP(code_page_id: u32) -> i32;
        pub fn GetStdHandle(std_handle: u32) -> *mut core::ffi::c_void;
        pub fn GetConsoleMode(handle: *mut core::ffi::c_void, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: *mut core::ffi::c_void, mode: u32) -> i32;
        pub fn SetConsoleTitleW(title: *const u16) -> i32;
    }
}

/// Windows: UTF-8 output codepage (replacing C#'s `Console.OutputEncoding = UTF8`) and virtual
/// terminal processing so the clear/title escapes below work. Idempotent; unix needs nothing.
pub fn init_terminal() {
    #[cfg(windows)]
    unsafe {
        win::SetConsoleOutputCP(win::CP_UTF8);
        let handle = win::GetStdHandle(win::STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if win::GetConsoleMode(handle, &mut mode) != 0 {
            win::SetConsoleMode(handle, mode | win::ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

/// Replaces C#'s `Console.Title`. The title comes from our own C# (the version watermark), so it
/// is trusted not to contain escape bytes.
pub fn set_title(title: &str) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            win::SetConsoleTitleW(wide.as_ptr());
        }
        None
    }
    #[cfg(not(windows))]
    {
        if std::io::stdout().is_terminal() {
            Some(format!("\x1b]0;{title}\x07").into_bytes())
        } else {
            None
        }
    }
}

/// Replaces C#'s `Console.Clear()`, including its `IsOutputRedirected` guard: no tty, no bytes.
pub fn clear() -> Option<Vec<u8>> {
    if std::io::stdout().is_terminal() {
        Some(b"\x1b[2J\x1b[H".to_vec())
    } else {
        None
    }
}

/// `std::io::Stdin::read_line` keeps the terminator; C#'s `Console.ReadLine` strips it.
pub fn strip_line_ending(mut line: String) -> String {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_line_ending_removes_one_trailing_newline_of_either_kind() {
        assert_eq!(strip_line_ending("hello\n".to_owned()), "hello");
        assert_eq!(strip_line_ending("hello\r\n".to_owned()), "hello");
        assert_eq!(strip_line_ending("hello".to_owned()), "hello");
        assert_eq!(strip_line_ending("\n".to_owned()), "");
        assert_eq!(strip_line_ending(String::new()), "");
        // Only the terminator goes; interior and doubled newlines survive.
        assert_eq!(strip_line_ending("a\nb\n".to_owned()), "a\nb");
        assert_eq!(strip_line_ending("hello\n\n".to_owned()), "hello\n");
    }

    /// Environment-independent: libtest captures output in-process without redirecting fd 1, so
    /// whether stdout is a tty depends on how cargo test was launched. Assert only the escape
    /// bytes when the gate opens; the None-when-redirected behavior is `is_terminal` itself.
    #[test]
    fn clear_and_title_emit_the_expected_escape_bytes_when_a_tty_is_present() {
        if let Some(bytes) = clear() {
            assert_eq!(bytes, b"\x1b[2J\x1b[H");
        }
        if let Some(bytes) = set_title("T") {
            assert_eq!(bytes, b"\x1b]0;T\x07");
        }
    }
}
