//! Command-line parsing, by hand (no `clap`).
//!
//! Base syntax: `wayvoid [OPTIONS] <idle-seconds>`. Optional features plug in
//! through their facades (see "Module layout" in main.rs), so this file has no
//! `#[cfg]`:
//! - `ignore-mouse` (`wayland::input::Options`): the `--ignore-mouse` flag.
//! - `theme` (`theme::Options`): `--backdrop` and the mandatory
//!   `<theme...>` after the seconds.
//!
//! A feature that is not compiled in has a stub with no options, so its
//! flags are simply unknown, i.e. a usage error.

use crate::{logging, theme, wayland::input};

/// Settings for this run.
pub struct Config {
    /// Idle timeout in milliseconds, as ext-idle-notify expects.
    pub timeout_ms: u32,
    /// `-l`/`--log-level`: 0 (none) to 4 (debug), see logging.rs.
    pub log_level: u8,
    // Feature options: empty structs (never read) when the feature is off.
    #[allow(dead_code)]
    pub ignore_mouse: input::Options,
    #[allow(dead_code)]
    pub theme: theme::Options,
}

/// What the command line asks for.
pub enum Command {
    Run(Config),
    /// `-h`/`--help`: print usage to stdout and exit successfully.
    Help,
    /// `-V`/`--version`: print the version to stdout and exit successfully.
    Version,
}

/// Options every build has, in `--help` format.
const BASE_OPTIONS: &[&str] = &[
    "  -h, --help          print this help and exit",
    "  -V, --version       print the version and exit",
    "  -l, --log-level LEVEL",
    "                      what to log: 0/none, 1/error, 2/warning (default),",
    "                      3/info or 4/debug (everything, themes' output included)",
];

/// Full usage text, listing only what is compiled into this binary.
pub fn usage() -> String {
    let mut text = format!(
        "usage: wayvoid [OPTIONS] <idle-seconds>{}\n",
        theme::SYNOPSIS_TAIL
    );
    for line in theme::DESCRIPTION {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("\noptions:\n");
    for line in [BASE_OPTIONS, input::USAGE, theme::USAGE].concat() {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Parses `args` (without the program name). Errors are short messages meant
/// to be printed together with the usage text.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut timeout_ms = None;
    let mut log_level = logging::DEFAULT_LEVEL;
    let mut ignore_mouse = input::Options::default();
    let mut theme = theme::Options::default();

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // With `theme`, everything after the seconds (options included)
        // belongs to the theme, as with `timeout`.
        if timeout_ms.is_some() && theme::TAKES_THEME {
            theme.set_theme(std::iter::once(arg).chain(args.by_ref()).collect());
            break;
        }
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "-l" | "--log-level" => {
                let value = args.next().ok_or(format!("{arg} needs a level"))?;
                log_level = parse_log_level(&value)?;
            }
            _ if arg.starts_with("--log-level=") => {
                log_level = parse_log_level(&arg["--log-level=".len()..])?;
            }
            _ if ignore_mouse.option(&arg) || theme.option(&arg) => {}
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ if timeout_ms.is_some() => return Err(format!("unexpected argument: {arg}")),
            _ => timeout_ms = Some(parse_timeout_ms(&arg)?),
        }
    }
    let timeout_ms = timeout_ms.ok_or("missing <idle-seconds>")?;
    theme.validate()?;
    Ok(Command::Run(Config {
        timeout_ms,
        log_level,
        ignore_mouse,
        theme,
    }))
}

/// A `--log-level` value, or the usage error explaining the valid ones.
fn parse_log_level(value: &str) -> Result<u8, String> {
    logging::parse_level(value).ok_or_else(|| {
        format!("invalid log level: {value} (expected 0-4 or none, error, warning, info, debug)")
    })
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

/// Helpers shared by the tests here and in the feature modules.
#[cfg(test)]
pub mod test_util {
    use super::*;

    pub fn run(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    /// `args` plus the `blank` theme when `theme` requires one.
    pub fn run_blank(args: &[&str]) -> Result<Command, String> {
        let mut args = args.to_vec();
        if theme::TAKES_THEME {
            args.push("blank");
        }
        run(&args)
    }

    pub fn config(result: Result<Command, String>) -> Option<Config> {
        match result {
            Ok(Command::Run(config)) => Some(config),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, test_util::*};
    use crate::theme;

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
    fn missing_and_extra_arguments() {
        assert!(run(&[]).is_err());
        // Without `theme`, nothing may follow the seconds.
        if !theme::TAKES_THEME {
            assert!(run(&["10", "20"]).is_err());
            assert!(matches!(run(&["10", "-h"]), Ok(Command::Help)));
        }
    }

    #[test]
    fn log_level() {
        let level = |args: &[&str]| config(run_blank(args)).map(|c| c.log_level);
        assert_eq!(level(&["10"]), Some(2), "warning by default");
        assert_eq!(level(&["-l", "4", "10"]), Some(4));
        assert_eq!(level(&["--log-level", "info", "10"]), Some(3));
        assert_eq!(level(&["--log-level=none", "10"]), Some(0));
        assert!(run_blank(&["-l", "9", "10"]).is_err());
        assert!(run(&["-l"]).is_err(), "a level is required");
    }

    #[test]
    fn options() {
        assert!(matches!(run(&["--help"]), Ok(Command::Help)));
        assert!(matches!(run(&["-V"]), Ok(Command::Version)));
        assert!(run_blank(&["--foo", "10"]).is_err());
        assert!(run_blank(&["-5"]).is_err());
    }
}
