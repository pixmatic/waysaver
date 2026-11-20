//! `command` feature: run an external program while the screen is idle.
//!
//! One child per covered output. Each child gets its own process group, so
//! stopping it (SIGTERM to the group) also reaches anything it spawned, and a
//! parent-death signal, so it doesn't outlive wayvoid. Exits are noticed
//! through a pidfd per child, polled by the main event loop: no SIGCHLD
//! handler, no threads, no timers.
//!
//! The command is run directly (no shell). It is responsible for showing
//! itself fullscreen on its output, e.g. `mpv --fs --fs-screen-name=...`;
//! to use `$WAYVOID_OUTPUT` in its arguments, wrap it in `sh -c '...'`.
//!
//! This file holds the feature's CLI part (`Options`) and the process side
//! (`Runner`); the Wayland side is `wayland/theme.rs`. When the feature is
//! off, `command_off.rs` replaces this file with the same API, empty.

use std::{
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use rustix::process::{Pid, PidfdFlags, Signal};
use wayland_client::protocol::wl_output::WlOutput;

use crate::logging::{ERR, INFO, WARNING, log};

/// The feature is compiled in (the stub says `false`).
pub const ENABLED: bool = true;

// --- Command line -------------------------------------------------------------

/// With this feature a command must follow `<idle-seconds>`.
pub const TAKES_COMMAND: bool = true;

/// Appended to the usage synopsis.
pub const SYNOPSIS_TAIL: &str = " <command...>";

/// Explanation printed under the synopsis.
pub const DESCRIPTION: &[&str] = &[
    "",
    "<command...> runs while the screen is idle, one instance per monitor (with",
    "multimonitor, $WAYVOID_OUTPUT holds the monitor's name). `blank` means a",
    "plain black screen. Options must come before <idle-seconds>.",
];

/// `--help` lines for this feature's options.
pub const USAGE: &[&str] = &[
    "      --backdrop      black background below the command's window (hides the desktop while it starts)",
];

/// This feature's part of the configuration.
#[derive(Default)]
pub struct Options {
    /// Program and arguments, as given; `blank` is turned into an empty list
    /// by `validate`.
    argv: Vec<String>,
    /// `--backdrop`: a black surface below the command's window.
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
    pub fn set_command(&mut self, argv: Vec<String>) {
        self.argv = argv;
    }

    /// Checks the command once parsing is done.
    pub fn validate(&mut self) -> Result<(), String> {
        match self.argv.first().map(String::as_str) {
            None => Err("missing <command> (use `blank` for a black screen)".into()),
            Some("blank") if self.argv.len() == 1 => {
                self.argv.clear();
                Ok(())
            }
            Some("blank") => Err("`blank` takes no arguments".into()),
            Some(_) => Ok(()),
        }
    }

    /// The command to run, or `None` for `blank`.
    pub fn argv(&self) -> Option<&[String]> {
        (!self.argv.is_empty()).then_some(self.argv.as_slice())
    }
}

// --- Processes ----------------------------------------------------------------

/// A child that exits sooner than this after starting is not restarted:
/// it is most likely failing (bad arguments, missing file...). Measured from
/// timestamps taken at spawn and at exit; nothing waits on it.
const MIN_LIFETIME: Duration = Duration::from_secs(2);

/// A running (or terminating) child.
struct Running {
    process: Child,
    /// Becomes readable when the process exits.
    pidfd: OwnedFd,
    /// The output it was started for (`None` in single-monitor builds).
    target: Option<WlOutput>,
    started: Instant,
}

/// A child that exited on its own while the screen was still blanked.
pub struct Exited {
    pub target: Option<WlOutput>,
    /// Whether it lived long enough to be worth restarting.
    pub restart: bool,
}

pub struct Runner {
    argv: Vec<String>,
    /// Children currently showing the command.
    live: Vec<Running>,
    /// Children sent SIGTERM, kept until they exit so they can be reaped.
    dying: Vec<Running>,
}

impl Runner {
    pub fn new(argv: &[String]) -> Self {
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

    /// Starts the command for `target`. `name` (the output's name, if known)
    /// is exported as `WAYVOID_OUTPUT`. Failures are logged; returns whether
    /// the child is running.
    pub fn start(&mut self, target: Option<WlOutput>, name: Option<&str>) -> bool {
        let mut cmd = std::process::Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..]).stdin(Stdio::null());
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
        let process = match cmd.spawn() {
            Ok(process) => process,
            Err(e) => {
                log(ERR, &format!("command {:?}: {e}", self.argv[0]));
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
                let mut process = process;
                let _ = process.wait();
                return false;
            }
        };
        log(
            INFO,
            &format!(
                "command started on {} (pid {pid})",
                name.unwrap_or("the screen")
            ),
        );
        self.live.push(Running {
            process,
            pidfd,
            target,
            started: Instant::now(),
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
        // ESRCH (already gone) is fine: the pidfd will report the exit.
        let _ = rustix::process::kill_process_group(pid, Signal::TERM);
        self.dying.push(running);
    }

    /// Every child's pidfd, to be polled for readability.
    pub fn fds(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.live.iter().chain(&self.dying).map(|r| r.pidfd.as_fd())
    }

    /// Reaps the children that exited. Terminated ones are just dropped; live
    /// ones that exited on their own are logged and returned, so the caller
    /// can restart them or fall back to black.
    pub fn reap(&mut self) -> Vec<Exited> {
        self.dying
            .retain_mut(|r| !matches!(r.process.try_wait(), Ok(Some(_))));
        let mut exited = Vec::new();
        let mut i = 0;
        while i < self.live.len() {
            let Ok(Some(status)) = self.live[i].process.try_wait() else {
                i += 1;
                continue;
            };
            let running = self.live.swap_remove(i);
            let restart = running.started.elapsed() >= MIN_LIFETIME;
            if restart {
                log(INFO, &format!("command exited ({status}), restarting"));
            } else {
                log(
                    WARNING,
                    &format!("command exited too soon ({status}), not restarting"),
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
/// in `/proc`. Used to ignore inhibitors requested by our own command.
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
    use super::is_own_descendant;
    use crate::cli::test_util::{config, run};

    #[test]
    fn command_argument() {
        let cmd = |args: &[&str]| config(run(args)).map(|c| c.command.argv().map(<[_]>::to_vec));
        assert!(run(&["10"]).is_err(), "the command is mandatory");
        assert_eq!(cmd(&["10", "blank"]), Some(None));
        assert!(run(&["10", "blank", "x"]).is_err());
        // Everything after the seconds is the command, options included.
        let mpv = ["mpv", "--fs", "-h", "blank"].map(String::from).to_vec();
        assert_eq!(cmd(&["10", "mpv", "--fs", "-h", "blank"]), Some(Some(mpv)));
    }

    #[test]
    fn backdrop_flag() {
        let flag = |args: &[&str]| config(run(args)).map(|c| c.command.backdrop);
        assert_eq!(flag(&["10", "blank"]), Some(false));
        assert_eq!(flag(&["--backdrop", "10", "blank"]), Some(true));
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
