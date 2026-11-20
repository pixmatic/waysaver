//! Command-line parsing, by hand (no `clap`).
//!
//! Syntax: `wayvoid [OPTIONS] <idle-seconds>`. With the `command` feature it
//! becomes `wayvoid [OPTIONS] <idle-seconds> <command...>`: everything after
//! the seconds is the command (so wayvoid's options must come first, as with
//! `timeout`), and `blank` means the plain black screen.
//!
//! Optional features add their own options behind `#[cfg(feature = "...")]`:
//! a field in `Config`, a match arm in `parse` and a `#[cfg]`'d line in
//! `USAGE_OPTIONS`. An option whose feature is not compiled in is simply
//! unknown, i.e. a usage error.

/// Settings for this run.
pub struct Config {
    /// Idle timeout in milliseconds, as ext-idle-notify expects.
    pub timeout_ms: u32,
    /// `--ignore-mouse`: pointer motion doesn't unblank, only keys, buttons,
    /// scrolling and touch do.
    #[cfg(feature = "ignore-mouse")]
    pub ignore_mouse: bool,
    /// Program and arguments to run while blanked; `None` for `blank`.
    #[cfg(feature = "command")]
    pub command: Option<Vec<String>>,
    /// `--backdrop`: a black surface below the command's window.
    #[cfg(feature = "command")]
    pub backdrop: bool,
}

/// What the command line asks for.
pub enum Command {
    Run(Config),
    /// `-h`/`--help`: print usage to stdout and exit successfully.
    Help,
    /// `-V`/`--version`: print the version to stdout and exit successfully.
    Version,
}

/// Synopsis and description of the positional arguments.
#[cfg(not(feature = "command"))]
const USAGE_SYNOPSIS: &str = "usage: wayvoid [OPTIONS] <idle-seconds>";
#[cfg(feature = "command")]
const USAGE_SYNOPSIS: &str = "usage: wayvoid [OPTIONS] <idle-seconds> <command...>

<command...> runs while the screen is idle, one instance per monitor (with
multimonitor, $WAYVOID_OUTPUT holds the monitor's name). `blank` means a
plain black screen. Options must come before <idle-seconds>.";

/// One line per option, in the order shown by `--help`.
/// Feature-gated options append their own line.
const USAGE_OPTIONS: &[&str] = &[
    "  -h, --help          print this help and exit",
    "  -V, --version       print the version and exit",
    #[cfg(feature = "ignore-mouse")]
    "      --ignore-mouse  don't wake up on mouse motion (keys, buttons, scroll and touch still do)",
    #[cfg(feature = "command")]
    "      --backdrop      black background below the command's window (hides the desktop while it starts)",
];

/// Full usage text, listing only the options compiled into this binary.
pub fn usage() -> String {
    let mut text = format!("{USAGE_SYNOPSIS}\n\noptions:\n");
    for line in USAGE_OPTIONS {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Parses `args` (without the program name). Errors are short messages meant
/// to be printed together with the usage text.
// `while let` instead of `for`: with `command`, the loop body consumes the
// rest of the iterator itself.
#[cfg_attr(not(feature = "command"), allow(clippy::while_let_on_iterator))]
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut timeout_ms = None;
    #[cfg(feature = "ignore-mouse")]
    let mut ignore_mouse = false;
    #[cfg(feature = "command")]
    let mut backdrop = false;
    #[cfg(feature = "command")]
    let mut command = Vec::new();

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // `command`: after the seconds, everything (options included) belongs
        // to the command.
        #[cfg(feature = "command")]
        if timeout_ms.is_some() {
            command.push(arg);
            command.extend(args.by_ref());
            break;
        }
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            #[cfg(feature = "ignore-mouse")]
            "--ignore-mouse" => ignore_mouse = true,
            #[cfg(feature = "command")]
            "--backdrop" => backdrop = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ if timeout_ms.is_some() => return Err(format!("unexpected argument: {arg}")),
            _ => timeout_ms = Some(parse_timeout_ms(&arg)?),
        }
    }
    let timeout_ms = timeout_ms.ok_or("missing <idle-seconds>")?;

    #[cfg(feature = "command")]
    let command = match command.first().map(String::as_str) {
        None => return Err("missing <command> (use `blank` for a black screen)".into()),
        Some("blank") if command.len() == 1 => None,
        Some("blank") => return Err("`blank` takes no arguments".into()),
        Some(_) => Some(command),
    };

    Ok(Command::Run(Config {
        timeout_ms,
        #[cfg(feature = "ignore-mouse")]
        ignore_mouse,
        #[cfg(feature = "command")]
        command,
        #[cfg(feature = "command")]
        backdrop,
    }))
}

/// Seconds (> 0) to milliseconds. The upper bound comes from ext-idle-notify
/// taking a `u32` in milliseconds (about 49 days).
fn parse_timeout_ms(arg: &str) -> Result<u32, String> {
    arg.parse::<u32>()
        .ok()
        .filter(|&s| s > 0)
        .and_then(|s| s.checked_mul(1000))
        .ok_or_else(|| format!("invalid <idle-seconds>: {arg} (expected 1 to 4294967)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    /// `args` plus the mandatory `blank` command when `command` is compiled in.
    fn run_blank(args: &[&str]) -> Result<Command, String> {
        let mut args = args.to_vec();
        if cfg!(feature = "command") {
            args.push("blank");
        }
        run(&args)
    }

    fn config(result: Result<Command, String>) -> Option<Config> {
        match result {
            Ok(Command::Run(config)) => Some(config),
            _ => None,
        }
    }

    #[test]
    fn timeout_in_milliseconds() {
        let timeout = |s| config(run_blank(&[s])).map(|c| c.timeout_ms);
        assert_eq!(timeout("300"), Some(300_000));
        assert_eq!(timeout("4294967"), Some(4_294_967_000));
    }

    #[test]
    fn invalid_timeouts() {
        for bad in ["0", "4294968", "abc", "1.5", ""] {
            assert!(run_blank(&[bad]).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn missing_arguments() {
        assert!(run(&[]).is_err());
    }

    #[cfg(not(feature = "command"))]
    #[test]
    fn extra_arguments() {
        assert!(run(&["10", "20"]).is_err());
    }

    #[test]
    fn options() {
        assert!(matches!(run(&["--help"]), Ok(Command::Help)));
        assert!(matches!(run(&["-V"]), Ok(Command::Version)));
        assert!(run_blank(&["--foo", "10"]).is_err());
        assert!(run_blank(&["-5"]).is_err());
    }

    #[cfg(not(feature = "command"))]
    #[test]
    fn options_after_seconds() {
        assert!(matches!(run(&["10", "-h"]), Ok(Command::Help)));
    }

    #[cfg(feature = "ignore-mouse")]
    #[test]
    fn ignore_mouse_flag() {
        let flag = |args: &[&str]| config(run_blank(args)).map(|c| c.ignore_mouse);
        assert_eq!(flag(&["10"]), Some(false));
        assert_eq!(flag(&["--ignore-mouse", "10"]), Some(true));
    }

    #[cfg(not(feature = "ignore-mouse"))]
    #[test]
    fn ignore_mouse_unknown_without_feature() {
        assert!(run_blank(&["--ignore-mouse", "10"]).is_err());
    }

    #[cfg(feature = "command")]
    #[test]
    fn command_argument() {
        let cmd = |args: &[&str]| config(run(args)).map(|c| c.command);
        assert!(run(&["10"]).is_err(), "the command is mandatory");
        assert_eq!(cmd(&["10", "blank"]), Some(None));
        assert!(run(&["10", "blank", "x"]).is_err());
        // Everything after the seconds is the command, options included.
        let mpv = ["mpv", "--fs", "-h", "blank"].map(String::from).to_vec();
        assert_eq!(cmd(&["10", "mpv", "--fs", "-h", "blank"]), Some(Some(mpv)));
    }

    #[cfg(feature = "command")]
    #[test]
    fn backdrop_flag() {
        let flag = |args: &[&str]| config(run(args)).map(|c| c.backdrop);
        assert_eq!(flag(&["10", "blank"]), Some(false));
        assert_eq!(flag(&["--backdrop", "10", "blank"]), Some(true));
    }

    #[cfg(not(feature = "command"))]
    #[test]
    fn backdrop_unknown_without_feature() {
        assert!(run(&["--backdrop", "10"]).is_err());
    }
}
