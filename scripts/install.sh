#!/bin/sh
# Installs waysaver from this source tree: the binary, a systemd user service
# (on systems with systemd) and the example themes. Build it first, as
# yourself, not with sudo (`make` does it, with the features in config.mk):
#
#   cargo build --release --all-features
#
# Then:
#
#   sudo scripts/install.sh  # for every user, under /usr/local, and enabled
#                            # for all of them (systemctl --global enable)
#   scripts/install.sh       # just for you, under ~/.local and ~/.config;
#                            # then enable it: systemctl --user enable --now waysaver
#
# Options:
#   --autostart   also start it at login through an XDG autostart entry, for
#                 sessions that don't start graphical-session.target (XFCE,
#                 LXQt, plain labwc...). Without systemd, the entry runs
#                 waysaver directly.
#   --uninstall   remove what the install put in place (your themes and
#                 settings in ~/.config/waysaver are kept)
#
# Environment, for packagers (`make install` passes them on):
#   PREFIX=/usr   install for every user under PREFIX (default /usr/local
#                 with sudo, ~/.local and ~/.config without)
#   DESTDIR=dir   stage the files under dir, to build a package from: nothing
#                 is enabled and systemctl isn't run
#   SYSTEMD=no    don't install the service (default: yes if systemd is the
#                 init, and always when staging with DESTDIR)
#
# The service is always a user service, even when installed for every user:
# a screensaver must run inside each user's graphical session, which a
# system service can't reach. Installed with sudo, it is enabled globally
# (like PipeWire): it starts in every user's session, and a user who doesn't
# want it opts out with: systemctl --user mask waysaver
#
# Without systemd (OpenRC, runit, sysvinit...) no service is installed: those
# inits run system services, outside the user's session. Start waysaver from
# your compositor's autostart instead, or with --autostart.
set -eu
cd "$(dirname "$0")/.."

autostart= uninstall=
for arg in "$@"; do
    case $arg in
        --autostart) autostart=1 ;;
        --uninstall) uninstall=1 ;;
        -h | --help) sed -n '2,/^set -eu/{/^set -eu/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        *) echo "install.sh: unknown option: $arg" >&2; exit 2 ;;
    esac
done

PREFIX=${PREFIX:-} DESTDIR=${DESTDIR:-} SYSTEMD=${SYSTEMD:-}

# Where everything goes. The paths below are where the files will live once
# installed; with DESTDIR they are written under it instead ($DESTDIR$path).
# For every user with sudo (or a PREFIX or DESTDIR), just for you without.
admin=
[ "$(id -u)" -eq 0 ] && [ -z "$DESTDIR" ] && admin=1
if [ -n "$PREFIX$DESTDIR$admin" ]; then
    scope=system  # for every user
    prefix=${PREFIX:-/usr/local}
    bin=$prefix/bin
    units=$prefix/lib/systemd/user
    themes=$prefix/share/waysaver/themes
    autostart_dir=/etc/xdg/autostart
else
    scope=user  # just for you
    config=${XDG_CONFIG_HOME:-$HOME/.config}
    bin=$HOME/.local/bin
    units=$config/systemd/user
    themes=$config/waysaver/themes
    autostart_dir=$config/autostart
fi
unit=$units/waysaver.service
desktop=$autostart_dir/waysaver.desktop

# A PREFIX without the rights to write there: say so, rather than fail
# halfway through.
if [ "$scope" = system ] && [ -z "$DESTDIR" ] && [ ! -w "$prefix" ]; then
    echo "install.sh: can't write to $prefix; run it with sudo." >&2
    exit 1
fi

# Whether to install the service: systemd is the init (its documented check,
# as sd_booted() does), or a package is being staged.
case $SYSTEMD in
    yes | no) ;;
    "") if [ -n "$DESTDIR" ] || [ -d /run/systemd/system ]; then SYSTEMD=yes; else SYSTEMD=no; fi ;;
    *) echo "install.sh: SYSTEMD must be yes or no" >&2; exit 2 ;;
esac

# Runs systemctl for this user, if there is a user manager to talk to.
user_systemctl() {
    [ "$SYSTEMD" = yes ] && [ "$scope" = user ] && systemctl --user "$@" 2>/dev/null || true
}

# Runs systemctl for every user's sessions (as root, not when staging).
global_systemctl() {
    [ "$SYSTEMD" = yes ] && [ -n "$admin" ] && systemctl --global "$@" || true
}

if [ -n "$uninstall" ]; then
    user_systemctl disable --now waysaver.service
    global_systemctl disable waysaver.service 2>/dev/null
    rm -fv "$DESTDIR$bin/waysaver" "$DESTDIR$unit" "$DESTDIR$desktop"
    # The example themes: all of them for everyone; a user's are kept.
    [ "$scope" = system ] && rm -rfv "$DESTDIR${themes%/themes}"
    user_systemctl daemon-reload
    if [ "$SYSTEMD" = no ]; then
        echo "Remember to remove waysaver from your compositor's autostart, if you added it."
    elif [ -n "$admin" ]; then
        echo "Disabled for every user; it stops at their next login (or now with:"
        echo "systemctl --user stop waysaver)."
    fi
    [ "$scope" = system ] || echo "Your themes and settings in ${themes%/themes} were kept."
    exit 0
fi

built=target/release/waysaver
if [ ! -x "$built" ]; then
    echo "install.sh: $built not found; build it first (as yourself, not with sudo):" >&2
    echo "  make    (or: cargo build --release --all-features)" >&2
    exit 1
fi

# The binary.
install -Dm755 "$built" "$DESTDIR$bin/waysaver"
echo "installed $DESTDIR$bin/waysaver"

# The example themes. A user's copies are never overwritten: they may have
# been edited. The system-wide ones are the installer's, so they are.
if "$built" --help | grep -q '<theme'; then
    blank=" blank"
    mkdir -p "$DESTDIR$themes"
    for theme in examples/themes/*; do
        target=$DESTDIR$themes/${theme##*/}
        if [ "$scope" = user ] && [ -e "$target" ]; then
            echo "kept    $target (already there)"
        else
            install -m755 "$theme" "$target"
            echo "installed $target"
        fi
    done
else
    blank=  # built without the theme feature: no <theme> argument
fi

# The systemd user service.
if [ "$SYSTEMD" = yes ]; then
    mkdir -p "$DESTDIR$units"
    cat > "$DESTDIR$unit" <<EOF
[Unit]
Description=waysaver screensaver
Documentation=https://github.com/pixmatic/waysaver
# Runs within the graphical session: it needs the session's Wayland display.
PartOf=graphical-session.target
After=graphical-session.target

[Service]
# To change the idle time or the theme, run: systemctl --user edit waysaver
# and add, between the comment lines it shows, e.g.:
#   [Service]
#   ExecStart=
#   ExecStart=$bin/waysaver -l info 600 matrix --color cyan
ExecStart=$bin/waysaver 300$blank
Restart=on-failure
# 2 is a usage error (e.g. the theme wasn't found): retrying won't help.
RestartPreventExitStatus=2

[Install]
WantedBy=graphical-session.target
EOF
    echo "installed $DESTDIR$unit"
fi

# The XDG autostart entry, for sessions without graphical-session.target.
if [ -n "$autostart" ]; then
    mkdir -p "$DESTDIR$autostart_dir"
    cat > "$DESTDIR$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=waysaver
Comment=Wayland screensaver
$(if [ "$SYSTEMD" = yes ]; then
        echo "# Hands the session's Wayland display to systemd, then starts the service."
        echo "Exec=sh -c 'systemctl --user import-environment WAYLAND_DISPLAY && systemctl --user start waysaver.service'"
    else
        echo "# No systemd: runs it directly, within the session."
        echo "Exec=$bin/waysaver 300$blank"
    fi)
NoDisplay=true
EOF
    echo "installed $DESTDIR$desktop"
fi

user_systemctl daemon-reload

# What's left to do.
echo
if [ -n "$DESTDIR" ]; then
    echo "Staged under $DESTDIR. Nothing was enabled: the package decides that"
    echo "(e.g. systemctl --global enable waysaver after installing it)."
    exit 0
elif [ "$SYSTEMD" = no ]; then
    if [ -n "$autostart" ]; then
        echo "Installed. Desktops that read XDG autostart entries (XFCE, LXQt, KDE...)"
        echo "start it at login: edit $desktop to change the idle time or the theme."
        echo "Compositors that don't (Sway, Hyprland, niri...) need it in their autostart:"
    else
        echo "Installed. No systemd here, so start it from your compositor's autostart:"
    fi
    echo "  Sway, i3-style:  exec $bin/waysaver 300$blank"
    echo "  Hyprland:        exec-once = $bin/waysaver 300$blank"
    echo "  niri:            spawn-at-startup \"$bin/waysaver\" \"300\"${blank:+ \"blank\"}"
    [ -n "$autostart" ] ||
        echo "or install it again with --autostart, for desktops like XFCE or LXQt."
elif [ -n "$admin" ]; then
    # Enabled in every user's session, through /etc/systemd/user.
    global_systemctl enable waysaver.service
    echo "Installed and enabled for every user: it starts at their next login."
    echo "  systemctl --user start waysaver          # start it now in your session"
    echo "  systemctl --user edit waysaver           # change the idle time or the theme"
    echo "  systemctl --user mask waysaver           # a user who doesn't want it"
    echo "  sudo systemctl --global disable waysaver # disable it for everyone"
    if [ -z "$autostart" ]; then
        echo
        echo "Note: sessions that don't start graphical-session.target (XFCE, LXQt,"
        echo "labwc...) won't start it at login; install it again with --autostart."
    fi
else
    echo "Installed just for you (with sudo, it installs for every user). Now run:"
    if [ -n "$autostart" ]; then
        echo "  systemctl --user start waysaver   # start now (it starts by itself at login)"
    else
        echo "  systemctl --user enable --now waysaver   # start now, and at every login"
    fi
    echo "  systemctl --user edit waysaver           # change the idle time or the theme"
    if [ -z "$autostart" ] &&
        ! systemctl --user is-active --quiet graphical-session.target 2>/dev/null; then
        echo
        echo "Note: your session doesn't start graphical-session.target, so the service"
        echo "won't start at login by itself. Install it again with --autostart."
    fi
fi
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo; echo "Note: $bin is not in your PATH; add it to run waysaver by name." ;;
esac
