#!/usr/bin/env bash
# Runs the theme examples from the README one after another, so they can all
# be watched (and checked) hands-off. Each theme blanks the screen after IDLE
# seconds of inactivity, stays on screen for SHOW seconds and is then stopped.
# Themes whose program is not installed are skipped. At the end, a summary
# flags any theme that failed to start or woke the screen by itself.
#
# This runs on your REAL screen. Don't touch the mouse or keyboard while it
# runs: any input wakes the screen and spoils that theme's result. Ctrl+C
# stops it at any time. To watch it in a window instead, run it inside a
# nested compositor:
#
#   dbus-run-session -- labwc -C scripts/nested-labwc -s scripts/demo-themes.sh
#
# Usage:
#   scripts/demo-themes.sh              # every theme
#   ONLY=mpv scripts/demo-themes.sh     # only themes whose name contains "mpv"
#   SHOW=30 scripts/demo-themes.sh      # keep each theme on screen longer
#
# Settings (environment variables):
#   IDLE  seconds of inactivity before each theme starts (default 3)
#   SHOW  seconds each theme stays on screen (default 15)
#   ONLY  run only the themes whose name contains this text
#   DIR   working directory for sample files, browser profiles and logs
#         (default ~/wayvoid-test; delete it when done)
#
# Requires ffmpeg (to generate the sample files; nothing is downloaded).
# Optional, per theme: foot, cmatrix, tty-clock, figlet, asciiquarium, pipes.sh,
# cbonsai, mpv, Chrome or Chromium, Firefox, glslViewer, systemd-inhibit.
set -u
cd "$(dirname "$0")/.."

IDLE=${IDLE:-3}
SHOW=${SHOW:-15}
ONLY=${ONLY:-}
DIR=${DIR:-$HOME/wayvoid-test}
LOGS=$DIR/logs

command -v ffmpeg >/dev/null || { echo "ffmpeg is required to generate the sample files" >&2; exit 1; }
cargo build --release --quiet --features command || exit 1
W=$PWD/target/release/wayvoid

# --- Sample files, generated once ---------------------------------------
mkdir -p "$DIR/slides" "$LOGS"
lavfi() { ffmpeg -loglevel error -f lavfi "$@"; }
[ -e "$DIR/video.mp4" ] || lavfi -i testsrc2=s=1280x720:r=30 -t 10 "$DIR/video.mp4"
[ -e "$DIR/anim.gif" ] || lavfi -i mandelbrot=s=480x270 -t 4 "$DIR/anim.gif"
[ -e "$DIR/slides/1.png" ] || lavfi -i cellauto=s=1280x720:rule=110 -frames:v 5 -start_number 1 "$DIR/slides/%d.png"
[ -e "$DIR/plasma.frag" ] || cat > "$DIR/plasma.frag" <<'EOF'
uniform vec2 u_resolution;
uniform float u_time;
void main() {
    vec2 p = gl_FragCoord.xy / u_resolution;
    float v = sin(p.x * 10.0 + u_time) + sin(p.y * 10.0 + u_time * 1.3)
            + sin((p.x + p.y) * 10.0 + u_time * 0.7);
    gl_FragColor = vec4(0.5 + 0.5 * sin(v), 0.5 + 0.5 * sin(v + 2.0),
                        0.5 + 0.5 * sin(v + 4.0), 1.0);
}
EOF

# --- Helpers --------------------------------------------------------------
# Browsers get their own profile, so they don't hand the page over to an
# instance that is already open (that would make the process exit at once).
CHROME=$(command -v google-chrome-stable || command -v google-chrome || echo chromium)
CHROME_ARGS=(--kiosk --no-first-run --no-default-browser-check --ozone-platform=wayland
             --user-data-dir="$DIR/chrome-profile")
FIREFOX_PROFILE=$DIR/firefox-profile
mkdir -p "$FIREFOX_PROFILE"

# Terminal themes wait a moment so the terminal is already fullscreen when
# they start (cmatrix and others keep the size they started with).
DELAY='sleep 0.3; exec'

pid=
trap '[ -n "$pid" ] && kill "$pid" 2>/dev/null; echo; echo "Interrupted."; exit 130' INT TERM

results=()

# How a theme went, according to wayvoid's log.
verdict() {
    if ! grep -q "screen blanked" "$1"; then echo "NEVER BLANKED (input or inhibitor)"
    elif grep -q 'command ".*": ' "$1"; then echo "FAIL: could not start"
    elif grep -q "too soon" "$1"; then echo "FAIL: theme exited right away"
    elif grep -q "screen restored" "$1"; then echo "WOKE UP before the end (input or inhibitor)"
    elif grep -q "restarting" "$1"; then echo "ok (theme exited and was restarted)"
    else echo ok
    fi
}

# run NAME REQUIRED-PROGRAM COMMAND...
# Runs wayvoid with COMMAND as its theme, unless REQUIRED-PROGRAM is missing.
run() {
    local name=$1 need=$2
    shift 2
    [[ -n $ONLY && $name != *"$ONLY"* ]] && return
    if ! command -v "$need" >/dev/null; then
        printf '  %-18s skipped (%s not installed)\n' "$name" "$need"
        results+=("$name|skipped: $need not installed")
        return
    fi
    printf '▶ %-18s starts in %ss, shown for %ss. Hands off...\n' "$name" "$IDLE" "$SHOW"
    "$W" "$IDLE" "$@" > "$LOGS/$name.out" 2> "$LOGS/$name.log" &
    pid=$!
    sleep $((IDLE + SHOW))
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    pid=
    results+=("$name|$(verdict "$LOGS/$name.log")")
    sleep 2
}

# --- Themes ---------------------------------------------------------------
# Plain black
run blank             "$W"          blank

# Terminal animations, in a fullscreen foot terminal
run cmatrix           cmatrix       foot -o colors-dark.background=000000 --fullscreen sh -c "$DELAY cmatrix -b"
run cmatrix-live      cmatrix       foot -o csd.preferred=none -o colors-dark.alpha=0.0 \
                                        -o colors-dark.alpha-mode=all --maximized sh -c "$DELAY cmatrix -b"
run tty-clock         tty-clock     foot -o colors-dark.background=000000 --fullscreen sh -c "$DELAY tty-clock -c -s -C 2"
run tty-clock-bounce  tty-clock     foot -o colors-dark.background=000000 --fullscreen sh -c "$DELAY tty-clock -s -r -C 2"
run jumpclock         figlet        foot -o colors-dark.background=000000 --fullscreen sh -c "$DELAY examples/jumpclock 3"
run asciiquarium      asciiquarium  foot --fullscreen sh -c "$DELAY asciiquarium"
run pipes             pipes.sh      foot --fullscreen sh -c "$DELAY pipes.sh"
run cbonsai           cbonsai       foot --fullscreen sh -c "$DELAY cbonsai -l -i"

# Videos, animated GIFs and images
MPV=(mpv --fs --no-osc --stop-screensaver=no)
run mpv-video         mpv           "${MPV[@]}" --no-audio --loop-file=inf "$DIR/video.mp4"
run mpv-folder        mpv           "${MPV[@]}" --no-audio --shuffle --loop-playlist=inf "$DIR/"
run mpv-gif           mpv           "${MPV[@]}" --loop-file=inf "$DIR/anim.gif"
run mpv-slideshow     mpv           "${MPV[@]}" --image-display-duration=2 --loop-playlist=inf "$DIR/slides/"

# Generated animations
run ffplay-mandelbrot ffplay        ffplay -fs -loglevel quiet -f lavfi -i mandelbrot
run ffplay-life       ffplay        ffplay -fs -loglevel quiet -f lavfi \
                                        -i 'life=s=320x180:mold=10:ratio=0.1:death_color=#000000:life_color=#00ff00,scale=1920:1080:flags=neighbor'

# WebGL and GLSL
run chrome-webgl      "$CHROME"     "$CHROME" "${CHROME_ARGS[@]}" https://webglsamples.org/aquarium/aquarium.html
run firefox-webgl     firefox       firefox --kiosk --new-instance --profile "$FIREFOX_PROFILE" \
                                        https://webglsamples.org/aquarium/aquarium.html
run chrome-shadertoy  "$CHROME"     "$CHROME" "${CHROME_ARGS[@]}" \
                                        'https://www.shadertoy.com/embed/XsXXDn?gui=false&paused=false&muted=true'
run glslviewer        glslViewer    glslViewer "$DIR/plasma.frag" --fullscreen

# --- systemd-inhibit: no blanking while it holds, blanking after ----------
if [[ -z $ONLY || systemd-inhibit == *"$ONLY"* ]]; then
    if ! command -v systemd-inhibit >/dev/null; then
        printf '  %-18s skipped (systemd-inhibit not installed)\n' systemd-inhibit
        results+=("systemd-inhibit|skipped: systemd-inhibit not installed")
    else
        hold=$((IDLE + 5))
        printf '▶ %-18s inhibited for %ss: must NOT blank until then...\n' systemd-inhibit "$hold"
        systemd-inhibit --what=idle --who=wayvoid-demo --why=test sleep "$hold" &
        sleep 0.5
        "$W" "$IDLE" blank 2> "$LOGS/systemd-inhibit.log" &
        pid=$!
        sleep $((hold - 1))
        early=$(grep -c "screen blanked" "$LOGS/systemd-inhibit.log")
        sleep $((IDLE + 3))
        late=$(grep -c "screen blanked" "$LOGS/systemd-inhibit.log")
        kill "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
        pid=
        if ((early > 0)); then v="FAIL: blanked while inhibited"
        elif ((late == 0)); then v="FAIL: never blanked after the inhibitor ended"
        else v=ok
        fi
        results+=("systemd-inhibit|$v")
    fi
fi

# --- Summary --------------------------------------------------------------
echo
echo "Summary (logs in $LOGS):"
for r in "${results[@]}"; do
    printf '  %-18s %s\n' "${r%%|*}" "${r#*|}"
done
echo
echo "Sample files, profiles and logs are in $DIR; delete it when done."
