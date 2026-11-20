//! wayvoid: a minimal Wayland screensaver.
//!
//! After N seconds of inactivity it covers the screen with black, and removes
//! it as soon as activity resumes. Idle time and inhibitors come from:
//! - `wayland`: the compositor (ext-idle-notify-v1, Wayland inhibitors).
//! - `inhibit`: D-Bus sources the compositor doesn't see (Firefox, systemd-inhibit).
//!
//! A single thread blocks in poll() on all sockets: 0% CPU while idle.

mod inhibit;
mod logging;
mod wayland;

use std::{os::fd::BorrowedFd, process::ExitCode};

use rustix::event::{PollFd, PollFlags, poll};

use logging::{ERR, INFO, log};

fn main() -> ExitCode {
    let Some(timeout_ms) = parse_timeout_ms() else {
        log(ERR, "usage: wayvoid <idle-seconds>");
        // 2 is the conventional exit code for command-line usage errors.
        return ExitCode::from(2);
    };
    match run(timeout_ms) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log(ERR, &e);
            ExitCode::FAILURE
        }
    }
}

/// Parses the single required argument: idle timeout in seconds (> 0).
/// Returns it in milliseconds, as ext-idle-notify expects. `None` (usage
/// error) if the argument is missing, repeated, not a number, zero, or too
/// large to fit in milliseconds as a `u32` (over ~49 days).
fn parse_timeout_ms() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    match (args.next(), args.next()) {
        (Some(arg), None) => arg
            .parse::<u32>()
            .ok()
            .filter(|&s| s > 0)
            .and_then(|s| s.checked_mul(1000)),
        _ => None,
    }
}

/// Sets everything up and runs the event loop. Only returns on fatal errors.
fn run(timeout_ms: u32) -> Result<(), String> {
    // Wayland is mandatory; the D-Bus sources are best-effort.
    let (mut queue, mut state) = wayland::connect(timeout_ms)?;
    let qh = queue.handle();
    let mut sources = inhibit::start();

    // Creates the idle notification unless a D-Bus inhibitor is already held.
    state.set_inhibited(sources.iter().any(|s| s.is_inhibited()), &qh);
    log(
        INFO,
        &format!("started, idle timeout {}s", timeout_ms / 1000),
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
        // 3. Sleep until any socket is readable. This inner block exists so the
        //    fd borrows (from `guard` and `sources`) end before `guard` is
        //    consumed by `read_events` below.
        let wl_ready;
        {
            let wl_fd = guard.connection_fd();
            // SAFETY: each fd belongs to a source's channel, which outlives this block.
            let bus_fds: Vec<_> = sources
                .iter()
                .map(|src| unsafe { BorrowedFd::borrow_raw(src.fd()) })
                .collect();
            let mut fds = vec![PollFd::new(&wl_fd, PollFlags::IN)];
            fds.extend(bus_fds.iter().map(|fd| PollFd::new(fd, PollFlags::IN)));
            // No timeout: we only wake up for events, never on a timer.
            match poll(&mut fds, None) {
                Ok(_) => {}
                // Interrupted by a signal: just poll again (the guard is dropped).
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(format!("poll: {e}")),
            }
            // Any revents (IN, HUP, ERR) means "go read": errors surface there.
            wl_ready = !fds[0].revents().is_empty();
            ready = fds[1..].iter().map(|fd| !fd.revents().is_empty()).collect();
        }

        // 4. Read Wayland events if there are any (dropping an unread guard
        //    cancels the read intent) and run their handlers.
        if wl_ready {
            wayland::read_events(guard)?;
        }
        wayland::dispatch(&mut queue, &mut state)?;
    }
}
