#!/bin/sh
# Runs waysaver inside a nested labwc, so it can be tested without blanking the
# real screen: only input inside the nested windows counts as activity.
#
# Usage: [OUTPUTS=n] [FEATURES="a b"] scripts/nested.sh <waysaver args...>
#   OUTPUTS   monitors of the nested compositor, one window each (default 1)
#   FEATURES  Cargo features to build with (default: none)
# Example: OUTPUTS=2 FEATURES=multimonitor scripts/nested.sh 5
#
# Inside the nested session, F1 adds a virtual (invisible) monitor and F2
# removes one. waysaver's log goes to this terminal. Needs labwc >= 0.7 and
# dbus-run-session.
set -eu
cd "$(dirname "$0")/.."

[ $# -gt 0 ] || { echo "usage: [OUTPUTS=n] [FEATURES=...] $0 <waysaver args...>" >&2; exit 2; }

cargo build --release --features "${FEATURES:-}"

# labwc's -s takes a single command string: wrap the call in a temp script.
inner=$(mktemp)
trap 'rm -f "$inner"' EXIT
{
    echo '#!/bin/sh'
    printf 'exec %s' "$PWD/target/release/waysaver"
    for arg in "$@"; do printf " '%s'" "$arg"; done
    echo
} > "$inner"
chmod +x "$inner"

# A private session bus keeps the test away from the real one: otherwise this
# waysaver could grab org.freedesktop.ScreenSaver and receive the inhibitors of
# apps in the real session. The system bus (logind) is still shared.
WLR_WL_OUTPUTS="${OUTPUTS:-1}" dbus-run-session -- labwc -C scripts/nested-labwc -s "$inner"
