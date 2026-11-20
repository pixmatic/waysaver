#!/bin/sh
# Pre-commit checks: formatting, then clippy and tests for every combination
# of Cargo features (none, each alone, all of them...). Needs cargo-hack.
set -eu
cd "$(dirname "$0")/.."

cargo fmt --check
cargo hack --feature-powerset clippy --all-targets -- -D warnings
cargo hack --feature-powerset test --quiet
