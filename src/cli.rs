//! Command-line parsing, by hand (no `clap`).
//!
//! Syntax: `wayvoid [OPTIONS] <idle-seconds>`. Optional features add their own
//! options here behind `#[cfg(feature = "...")]`: a field in `Config`, a match
//! arm in `parse` and a line in `USAGE_OPTIONS`. An option whose feature is not
//! compiled in is simply unknown, i.e. a usage error.

/// Settings for this run.
pub struct Config {
    /// Idle timeout in milliseconds, as ext-idle-notify expects.
    pub timeout_ms: u32,
}

/// What the command line asks for.
pub enum Command {
    Run(Config),
    /// `-h`/`--help`: print usage to stdout and exit successfully.
    Help,
    /// `-V`/`--version`: print the version to stdout and exit successfully.
    Version,
}

/// One line per option, in the order shown by `--help`.
/// Feature-gated options append their own line.
const USAGE_OPTIONS: &[&str] = &[
    "  -h, --help       print this help and exit",
    "  -V, --version    print the version and exit",
];

/// Full usage text, listing only the options compiled into this binary.
pub fn usage() -> String {
    let mut text = String::from("usage: wayvoid [OPTIONS] <idle-seconds>\n\noptions:\n");
    for line in USAGE_OPTIONS {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Parses `args` (without the program name). Errors are short messages meant
/// to be printed together with the usage text.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut timeout_ms = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            // Feature options go here, e.g.:
            // #[cfg(feature = "x")]
            // "--x" => x = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ if timeout_ms.is_some() => return Err(format!("unexpected argument: {arg}")),
            _ => timeout_ms = Some(parse_timeout_ms(&arg)?),
        }
    }
    let timeout_ms = timeout_ms.ok_or("missing <idle-seconds>")?;
    Ok(Command::Run(Config { timeout_ms }))
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

    fn timeout(args: &[&str]) -> Option<u32> {
        match run(args) {
            Ok(Command::Run(config)) => Some(config.timeout_ms),
            _ => None,
        }
    }

    #[test]
    fn timeout_in_milliseconds() {
        assert_eq!(timeout(&["300"]), Some(300_000));
        assert_eq!(timeout(&["4294967"]), Some(4_294_967_000));
    }

    #[test]
    fn invalid_timeouts() {
        for bad in ["0", "4294968", "abc", "1.5", ""] {
            assert!(run(&[bad]).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn missing_or_extra_arguments() {
        assert!(run(&[]).is_err());
        assert!(run(&["10", "20"]).is_err());
    }

    #[test]
    fn options() {
        assert!(matches!(run(&["--help"]), Ok(Command::Help)));
        assert!(matches!(run(&["10", "-h"]), Ok(Command::Help)));
        assert!(matches!(run(&["-V"]), Ok(Command::Version)));
        assert!(run(&["--foo", "10"]).is_err());
        assert!(run(&["-5"]).is_err());
    }
}
