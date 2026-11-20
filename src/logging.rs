//! Minimal journal-aware logging to stderr, filtered by a log level.
//!
//! Each message has a syslog priority; the log level (`-l`/`--log-level`)
//! decides which ones are written:
//!
//! | Level | Name      | Priorities written                           |
//! |-------|-----------|----------------------------------------------|
//! | 0     | `none`    | nothing                                      |
//! | 1     | `error`   | `ERR`                                        |
//! | 2     | `warning` | `ERR`, `WARNING` (the default)               |
//! | 3     | `info`    | `ERR`, `WARNING`, `INFO`                     |
//! | 4     | `debug`   | everything, themes' output included          |

use std::sync::{
    OnceLock,
    atomic::{AtomicU8, Ordering},
};

/// syslog priorities understood by journald as `<N>` line prefixes.
pub const ERR: u8 = 3;
pub const WARNING: u8 = 4;
pub const INFO: u8 = 6;
pub const DEBUG: u8 = 7;

/// Level names, indexed by level number.
const LEVELS: [&str; 5] = ["none", "error", "warning", "info", "debug"];

/// The default level: only problems.
pub const DEFAULT_LEVEL: u8 = 2;

/// The current level. A global, as logging is: set once at startup.
static LEVEL: AtomicU8 = AtomicU8::new(DEFAULT_LEVEL);

/// Parses a level given as a number (`0`-`4`) or a name (`warning`...).
pub fn parse_level(text: &str) -> Option<u8> {
    let by_number = text
        .parse::<u8>()
        .ok()
        .filter(|&n| (n as usize) < LEVELS.len());
    by_number.or_else(|| {
        LEVELS
            .iter()
            .position(|&name| name == text)
            .map(|n| n as u8)
    })
}

/// Sets the level for the rest of the run.
pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

/// Whether messages with `priority` are written at the current level.
pub fn enabled(priority: u8) -> bool {
    let needed = match priority {
        ..=ERR => 1,
        WARNING => 2,
        DEBUG.. => 4,
        _ => 3, // NOTICE and INFO
    };
    LEVEL.load(Ordering::Relaxed) >= needed
}

/// Writes one log line to stderr, if the level allows it.
/// In the journal: `<N>msg`, so the priority is kept and filterable (`-p`).
/// In a terminal: `wayvoid: msg`.
pub fn log(priority: u8, msg: &str) {
    if !enabled(priority) {
        return;
    }
    if stderr_is_journal() {
        eprintln!("<{priority}>{msg}");
    } else {
        eprintln!("wayvoid: {msg}");
    }
}

/// True when stderr is connected to the systemd journal.
/// systemd exports `JOURNAL_STREAM=<dev>:<inode>` for the stream it hands us;
/// comparing it with stderr avoids false positives from inherited variables.
fn stderr_is_journal() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let Ok(var) = std::env::var("JOURNAL_STREAM") else {
            return false;
        };
        let Ok(stat) = rustix::fs::fstat(std::io::stderr()) else {
            return false;
        };
        var == format!("{}:{}", stat.st_dev, stat.st_ino)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(parse_level("0"), Some(0));
        assert_eq!(parse_level("4"), Some(4));
        assert_eq!(parse_level("warning"), Some(2));
        assert_eq!(parse_level("debug"), Some(4));
        for bad in ["5", "-1", "verbose", ""] {
            assert_eq!(parse_level(bad), None, "{bad:?} should be rejected");
        }
    }
}
