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

mkdir -p "$bin" "$data/applications" "$data/icons/hicolor/256x256/apps" "$data/camless"

# Copied next to the old ones, then renamed over them: that works while they run.
for exe in camless camless-gui; do
  cp "$here/$exe" "$bin/.$exe.new"
  chmod 755 "$bin/.$exe.new"
  mv -f "$bin/.$exe.new" "$bin/$exe"
done
echo "Installed camless and camless-gui in $bin."

# Where the programs are, in files that name them; | cannot be in a path here.
fill() { sed "s|@BIN@|$bin|g" "$1" > "$2"; }
fill "$here/camless.desktop" "$data/applications/camless.desktop"
cp "$here/camless.png" "$data/icons/hicolor/256x256/apps/camless.png"
# So it is there after the download is gone.
cp "$here/uninstall.sh" "$data/camless/uninstall.sh"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true

# The udev rules, when they are new or changed. udev applies them to the devices
# plugged in now too, though liquidctl finds that a hidraw node can keep its old
# rights until the device is plugged in again or the PC restarts. Run as root, with
# the rules file and where it goes.
apply='install -D -m 644 "$1" "$2" || exit 1
if ! { udevadm control --reload && udevadm trigger --action=change --subsystem-match=hidraw --subsystem-match=usb; }; then
  echo "udev did not reload its rules; they apply after a restart, or when the device is plugged in again." >&2
fi'
if ! cmp -s "$here/71-camless.rules" "$rules"; then
  if $update; then
    # No terminal to ask in: pkexec asks through the desktop, if it is there.
    if ! { command -v pkexec >/dev/null 2>&1 && pkexec sh -c "$apply" sh "$here/71-camless.rules" "$rules"; }; then
      echo "The udev rules changed and were not installed. Run install.sh from the download to install them." >&2
    fi
  else
    echo "Installing the udev rules in $rules; sudo asks for your password."
    sudo sh -c "$apply" sh "$here/71-camless.rules" "$rules"
    echo "If camless cannot open a device yet, plug it in again or restart."
  fi
fi

# The service. Without a systemd user session the app starts it when it opens.
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  mkdir -p "$config/systemd/user"
  fill "$here/camless.service" "$config/systemd/user/camless.service"
  systemctl --user daemon-reload
  if $update; then
    systemctl --user try-restart camless.service || true
  else
    systemctl --user enable camless.service >/dev/null 2>&1
    systemctl --user restart camless.service
    echo "The camless service runs, and starts when you log in: systemctl --user status camless"
  fi
else
  # The app started the old service; it starts the new one when it opens. SIGTERM
  # lets the old one set the fans to their safe speed first.
  $update && { pkill -TERM -x -u "$(id -u)" camless || true; }
  echo "No systemd user session, so the app starts the service when it opens."
fi

if $update; then
  # The app that started this opens again, as the setup does on Windows, once the old
  # one has gone: while it lives, a new one would hand its window over to it and end.
  n=0
  while pgrep -x -u "$(id -u)" camless-gui >/dev/null 2>&1 && [ $n -lt 50 ]; do
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
