#!/bin/sh
# Removes what install.sh put in place, and what camless keeps besides the settings:
# the app's and the LCD browser's data in ~/.local/share/camless, where install.sh
# leaves a copy of this script, the last update in ~/.cache/camless and its log in
# ~/.local/state/camless. The settings, the history and the game sessions in
# ~/.config/camless stay; delete that folder to remove them. Only the udev rules need
# root; sudo asks for it.
#
# CAMLESS_BIN_DIR names the folder install.sh was given, if it was.
set -eu

if [ "$(id -u)" = 0 ]; then
  echo "Run this as yourself, not as root: it removes camless for the user who runs it." >&2
  exit 1
fi

bin=${CAMLESS_BIN_DIR:-$HOME/.local/bin}
data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}
cache=${XDG_CACHE_HOME:-$HOME/.cache}
state=${XDG_STATE_HOME:-$HOME/.local/state}
rules=/etc/udev/rules.d/71-camless.rules

# The app first, so it cannot start the service again; Exit in its menu does the same.
pkill -x -u "$(id -u)" camless-gui 2>/dev/null || true
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  # Stopping sets the fans to their safe speed and the LCD back to its own screen.
  systemctl --user disable --now camless.service 2>/dev/null || true
  # Where install.sh put it: where this user's systemd looks.
  units=$(systemctl --user show-environment | sed -n 's/^XDG_CONFIG_HOME=//p')
  rm -f "${units:-$HOME/.config}/systemd/user/camless.service"
  systemctl --user daemon-reload
fi
# A service the app started outside systemd. SIGTERM lets it set the fans safe.
pkill -TERM -x -u "$(id -u)" camless 2>/dev/null || true

rm -f "$bin/camless" "$bin/camless-gui" \
  "$data/applications/camless.desktop" \
  "$data/icons/hicolor/256x256/apps/camless.png" \
  "$config/autostart/camless.desktop" \
  "$data/camless/uninstall.sh"
rm -rf "$data/camless/webkit" "$data/camless/browser" "$HOME/snap/chromium/common/camless-browser" \
  "$cache/camless" "$state/camless"
rmdir "$data/camless" 2>/dev/null || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" || true

if [ -e "$rules" ]; then
  echo "Removing the udev rules in $rules; sudo asks for your password."
  sudo sh -c 'rm -f "$1" && { udevadm control --reload || true; }' sh "$rules"
fi
echo "camless is removed. Its settings are still in $config/camless."
