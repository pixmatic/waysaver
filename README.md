# wayvoid

A minimal screensaver for Wayland, written in Rust.

After a period of inactivity it covers the screen with pure black, and removes it as soon as there is activity again. Optionally it can run any program as a theme instead: a video, an animated GIF, a terminal animation, a WebGL page, a GLSL shader...

- **Tiny and idle-friendly**: a single thread blocked in `poll()`, 0% CPU while waiting, ~4 MB of memory. The compositor keeps track of idle time; wayvoid just sleeps until it is told.
- **No toolkit, no GPU**: raw Wayland protocols only. The black screen is a single 1x1 pixel stretched by the compositor.
- **Respects inhibitors**: videos in the browser, games, `systemd-inhibit`...
- **Not a screen locker**: there is no password and no authentication. Use a real locker (swaylock, hyprlock...) if you need one.
- **Modular**: every extra lives behind an optional Cargo feature; the default build is the minimal one.

## Requirements

- A Wayland compositor that implements **`wlr-layer-shell`** and **`ext-idle-notify-v1`**: Sway, Hyprland, niri, river, labwc, Wayfire, KDE Plasma 6, COSMIC... (GNOME is not supported, see [below](#gnome)).
- `libdbus-1`, present on any desktop (and its development files to build it).
- Rust stable to build it.

If the compositor lacks a protocol, wayvoid exits with an error naming it.

## Building

```sh
cargo build --release                              # minimal: black screen only
cargo build --release --features command           # with themes
cargo build --release --all-features               # everything
```

The binary ends up in `target/release/wayvoid`.

| Feature | What it adds |
|---|---|
| *(none)* | Black screen on the current monitor. |
| `multimonitor` | One cover per monitor, following hotplug. |
| `ignore-mouse` | The `--ignore-mouse` option: moving the mouse doesn't wake the screen; keys, buttons, scrolling and touch do. |
| `command` | Themes: run an external program while idle. |

Features are independent and can be combined freely.

## Usage

```sh
wayvoid [OPTIONS] <idle-seconds>                   # without the `command` feature
wayvoid [OPTIONS] <idle-seconds> <command...>      # with it
```

- `<idle-seconds>`: inactivity time before blanking (1 to 4294967).
- `<command...>` (feature `command`): the program to run while idle. Everything after the seconds is the command, so wayvoid's own options go **before** the seconds. `blank` means the plain black screen.

Options (only those compiled in are accepted and shown by `--help`):

| Option | Feature | |
|---|---|---|
| `-h`, `--help` | | Show the help. |
| `-V`, `--version` | | Show the version. |
| `--ignore-mouse` | `ignore-mouse` | Don't wake up on mouse motion. |
| `--backdrop` | `command` | Black background below the theme while it starts (see [notes](#how-themes-work)). |

Whatever wakes the screen up (a key, a click) is swallowed: it never reaches the application underneath.

## Examples

### Black screen

```sh
wayvoid 300                    # build without `command`
wayvoid 300 blank              # build with `command`
wayvoid --ignore-mouse 600 blank
```

### Terminal animations

Any terminal program works if it runs inside a fullscreen terminal. `foot` is a small, Wayland-native terminal:

```sh
# The Matrix rain on black
wayvoid 300 foot -o colors-dark.background=000000 --fullscreen sh -c 'sleep 0.3; exec cmatrix -b'

# Big centered clock
wayvoid 300 foot -o colors-dark.background=000000 --fullscreen sh -c 'sleep 0.3; exec tty-clock -c -s -C 2'

# A clock bouncing around the screen
wayvoid 300 foot -o colors-dark.background=000000 --fullscreen sh -c 'sleep 0.3; exec tty-clock -s -r -C 2'

# A big clock jumping to a random spot every 10 seconds (examples/jumpclock, needs figlet)
wayvoid 300 foot -o colors-dark.background=000000 --fullscreen sh -c 'sleep 0.3; exec examples/jumpclock 10'

# An aquarium, pipes, a growing bonsai
wayvoid 300 foot --fullscreen sh -c 'sleep 0.3; exec asciiquarium'
wayvoid 300 foot --fullscreen sh -c 'sleep 0.3; exec pipes.sh'
wayvoid 300 foot --fullscreen sh -c 'sleep 0.3; exec cbonsai -l -i'
```

Moving clocks also keep anything from staying on the same pixels, which avoids burn-in on OLED screens.

The `sleep 0.3` gives the terminal time to become fullscreen first: some programs (cmatrix among them) keep the size they started with and would only draw in an 80x24 corner.

**Over the live desktop.** If the theme's window is transparent, the desktop shows through *live*, with windows still updating underneath (unlike X11 screensavers, which drew over a frozen screenshot):

```sh
wayvoid 300 foot -o csd.preferred=none -o colors-dark.alpha=0.0 -o colors-dark.alpha-mode=all \
    --maximized sh -c 'sleep 0.3; exec cmatrix -b'
```

- `--maximized` instead of `--fullscreen`: at least with foot on labwc, a fullscreen window is drawn opaque.
- `alpha-mode=all`: cmatrix paints its cells with the "black" palette color, which foot would otherwise keep opaque.
- `csd.preferred=none`: no title bar.

### Videos

`mpv` is the easiest choice:

```sh
# One video in a loop
wayvoid 300 mpv --fs --loop-file=inf --no-audio --no-osc --stop-screensaver=no ~/Videos/aquarium.mp4

# A whole folder, shuffled
wayvoid 300 mpv --fs --shuffle --loop-playlist=inf --no-audio --no-osc --stop-screensaver=no ~/Videos/screensaver/
```

`--stop-screensaver=no` matters: a playing mpv otherwise asks the compositor not to blank, which may wake the screen right away (see [notes](#how-themes-work)).

With `multimonitor`, use `$WAYVOID_OUTPUT` to put each instance on its monitor (it needs a shell to expand the variable):

```sh
wayvoid 300 sh -c 'exec mpv --fs --fs-screen-name="$WAYVOID_OUTPUT" --loop-file=inf \
    --no-audio --no-osc --stop-screensaver=no ~/Videos/aquarium.mp4'
```

### Animated GIFs and images

```sh
# An animated GIF (mpv plays them like videos)
wayvoid 300 mpv --fs --loop-file=inf --no-osc --stop-screensaver=no ~/Pictures/fire.gif

# Slideshow, 10 seconds per image
wayvoid 300 mpv --fs --image-display-duration=10 --shuffle --loop-playlist=inf \
    --no-osc --stop-screensaver=no ~/Pictures/wallpapers/
```

### Generated animations (no files needed)

`ffplay` (part of FFmpeg) can render synthetic sources:

```sh
wayvoid 300 ffplay -fs -loglevel quiet -f lavfi -i mandelbrot
wayvoid 300 ffplay -fs -loglevel quiet -f lavfi -i 'life=s=320x180:mold=10:ratio=0.1:death_color=#000000:life_color=#00ff00,scale=1920:1080:flags=neighbor'
```

### WebGL and HTML5

Any web page in a browser's kiosk mode. Use a **separate profile**: otherwise the browser hands the page to the instance you already have open and the process exits immediately (wayvoid would treat it as a failed command, and couldn't close the window on wake).

```sh
# Chromium (or Chrome: google-chrome-stable)
wayvoid 300 chromium --kiosk --no-first-run --user-data-dir=/tmp/wayvoid-chromium --ozone-platform=wayland \
    https://webglsamples.org/aquarium/aquarium.html

# Firefox
wayvoid 300 sh -c 'mkdir -p /tmp/wayvoid-firefox && exec firefox --kiosk --new-instance \
    --profile /tmp/wayvoid-firefox https://webglsamples.org/aquarium/aquarium.html'
```

A local `file:///…/index.html` works just as well, so you can write your own HTML5 screensaver.

### GLSL shaders

- **Shadertoy** shaders run in the browser through their embed URL:

  ```sh
  wayvoid 300 chromium --kiosk --no-first-run --user-data-dir=/tmp/wayvoid-chromium --ozone-platform=wayland \
      'https://www.shadertoy.com/embed/XsXXDn?gui=false&paused=false&muted=true'
  ```

- **Local fragment shaders** with [glslViewer](https://github.com/patriciogonzalezvivo/glslViewer):

  ```sh
  wayvoid 300 glslViewer ~/shaders/plasma.frag --fullscreen
  ```

### How themes work

- wayvoid's own surface stays above everything and turns **transparent** while the theme runs. It still catches the input, hides the cursor and swallows the waking key, and `--ignore-mouse` keeps working.
- The theme must make itself **fullscreen** (or maximized and transparent). It runs directly, without a shell; wrap it in `sh -c '...'` for variables, pipes or several commands.
- With `multimonitor` one instance runs per monitor, with its name in `$WAYVOID_OUTPUT` (e.g. `HDMI-A-1`).
- On wake, the theme and anything it started get `SIGTERM`. It never outlives wayvoid.
- If the theme exits on its own it is restarted. If it exits within 2 seconds, it is considered broken: that monitor goes black instead.
- Many programs ask not to be interrupted while they run (SDL games and players, browsers...). Requests made **through D-Bus** by the theme itself are recognized and ignored. Requests made **through Wayland** (`zwp_idle_inhibit`, e.g. mpv) can't be told apart, because the compositor handles them: they may wake the screen right away. Disable them in the theme (`mpv --stop-screensaver=no`).
- `--backdrop` puts a black surface below the theme so the desktop isn't visible while it starts. It needs the compositor to stack fullscreen windows above the `top` layer; otherwise the black would hide the theme.

## Inhibitors

wayvoid doesn't blank while something asks it not to:

| Source | Who uses it |
|---|---|
| Wayland (`zwp_idle_inhibit`) | Chrome/Chromium, mpv, games, video players. Handled by the compositor. |
| D-Bus `org.freedesktop.ScreenSaver` | Firefox, VLC, SDL applications... wayvoid provides this service itself. |
| logind (`systemd-inhibit --what=idle`) | Scripts, and anything you wrap with it (see below). |

An inhibitor appearing while the screen is blank wakes it up.

### Applications that don't inhibit: `systemd-inhibit`

Some programs should keep the screen on but don't ask (many games, especially with a controller, which the compositor doesn't count as activity). Wrap them with `systemd-inhibit`:

```sh
systemd-inhibit --what=idle --who=retroarch --why="playing" retroarch
```

- **Steam**: put this in a game's launch options (right click → Properties):

  ```
  systemd-inhibit --what=idle --who=steam --why=playing %command%
  ```

- **Manual switch**, e.g. for a long presentation:

  ```sh
  systemd-inhibit --what=idle --why="presentation" sleep infinity   # Ctrl+C to end it
  ```

It needs systemd-logind (or elogind). Only `--what=idle` matters to wayvoid; `systemd-inhibit`'s default (`idle:sleep:shutdown`) also blocks suspend and shutdown.

## GNOME

**wayvoid doesn't work on GNOME, and it won't.** Mutter (GNOME's compositor) deliberately doesn't offer the two protocols it relies on:

- **`wlr-layer-shell`**, which lets a client place a surface above every window. GNOME reserves that for gnome-shell itself; an ordinary window can't guarantee it stays on top, nor that it gets the keyboard.
- **`ext-idle-notify-v1`**, which tells a client when the user is idle.

A GNOME backend could be built from a fullscreen window plus GNOME's D-Bus APIs, but it would be fragile: the panel and notifications could show on top, the waking key would reach other applications, and themes and `--ignore-mouse` wouldn't work reliably. Besides, GNOME already does what wayvoid's basic mode does: *Settings → Power → Screen Blank*, which respects inhibitors.

The right way to build a screensaver for GNOME is a **separate project as a GNOME Shell extension**: extensions run inside gnome-shell, the only place with the privileges to draw above everything and handle input properly.

## Running at login

Add it to your compositor's autostart, e.g. Sway:

```
exec wayvoid 300 blank
```

Or as a systemd user service, `~/.config/systemd/user/wayvoid.service`:

```ini
[Unit]
Description=wayvoid screensaver
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.local/bin/wayvoid 300 blank
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

```sh
systemctl --user enable --now wayvoid.service
```

(Your compositor must start `graphical-session.target` and pass `WAYLAND_DISPLAY` to the systemd user environment, which most setups already do.)

## Logs

wayvoid only prints state changes and errors to stderr: started, blanked, restored, inhibitors, monitors, theme processes. Under systemd they go to the journal with proper priorities:

```sh
journalctl --user -u wayvoid -f          # follow
journalctl --user -u wayvoid -p warning  # warnings and errors only
```

To see every Wayland message, run it with `WAYLAND_DEBUG=1`.

## Development

- `scripts/check.sh`: formatting, then clippy (no warnings) and tests for **every combination** of features. Needs [cargo-hack](https://github.com/taiki-e/cargo-hack).
- `scripts/nested.sh`: runs wayvoid inside a nested labwc (in a window, with its own D-Bus session), so it can be tested without blanking your real screen. `OUTPUTS=2 FEATURES=multimonitor scripts/nested.sh 5` gives two virtual monitors; inside, F1/F2 add or remove monitors.
- Each feature lives in its own files, with an empty stub (`*_off.rs`) used when it is disabled; the core code has no `#[cfg]`. See the module documentation in `src/main.rs`.
