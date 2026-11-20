//! `theme` feature: run an external program while the screen is idle.
//!
//! One child per covered output. Each child gets its own process group, so
//! stopping it (SIGTERM to the group) also reaches anything it spawned, and a
//! parent-death signal, so it doesn't outlive wayvoid. Exits are noticed
//! through a pidfd per child, polled by the main event loop: no SIGCHLD
//! handler, no threads, no timers.
//!
//! The children's output goes through pipes, polled the same way: at the
//! `debug` log level every line is logged; otherwise stdout is discarded and
//! only the last lines of stderr are kept, to explain a theme that fails.
//!
//! A theme is any program, run directly (no shell). It is responsible for
//! showing itself fullscreen on its output, e.g. `mpv --fs --fs-screen-name=...`;
//! to use `$WAYVOID_OUTPUT` in its arguments, wrap it in `sh -c '...'`.
//! A bare name is looked up first in the themes directory
//! (`$XDG_CONFIG_HOME/wayvoid/themes`, see `themes_dir`), then in `PATH`, so
//! users can keep their themes as small scripts and call them by name.
//!
//! This file holds the feature's CLI part (`Options`) and the process side
//! (`Runner`); the Wayland side is `wayland/theme.rs`. When the feature is
//! off, `theme_off.rs` replaces this file with the same API, empty.

use std::{
    collections::VecDeque,
    fs::File,
    io::{ErrorKind, Read},
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use rustix::process::{Pid, PidfdFlags, Signal};
use wayland_client::protocol::wl_output::WlOutput;

use crate::logging::{DEBUG, ERR, INFO, WARNING, enabled, log};

/// The feature is compiled in (the stub says `false`).
pub const ENABLED: bool = true;

// --- Command line -------------------------------------------------------------

/// With this feature a theme must follow `<idle-seconds>`.
pub const TAKES_THEME: bool = true;

/// Appended to the usage synopsis.
pub const SYNOPSIS_TAIL: &str = " <theme...>";

/// Explanation printed under the synopsis.
pub const DESCRIPTION: &[&str] = &[
    "",
    "<theme...> is a program (and its arguments) to run while the screen is idle,",
    "one instance per monitor (with multimonitor, $WAYVOID_OUTPUT holds the",
    "monitor's name). A bare name is looked up first in",
    "$XDG_CONFIG_HOME/wayvoid/themes (~/.config/wayvoid/themes), then in PATH.",
    "`blank` means a plain black screen. Options must come before <idle-seconds>.",
];

/// `--help` lines for this feature's options.
pub const USAGE: &[&str] = &[
    "      --backdrop      black background below the theme's window (hides the desktop while it starts)",
];

/// This feature's part of the configuration.
#[derive(Default)]
pub struct Options {
    /// Program and arguments, as given; `blank` is turned into an empty list
    /// by `validate`.
    argv: Vec<String>,
    /// `--backdrop`: a black surface below the theme's window.
    pub backdrop: bool,
}

impl Options {
    /// Handles one option before `<idle-seconds>`; `false` if not ours.
    pub fn option(&mut self, arg: &str) -> bool {
        let ours = arg == "--backdrop";
        self.backdrop |= ours;
        ours
    }

    /// Takes everything that followed `<idle-seconds>`.
    pub fn set_theme(&mut self, argv: Vec<String>) {
        self.argv = argv;
    }

    /// Checks the theme once parsing is done.
    pub fn validate(&mut self) -> Result<(), String> {
        match self.argv.first().map(String::as_str) {
            None => Err("missing <theme> (use `blank` for a black screen)".into()),
            Some("blank") if self.argv.len() == 1 => {
                self.argv.clear();
                Ok(())
            }
            Some("blank") => Err("`blank` takes no arguments".into()),
            Some(_) => {
                if let Some(path) = find_theme(themes_dir().as_deref(), &self.argv[0]) {
                    self.argv[0] = path;
                }
                Ok(())
            }
        }
    }

    /// The theme to run, or `None` for `blank`.
    pub fn argv(&self) -> Option<&[String]> {
        (!self.argv.is_empty()).then_some(self.argv.as_slice())
    }
}

/// The user's themes directory: `$XDG_CONFIG_HOME/wayvoid/themes`, or
/// `~/.config/wayvoid/themes` when that variable is unset or empty.
fn themes_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))?;
    Some(config.join("wayvoid/themes"))
}

/// The path of theme `name` in `dir`, if `name` is a bare name (no `/`) and
/// such a file exists there. Otherwise the name is left for `PATH` lookup
/// (or used as is, if it is a path).
fn find_theme(dir: Option<&Path>, name: &str) -> Option<String> {
    if name.contains('/') {
        return None;
    }
    let path = dir?.join(name);
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

// --- Processes ----------------------------------------------------------------

/// A child that exits sooner than this after starting is not restarted:
/// it is most likely failing (bad arguments, missing file...). Measured from
/// timestamps taken at spawn and at exit; nothing waits on it.
const MIN_LIFETIME: Duration = Duration::from_secs(2);

/// Longest output line kept whole; longer ones are split.
const MAX_LINE: usize = 4096;
/// How many of stderr's last lines are kept to explain a failure.
const TAIL_LINES: usize = 3;

/// One of a child's output streams, read line by line without blocking.
struct Stream {
    file: File,
    /// The end of the last read, still waiting for its newline.
    partial: Vec<u8>,
    /// False once every writer closed it (the child and anything it spawned).
    open: bool,
}

impl Stream {
    /// Takes the read end of a pipe, made non-blocking for the event loop.
    fn new(fd: OwnedFd) -> Self {
        // A blocking read could hang the whole daemon: if O_NONBLOCK can't
        // be set, the stream is ignored (treated as closed) instead.
        let open = rustix::fs::fcntl_getfl(&fd)
            .and_then(|flags| rustix::fs::fcntl_setfl(&fd, flags | rustix::fs::OFlags::NONBLOCK))
            .is_ok();
        Self {
            file: File::from(fd),
            partial: Vec::new(),
            open,
        }
    }

    /// Reads what is available and returns the complete lines.
    fn read_lines(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut buf = [0; 4096];
        // Bounded, so a chatty child can't starve the event loop: poll()
        // reports the rest right away.
        for _ in 0..16 {
            if !self.open {
                break;
            }
            match self.file.read(&mut buf) {
                Ok(0) => self.open = false,
                Ok(n) => self.partial.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => self.open = false,
            }
            while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.partial.drain(..=end).collect();
                lines.push(text(&line[..end]));
            }
            if self.partial.len() >= MAX_LINE {
                lines.push(text(&std::mem::take(&mut self.partial)));
            }
        }
        // The last line may lack its newline.
        if !self.open && !self.partial.is_empty() {
            lines.push(text(&std::mem::take(&mut self.partial)));
        }
        lines
    }
}

/// Output bytes as a log-friendly line.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\r')
        .to_string()
}

/// A running (or terminating) child.
struct Running {
    process: Child,
    /// Becomes readable when the process exits.
    pidfd: OwnedFd,
    /// The output it was started for (`None` in single-monitor builds).
    target: Option<WlOutput>,
    /// "theme on HDMI-A-1", or just "theme": how the log refers to it.
    label: String,
    started: Instant,
    /// Its stdout, only read at the `debug` level (discarded otherwise).
    stdout: Option<Stream>,
    stderr: Option<Stream>,
    /// stderr's last lines, to explain a failure.
    tail: VecDeque<String>,
}

impl Running {
    /// Reads whatever the child wrote: each line is logged at the `debug`
    /// level, and stderr's last lines are kept.
    fn read_output(&mut self) {
        if let Some(stdout) = &mut self.stdout {
            for line in stdout.read_lines() {
                log(DEBUG, &format!("{} (stdout): {line}", self.label));
            }
        }
        if let Some(stderr) = &mut self.stderr {
            for line in stderr.read_lines() {
                log(DEBUG, &format!("{} (stderr): {line}", self.label));
                if !line.trim().is_empty() {
                    if self.tail.len() == TAIL_LINES {
                        self.tail.pop_front();
                    }
                    self.tail.push_back(line.chars().take(200).collect());
                }
            }
        }
    }

    /// The fds to poll: the pidfd, and the output pipes still open.
    fn fds(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        let pipes = [&self.stdout, &self.stderr].into_iter().flatten();
        std::iter::once(self.pidfd.as_fd()).chain(pipes.filter(|s| s.open).map(|s| s.file.as_fd()))
    }
}

/// A child that exited on its own while the screen was still blanked.
pub struct Exited {
    pub target: Option<WlOutput>,
    /// Whether it lived long enough to be worth restarting.
    pub restart: bool,
}

pub struct Runner {
    argv: Vec<String>,
    /// Children currently showing the theme.
    live: Vec<Running>,
    /// Children sent SIGTERM, kept until they exit so they can be reaped.
    dying: Vec<Running>,
}

impl Runner {
    pub fn new(argv: &[String]) -> Self {
        log(DEBUG, &format!("theme: {argv:?}"));
        Self {
            argv: argv.to_vec(),
            live: Vec::new(),
            dying: Vec::new(),
        }
    }

    /// Whether a child is currently running for `target`.
    pub fn is_running(&self, target: Option<&WlOutput>) -> bool {
        self.live.iter().any(|r| r.target.as_ref() == target)
    }

    /// Starts the theme for `target`. `name` (the output's name, if known)
    /// is exported as `WAYVOID_OUTPUT`. Failures are logged; returns whether
    /// the child is running.
    pub fn start(&mut self, target: Option<WlOutput>, name: Option<&str>) -> bool {
        let mut cmd = std::process::Command::new(&self.argv[0]);
        // stderr is always read (its last lines explain failures); stdout
        // only when it will be logged.
        let stdout = if enabled(DEBUG) {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        cmd.args(&self.argv[1..])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped());
        if let Some(name) = name {
            cmd.env("WAYVOID_OUTPUT", name);
        }
        // Own process group: SIGTERM to the group reaches grandchildren too.
        cmd.process_group(0);
        // SAFETY: the closure runs between fork and exec and only makes one
        // raw syscall (prctl), which is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(Signal::TERM))
                    .map_err(Into::into)
            });
        }
        let mut process = match cmd.spawn() {
            Ok(process) => process,
            Err(e) => {
                log(ERR, &format!("theme {:?}: {e}", self.argv[0]));
                return false;
            }
        };
        let pid = Pid::from_child(&process);
        let pidfd = match rustix::process::pidfd_open(pid, PidfdFlags::empty()) {
            Ok(fd) => fd,
            Err(e) => {
                // Without a pidfd we can't notice its exit: don't keep it.
                log(ERR, &format!("pidfd_open: {e}"));
                let _ = rustix::process::kill_process_group(pid, Signal::KILL);
                let _ = process.wait();
                return false;
            }
        };
        let label = match name {
            Some(name) => format!("theme on {name}"),
            None => "theme".to_string(),
        };
        log(INFO, &format!("{label} started (pid {pid})"));
        let stdout = process.stdout.take().map(|s| Stream::new(s.into()));
        let stderr = process.stderr.take().map(|s| Stream::new(s.into()));
        self.live.push(Running {
            process,
            pidfd,
            target,
            label,
            started: Instant::now(),
            stdout,
            stderr,
            tail: VecDeque::new(),
        });
        true
    }

    /// Sends SIGTERM to every live child; they are reaped as they exit.
    pub fn stop_all(&mut self) {
        while let Some(running) = self.live.pop() {
            self.terminate(running);
        }
    }

    /// Sends SIGTERM to the child of `target` (its output was unplugged).
    #[cfg_attr(not(feature = "multimonitor"), allow(dead_code))]
    pub fn stop(&mut self, target: &WlOutput) {
        while let Some(i) = self
            .live
            .iter()
            .position(|r| r.target.as_ref() == Some(target))
        {
            let running = self.live.swap_remove(i);
            self.terminate(running);
        }
    }

    fn terminate(&mut self, running: Running) {
        let pid = Pid::from_child(&running.process);
        log(
            DEBUG,
            &format!("{}: SIGTERM to process group {pid}", running.label),
        );
        // ESRCH (already gone) is fine: the pidfd will report the exit.
        let _ = rustix::process::kill_process_group(pid, Signal::TERM);
        self.dying.push(running);
    }

    /// The fds to poll: every child's pidfd and open output pipes.
    pub fn fds(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.live.iter().chain(&self.dying).flat_map(Running::fds)
    }

    /// Reads the children's output, then reaps the ones that exited.
    /// Terminated ones are just dropped; live ones that exited on their own
    /// are logged and returned, so the caller can restart them or fall back
    /// to black.
    pub fn reap(&mut self) -> Vec<Exited> {
        for running in self.live.iter_mut().chain(&mut self.dying) {
            running.read_output();
        }
        self.dying.retain_mut(|r| match r.process.try_wait() {
            Ok(Some(status)) => {
                log(DEBUG, &format!("{} ended ({status})", r.label));
                false
            }
            _ => true,
        });
        let mut exited = Vec::new();
        let mut i = 0;
        while i < self.live.len() {
            let Ok(Some(status)) = self.live[i].process.try_wait() else {
                i += 1;
                continue;
            };
            let running = self.live.swap_remove(i);
            let label = &running.label;
            let restart = running.started.elapsed() >= MIN_LIFETIME;
            if restart {
                log(INFO, &format!("{label} exited ({status}), restarting"));
            } else {
                // Its last words usually say why (e.g. a bad option).
                let said = Vec::from(running.tail.clone()).join(" | ");
                let said = if said.is_empty() {
                    String::new()
                } else {
                    format!(": {said}")
                };
                log(
                    WARNING,
                    &format!("{label} exited too soon ({status}), not restarting{said}"),
                );
            }
            exited.push(Exited {
                target: running.target,
                restart,
            });
        }
        exited
    }
}

/// Don't leave children behind if wayvoid returns (fatal error): the
/// parent-death signal covers being killed, this covers a normal exit.
impl Drop for Runner {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Whether `pid` is a descendant of wayvoid, by walking up its parent chain
/// in `/proc`. Used to ignore inhibitors requested by our own theme.
/// A process that detached itself (double fork) is no longer recognized.
pub fn is_own_descendant(pid: u32) -> bool {
    let me = std::process::id();
    let mut pid = pid;
    // Bounded walk: the chain always ends at PID 1, this just guards loops.
    for _ in 0..64 {
        if pid <= 1 {
            return false;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // "pid (comm) state ppid ...": comm may contain spaces and
        // parentheses, so parse after the last ')'.
        let Some(ppid) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse().ok())
        else {
            return false;
        };
        if ppid == me {
            return true;
        }
        pid = ppid;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{find_theme, is_own_descendant};
    use crate::cli::test_util::{config, run};

    #[test]
    fn theme_argument() {
        let cmd = |args: &[&str]| config(run(args)).map(|c| c.theme.argv().map(<[_]>::to_vec));
        assert!(run(&["10"]).is_err(), "the theme is mandatory");
        assert_eq!(cmd(&["10", "blank"]), Some(None));
        assert!(run(&["10", "blank", "x"]).is_err());
        // Everything after the seconds is the theme, options included.
        let mpv = ["mpv", "--fs", "-h", "blank"].map(String::from).to_vec();
        assert_eq!(cmd(&["10", "mpv", "--fs", "-h", "blank"]), Some(Some(mpv)));
    }

    #[test]
    fn backdrop_flag() {
        let flag = |args: &[&str]| config(run(args)).map(|c| c.theme.backdrop);
        assert_eq!(flag(&["10", "blank"]), Some(false));
        assert_eq!(flag(&["--backdrop", "10", "blank"]), Some(true));
    }

    #[test]
    fn themes_directory() {
        let dir = std::env::temp_dir().join(format!("wayvoid-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("matrix"), "#!/bin/sh\n").unwrap();
        let found = dir.join("matrix").to_string_lossy().into_owned();
        assert_eq!(find_theme(Some(&dir), "matrix"), Some(found));
        // Not in the directory: left for PATH.
        assert_eq!(find_theme(Some(&dir), "mpv"), None);
        // Paths are used as given, never looked up.
        assert_eq!(find_theme(Some(&dir), "./matrix"), None);
        assert_eq!(find_theme(None, "matrix"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn descendants() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        assert!(is_own_descendant(child.id()));
        let _ = child.kill();
        let _ = child.wait();
        assert!(!is_own_descendant(1));
        // Our parent (cargo's test runner) is an ancestor, not a descendant.
        assert!(!is_own_descendant(
            rustix::process::getppid().unwrap().as_raw_pid() as u32
        ));
    }
}
