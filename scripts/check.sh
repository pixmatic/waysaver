#!/bin/sh
# Pre-commit checks: formatting, then clippy and tests for every combination
# of Cargo features (none, each alone, all of them...). `default` is empty,
# so it is excluded to avoid checking every combination twice. Needs cargo-hack.
set -eu
cd "$(dirname "$0")/.."

cargo fmt --check
cargo hack --feature-powerset --exclude-features default clippy --all-targets -- -D warnings
cargo hack --feature-powerset --exclude-features default test --quiet
