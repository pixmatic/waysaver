//! wayvoid: a minimal Wayland screensaver.
//!
//! After N seconds of inactivity it covers the screen with black, and removes
//! it as soon as activity resumes. Idle time and inhibitors come from:
//! - `wayland`: the compositor (ext-idle-notify-v1, Wayland inhibitors).
//! - `inhibit`: D-Bus sources the compositor doesn't see (Firefox, systemd-inhibit).
//!
//! A single thread blocks in poll() on all sockets: 0% CPU while idle.

mod cli;
#[cfg(feature = "command")]
mod command;
mod inhibit;
mod logging;
mod wayland;

use std::{os::fd::BorrowedFd, process::ExitCode};

use rustix::event::{PollFd, PollFlags, poll};

use logging::{ERR, INFO, log};

fn main() -> ExitCode {
    let config = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Run(config)) => config,
        Ok(cli::Command::Help) => {
            print!("{}", cli::usage());
            return ExitCode::SUCCESS;
        }
        Ok(cli::Command::Version) => {
            println!("wayvoid {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            log(ERR, &e);
            eprint!("{}", cli::usage());
            // 2 is the conventional exit code for command-line usage errors.
            return ExitCode::from(2);
        }
    };
    match run(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log(ERR, &e);
            ExitCode::FAILURE
        }
    }
}

/// Sets everything up and runs the event loop. Only returns on fatal errors.
fn run(config: &cli::Config) -> Result<(), String> {
    // Wayland is mandatory; the D-Bus sources are best-effort.
    let (mut queue, mut state) = wayland::connect(config)?;
    let qh = queue.handle();
    let mut sources = inhibit::start();

    // Creates the idle notification unless a D-Bus inhibitor is already held.
    state.set_inhibited(sources.iter().any(|s| s.is_inhibited()), &qh);
    log(
        INFO,
        &format!("started, idle timeout {}s", config.timeout_ms / 1000),
    );

    // `ready[i]` says whether `sources[i]` had data at the last poll(). The
    // two vectors stay index-aligned: `ready` is rebuilt after every poll()
    // and cleared as soon as it has been used, before `sources` can shrink.
    // All start "ready" because the startup calls may have left messages
    // buffered inside libdbus, which poll() would never report.
    let mut ready = vec![true; sources.len()];
    loop {
        // 1. D-Bus: handle whatever arrived, then recompute the inhibition.
        if ready.contains(&true) {
            // Dispatch the ready sources, dropping any whose bus was lost
            // (`retain_mut` visits them in order, so `i` matches `ready`).
            let mut i = 0;
            sources.retain_mut(|src| {
                let keep = !ready[i]
                    || src
                        .dispatch()
                        .inspect_err(|e| inhibit::warn_disabled(src.name(), e))
                        .is_ok();
                i += 1;
                keep
            });
            ready.clear();
            // Idempotent: only acts when the overall state actually changes.
            state.set_inhibited(sources.iter().any(|s| s.is_inhibited()), &qh);
        }

        // 2. Wayland: requests are buffered client-side; flush them so the
        //    compositor sees them before we go to sleep.
        queue.flush().map_err(|e| format!("flush: {e}"))?;
        // wayland-client's race-free reading protocol: announce the intent to
        // read (`prepare_read`), poll, then `read` or drop the guard. It
        // returns None if events are already queued: dispatch those first.
        let Some(guard) = queue.prepare_read() else {
            wayland::dispatch(&mut queue, &mut state)?;
            continue;
        };
        // 3. Sleep until any fd is readable: Wayland, then the D-Bus sources,
        //    then (with `command`) the children's pidfds. This inner block
        //    exists so the fd borrows (from `guard`, `sources` and `state`) end
        //    before `guard` is consumed and `state` mutated below.
        let wl_ready;
        #[cfg(feature = "command")]
        let child_exited;
        {
            let wl_fd = guard.connection_fd();
            // SAFETY: each fd belongs to a source's channel, which outlives this block.
            let bus_fds: Vec<_> = sources
                .iter()
                .map(|src| unsafe { BorrowedFd::borrow_raw(src.fd()) })
                .collect();
            let mut fds = vec![PollFd::new(&wl_fd, PollFlags::IN)];
            fds.extend(bus_fds.iter().map(|fd| PollFd::new(fd, PollFlags::IN)));
            #[cfg(feature = "command")]
            let child_fds = state.command_fds();
            #[cfg(feature = "command")]
            fds.extend(child_fds.iter().map(|fd| PollFd::new(fd, PollFlags::IN)));
            // No timeout: we only wake up for events, never on a timer.
            match poll(&mut fds, None) {
                Ok(_) => {}
                // Interrupted by a signal: just poll again (the guard is dropped).
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(format!("poll: {e}")),
            }
            // Any revents (IN, HUP, ERR) means "go read": errors surface there.
            wl_ready = !fds[0].revents().is_empty();
            let bus_end = 1 + bus_fds.len();
            ready = fds[1..bus_end]
                .iter()
                .map(|fd| !fd.revents().is_empty())
                .collect();
            #[cfg(feature = "command")]
            {
                child_exited = fds[bus_end..].iter().any(|fd| !fd.revents().is_empty());
            }
        }

        // 4. Read Wayland events if there are any (dropping an unread guard
        //    cancels the read intent) and run their handlers.
        if wl_ready {
            wayland::read_events(guard)?;
        }
        wayland::dispatch(&mut queue, &mut state)?;

        // 5. `command`: reap exited children (restart them or go black).
        #[cfg(feature = "command")]
        if child_exited {
            state.command_exited();
        }
    }
}
