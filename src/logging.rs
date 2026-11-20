//! Minimal journal-aware logging to stderr.

use std::sync::OnceLock;

/// syslog priorities understood by journald as `<N>` line prefixes.
pub const ERR: u8 = 3;
pub const WARNING: u8 = 4;
pub const INFO: u8 = 6;

/// Writes one log line to stderr.
/// In the journal: `<N>msg`, so the priority is kept and filterable (`-p`).
/// In a terminal: `wayvoid: msg`.
pub fn log(priority: u8, msg: &str) {
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
