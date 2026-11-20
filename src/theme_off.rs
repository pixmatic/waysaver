//! Stub of `theme.rs`, compiled when the `theme` feature is off: the same
//! API, doing nothing. See `theme.rs` for what each item means.
//!
//! Everything here is a constant or a no-op, so the compiler removes it.
#![allow(dead_code)]

pub const ENABLED: bool = false;
pub const TAKES_THEME: bool = false;
pub const SYNOPSIS_TAIL: &str = "";
pub const DESCRIPTION: &[&str] = &[];
pub const USAGE: &[&str] = &[];

#[derive(Default)]
pub struct Options {}

impl Options {
    pub fn option(
        &mut self,
        _arg: &str,
        _args: &mut dyn Iterator<Item = String>,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn set_theme(&mut self, _argv: Vec<String>) {}

    pub fn validate(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub fn is_own_descendant(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use crate::cli::test_util::run;

    #[test]
    fn backdrop_unknown_without_feature() {
        assert!(run(&["--backdrop", "10"]).is_err());
        assert!(run(&["--max-restarts", "3", "10"]).is_err());
    }
}
