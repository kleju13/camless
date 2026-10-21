#!/bin/sh
# Installs camless for the user who runs it: both programs in ~/.local/bin, the
# service as a systemd user service that starts at login, the app in the desktop's
# menu, and udev rules that give the user at the desk the devices camless drives.
# Only the udev rules need root; sudo asks for it.
#
#   ./install.sh            install, or install again over an older version
#   ./install.sh --update   what the app runs to update itself: the same without a
#                           terminal, then the app opens again
#
# CAMLESS_BIN_DIR puts the programs somewhere other than ~/.local/bin.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
update=false
[ "${1:-}" = "--update" ] && update=true

if [ "$(id -u)" = 0 ]; then
  echo "Run this as yourself, not as root: camless runs as your user." >&2
  exit 1
fi

bin=${CAMLESS_BIN_DIR:-$HOME/.local/bin}
data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}
rules=/etc/udev/rules.d/71-camless.rules
fan_rules=/etc/udev/rules.d/72-camless-fans.rules
me=$(id -u)
user=$(id -un)

# The folders go into the menu entry and the service, where these characters would
# mean something else.
for path in "$bin" "$config"; do
  case $path in
    *[\|\&\\\"\$%\`]*)
      echo "camless cannot be installed with | & \\ \" \$ % or \` in a folder's name: $path" >&2
      exit 1 ;;
  esac
done

mkdir -p "$bin" "$data/applications" "$data/icons/hicolor/256x256/apps" "$data/camless"

# Copied next to the old ones, then renamed over them: that works while they run.
for exe in camless camless-gui; do
  cp "$here/$exe" "$bin/.$exe.new"
  chmod 755 "$bin/.$exe.new"
  mv -f "$bin/.$exe.new" "$bin/$exe"
done
echo "Installed camless and camless-gui in $bin."

# Where the programs are, in the files that start them.
fill() { sed "s|@BIN@|$bin|g" "$1" > "$2"; }
fill "$here/camless.desktop" "$data/applications/camless.desktop"
cp "$here/camless.png" "$data/icons/hicolor/256x256/apps/camless.png"
# So it is there after the download is gone.
cp "$here/uninstall.sh" "$data/camless/uninstall.sh"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true

# The udev rules, when they are new or changed: the devices for the user at the
# desk, and the motherboard's fan headers for this user. udev applies them to the
# devices plugged in now too, though liquidctl finds that a hidraw node can keep its
# old rights until the device is plugged in again or the PC restarts. Run as root,
# with each rules file and where it goes.
set -- "$here/71-camless.rules" "$rules"
# The name goes into the fan rule's shell command. A name that does not fit gets no
# fan rule: camless works, but the motherboard's headers stay root's.
fans=
case $user in
  *[!A-Za-z0-9._-]*|-*)
    echo "No motherboard fan rule for a user named $user: only letters, digits, . _ and - fit in it, so camless cannot set the motherboard's fan headers." >&2 ;;
  *)
    fans=$(mktemp)
    trap 'rm -f "$fans"' EXIT
    trap 'exit 1' INT HUP TERM
    sed "s|@USER@|$user|g" "$here/72-camless-fans.rules" > "$fans"
    set -- "$@" "$fans" "$fan_rules" ;;
esac
apply='while [ $# -ge 2 ]; do install -D -m 644 "$1" "$2" || exit 1; shift 2; done
if ! { udevadm control --reload && udevadm trigger --action=change --subsystem-match=hidraw --subsystem-match=usb --subsystem-match=tty --subsystem-match=hwmon; }; then
  echo "udev did not reload its rules; they apply after a restart, or when the device is plugged in again." >&2
fi'
if ! cmp -s "$here/71-camless.rules" "$rules" || { [ -n "$fans" ] && ! cmp -s "$fans" "$fan_rules"; }; then
  if $update; then
    # No terminal to ask in: pkexec asks through the desktop, if it is there.
    if ! { command -v pkexec >/dev/null 2>&1 && pkexec sh -c "$apply" sh "$@"; }; then
      echo "The udev rules changed and were not installed. Run install.sh from the download to install them." >&2
    fi
  else
    echo "Installing the udev rules in /etc/udev/rules.d; sudo asks for your password."
    sudo sh -c "$apply" sh "$@"
    echo "If camless cannot open a device yet, plug it in again or restart."
  fi
fi
[ -z "$fans" ] || rm -f "$fans"

# The service the app started outside systemd: this user's `camless run` processes
# other than the systemd service's. One started by hand in a terminal counts too, as
# two services cannot share the devices. Other camless commands, such as
# `camless mcp`, are left alone.
outside() {
  unit=$(systemctl --user show --property=MainPID --value camless.service 2>/dev/null || echo 0)
  for pid in $(pgrep -x -u "$me" camless || true); do
    if [ "$pid" != "$unit" ] && [ "$(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null | sed -n 2p)" = run ]; then
      echo "$pid"
    fi
  done
}

# Stops them, keeping each one's command line in $stopped, and waits up to 15 s, as
# systemd does: SIGTERM lets each set the fans to their safe speed first.
stopped=$(mktemp -d)
trap 'rm -rf "$stopped"' EXIT
trap 'exit 1' INT HUP TERM
stop_outside() {
  for pid in $(outside); do
    # One that ended meanwhile has no command line left: nothing to start again.
    if cat "/proc/$pid/cmdline" > "$stopped/$pid" 2>/dev/null && [ -s "$stopped/$pid" ]; then
      echo "Stopping camless $(tr '\0' '\n' < "$stopped/$pid" | tail -n +2 | tr '\n' ' ')(process $pid)."
      kill -TERM "$pid" 2>/dev/null || true
    else
      rm -f "$stopped/$pid"
    fi
  done
  n=0
  while [ -n "$(outside)" ] && [ $n -lt 75 ]; do
    sleep 0.2
    n=$((n + 1))
  done
}

# Starts them again from the new program, each with the arguments it had.
start_stopped() {
  for f in "$stopped"/*; do
    [ -e "$f" ] || continue
    set --
    while IFS= read -r arg; do
      set -- "$@" "$arg"
    done <<EOF
$(tr '\0' '\n' < "$f" | tail -n +2)
EOF
    setsid "$bin/camless" "$@" </dev/null >/dev/null 2>&1 &
  done
}

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  # Where this user's systemd looks for units, which follows its own environment.
  units=$(systemctl --user show-environment | sed -n 's/^XDG_CONFIG_HOME=//p')
  units=${units:-$HOME/.config}/systemd/user
  mkdir -p "$units"
  fill "$here/camless.service" "$units/camless.service"
  # A config folder set for the desktop, which the app reads, but maybe not for systemd.
  if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    sed -i "/^\[Service\]/a Environment=\"XDG_CONFIG_HOME=$XDG_CONFIG_HOME\"" "$units/camless.service"
  fi
  systemctl --user daemon-reload
  # The systemd service takes over from one the app started.
  stop_outside
  if $update; then
    systemctl --user try-restart camless.service || true
  else
    systemctl --user enable camless.service >/dev/null 2>&1
    systemctl --user restart camless.service
    echo "The camless service runs, and starts when you log in: systemctl --user status camless"
  fi
else
  # Without systemd the app starts the service. One it started runs again, from the
  # new program, before the app opens again: one still stopping would look running.
  stop_outside
  if [ -n "$(ls "$stopped")" ]; then
    start_stopped
    echo "The camless service runs again, from this version."
  else
    echo "No systemd user session, so the app starts the service when it opens."
  fi
fi

if $update; then
  # The app that started this opens again, as the setup does on Windows, once the old
  # one has gone: while it lives, a new one would hand its window over to it and end.
  n=0
  while pgrep -x -u "$me" camless-gui >/dev/null 2>&1 && [ $n -lt 50 ]; do
    sleep 0.2
    n=$((n + 1))
  done
  setsid "$bin/camless-gui" </dev/null >/dev/null 2>&1 &
else
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "$bin is not on your PATH; the menu entry works anyway." ;;
  esac
  echo "Open camless from your desktop's menu, or run camless-gui."
fi
