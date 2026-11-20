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
cargo build --release --features theme             # with themes
cargo build --release --all-features               # everything
```

The binary ends up in `target/release/wayvoid`.

| Feature | What it adds |
|---|---|
| *(none)* | Black screen on the current monitor. |
| `multimonitor` | One cover per monitor, following hotplug. |
| `ignore-mouse` | The `--ignore-mouse` option: moving the mouse doesn't wake the screen; keys, buttons, scrolling and touch do. |
| `theme` | Themes: run a program while idle (a video, an animation, a web page...). |

Features are independent and can be combined freely.

## Usage

```sh
wayvoid [OPTIONS] <idle-seconds>                   # without the `theme` feature
wayvoid [OPTIONS] <idle-seconds> <theme...>        # with it
```

- `<idle-seconds>`: inactivity time before blanking (1 to 4294967).
- `<theme...>` (feature `theme`): what to show while idle. `blank` is the plain black screen; anything else is a program and its arguments: either a theme from your [themes directory](#installing-the-example-themes) or [any command](#running-a-program-directly). Everything after the seconds belongs to the theme, so wayvoid's own options go **before** the seconds.

Options (only those compiled in are accepted and shown by `--help`):

| Option | Feature | |
|---|---|---|
| `-h`, `--help` | | Show the help. |
| `-V`, `--version` | | Show the version. |
| `-l`, `--log-level LEVEL` | | What to log, from `0`/`none` to `4`/`debug` (see [Logs](#logs)); default `warning`. |
| `--ignore-mouse` | `ignore-mouse` | Don't wake up on mouse motion. |
| `--backdrop` | `theme` | Black background below the theme while it starts (see [notes](#how-themes-work)). |

Whatever wakes the screen up (a key, a click) is swallowed: it never reaches the application underneath.

## Themes

A theme is what wayvoid shows instead of black. It is just a program: wayvoid runs it while the screen is idle and stops it on wake.

### Installing the example themes

This repository ships ready-made themes in [`examples/themes/`](examples/themes). Copy them to your themes directory, `~/.config/wayvoid/themes/` (`$XDG_CONFIG_HOME/wayvoid/themes/`), and call them by name:

```sh
mkdir -p ~/.config/wayvoid/themes
cp examples/themes/* ~/.config/wayvoid/themes/

wayvoid 300 matrix
```

A bare name is looked up in the themes directory first, then in `PATH`.

| Theme | Shows | Options | Needs |
|---|---|---|---|
| `matrix` | The Matrix rain | `--color`, `--background` | foot, cmatrix |
| `matrix-live` | The Matrix rain over the live desktop | `--color` | foot, cmatrix |
| `marquee` | Your text in a big font, bouncing and changing color | `--font`, `--color`, `--background`, `--speed`, `--transparent`, text | foot, figlet |
| `clock` | A big clock, bouncing or still | `--still`, `--color`, `--background` | foot, tty-clock |
| `jumpclock` | A big clock jumping to a random spot | `--font`, `--color`, `--background`, seconds | foot, figlet |
| `aquarium` | An ASCII aquarium | `--background` | foot, asciiquarium |
| `pipes` | Pipes crawling across the screen, in 2D | `--count`, `--color`, `--background` | foot, pipes.sh |
| `bonsai` | A bonsai tree growing | `--message`, `--background` | foot, cbonsai |
| `mandelbrot` | An endless zoom into the Mandelbrot set | | ffplay |
| `life` | Conway's Game of Life | `--color`, `--background` | ffplay |
| `text3d` | Your text with depth, spinning in 3D | `--font`, `--color`, `--background`, `--speed`, text | a browser¹ |
| `maze3d` | A walk through a random 3D maze | `--size`, `--speed`, `--color` | a browser¹ |
| `pipes3d` | 3D pipes growing until they fill the screen | `--speed`, `--pipes`, `--size`, `--background` | python, pygame |
| `shader` | A GLSL fragment shader (a plasma one by default) | file | glslViewer |
| `shadertoy` | A shader from shadertoy.com (needs Internet) | shader id | a browser¹ |
| `video` | Videos, GIFs or images (a slideshow), shuffled | `--image-time`, files or folders | mpv |
| `web` | Any web page, e.g. your own HTML5 screensaver | `--browser`, URL | a browser¹ |
| `blur` | A blurred, frozen snapshot of the screen² | `--dim`, percentage | grim, ffmpeg, mpv |

¹ Chrome, Chromium or Firefox. The 3D pages are plain HTML and JavaScript, with no libraries and no network access.
² On Wayland only the compositor sees other windows, so a *live* blur is impossible. `blur` takes a snapshot with `grim`, which needs the wlr-screencopy protocol (labwc, Sway, Hyprland, river, niri...; not KDE).

### Customizing a theme

Everything after the theme's name is passed to it, so each theme takes its own options. Its first lines list them, with an example. Colors are names (`red`, `cyan`...) for terminal themes, `RRGGBB` for backgrounds, and any CSS color in browser themes.

```sh
wayvoid 300 matrix --color cyan --background 001018
wayvoid 300 clock --still --color yellow --background 1a1a2e
wayvoid 300 marquee --font slant --color cyan --background 101828 "Back in 5 minutes"
wayvoid 300 bonsai --message "Out for lunch"
wayvoid 300 life --color ff8800 --background 100800
wayvoid 300 text3d --font "DejaVu Serif" --color gold --background "#102030" Hello
wayvoid 300 maze3d --size 20 --speed 0.7 --color 30
wayvoid 300 pipes3d --speed 30 --pipes 20
wayvoid 300 video --image-time 30 ~/Pictures/wallpapers ~/Videos/nature
wayvoid 300 blur --dim 40 70
```

Mistakes, such as an unknown color, are reported in wayvoid's log (see [Logs](#logs)) and that screen stays black.

### Writing a theme

The example themes are short scripts (shell, plus one in Python): copy one and edit it to make your own. Any executable works, but they follow a convention that makes them self-describing:

```sh
#!/bin/sh
THEME_NAME="Aquarium"
THEME_DESCRIPTION="An ASCII-art aquarium."
THEME_VERSION="1.1"
THEME_REQUIRES="foot asciiquarium"
THEME_USAGE="aquarium [--background RRGGBB]"

for dep in $THEME_REQUIRES; do
    command -v "$dep" >/dev/null || { echo "$0: $dep is not installed" >&2; exit 1; }
done

# ... read and check the options ...

exec foot -o colors-dark.background="$background" --fullscreen sh -c 'sleep 0.3; exec asciiquarium'
```

- **Metadata** goes at the top, one `THEME_*="value"` per line, with **literal values** (no `$HOME`, no `$(...)`), so it can be read without running the theme: `grep -H '^THEME_DESCRIPTION' ~/.config/wayvoid/themes/*`.
- `THEME_REQUIRES` lists the programs the theme needs; checking them makes a missing one show up clearly in the logs. Alternatives are written `a|b` and checked by the theme itself (see `web`).
- `THEME_USAGE` shows its options, which it parses itself from its arguments. Check them **before** starting anything slow, and exit with an error message: it reaches wayvoid's log.
- **`exec`** the final program, so it is the process wayvoid stops on wake (wayvoid stops the whole process group anyway).
- It must show itself **fullscreen** and keep running; one that exits within 2 seconds is considered broken (that monitor goes black). See [how themes work](#how-themes-work).
- Write it in any language: in Python, the same `THEME_*="value"` lines are valid code (see `pipes3d`).

Tips learned from the examples:

- **Terminal programs** (`foot` is a small, Wayland-native terminal): start them after `sleep 0.3`, so the terminal is already fullscreen. Some (cmatrix, tty-clock) keep the size they started with and would only draw in an 80x24 corner.
- **Over the live desktop**: a transparent window shows the desktop *live* underneath, with windows still updating (unlike X11 screensavers, which drew over a frozen screenshot). With foot: `-o colors-dark.alpha=0.0 -o colors-dark.alpha-mode=all -o csd.preferred=none --maximized`. Maximized, because at least on labwc a fullscreen window is drawn opaque.
- **Browsers**: use a separate profile (`--user-data-dir`, or Firefox's `--profile` with `--new-instance`). Otherwise the page is handed to the browser you already have open and the process exits at once.
- **mpv**: pass `--stop-screensaver=no`. A playing mpv otherwise asks the compositor not to blank, which may wake the screen right away.
- **SDL programs** (pygame, games...): set `SDL_VIDEO_ALLOW_SCREENSAVER=1`, for the same reason.
- **Several monitors** (`multimonitor`): one instance runs per monitor, with its name in `$WAYVOID_OUTPUT`, e.g. for `mpv --fs-screen-name`.

### Running a program directly

A theme doesn't have to be installed: anything after the seconds can be a command, run as is. Handy to try something out before turning it into a theme:

```sh
wayvoid 300 mpv --fs --no-osc --no-audio --loop-playlist=inf --stop-screensaver=no ~/Videos/aquarium.mp4
wayvoid 300 foot --fullscreen sh -c 'sleep 0.3; exec cmatrix -b'
```

The command runs directly, without a shell: wrap it in `sh -c '...'` for variables, pipes or several commands.

### Black screen

`blank` is the plain black screen (and the only mode without the `theme` feature):

```sh
wayvoid 300                    # build without `theme`
wayvoid 300 blank              # build with `theme`
wayvoid --ignore-mouse 600 blank
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

wayvoid logs to stderr. How much is set with `-l`/`--log-level`, by number or name:

| Level | Shows |
|---|---|
| `0`, `none` | Nothing at all. |
| `1`, `error` | Fatal errors, and themes that can't be started. |
| `2`, `warning` | Also problems that don't stop it: a D-Bus source unavailable, a theme that fails right away (with its last error lines). **The default.** |
| `3`, `info` | Also every change: blanked, restored, inhibited and released (and by whom), monitors, themes started and restarted. |
| `4`, `debug` | Everything it does (protocols bound, idle notifications, overlays, D-Bus calls, signals sent to themes...) plus **each line the theme prints**, on stdout or stderr. |

```sh
wayvoid -l info 300 matrix
wayvoid --log-level=debug 300 marquee --color pink   # see why a theme fails
```

Below `debug`, a theme's stdout is discarded; its stderr is read only to report why it failed.

Under systemd the messages go to the journal with their priority, so they can be filtered there too:

```sh
journalctl --user -u wayvoid -f          # follow
journalctl --user -u wayvoid -p warning  # warnings and errors only
```

To also see every Wayland message, run it with `WAYLAND_DEBUG=1`.

## Development

- `scripts/check.sh`: formatting, then clippy (no warnings) and tests for **every combination** of features. Needs [cargo-hack](https://github.com/taiki-e/cargo-hack).
- `scripts/nested.sh`: runs wayvoid inside a nested labwc (in a window, with its own D-Bus session), so it can be tested without blanking your real screen. `OUTPUTS=2 FEATURES=multimonitor scripts/nested.sh 5` gives two virtual monitors; inside, F1/F2 add or remove monitors.
- Each feature lives in its own files, with an empty stub (`*_off.rs`) used when it is disabled; the core code has no `#[cfg]`. See the module documentation in `src/main.rs`.
