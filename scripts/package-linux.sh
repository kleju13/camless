#!/bin/sh
# Builds camless and camless-gui for Linux and packs them with linux/ into
# dist/camless-linux-x86_64.tar.gz, next to its SHA-256 in the .sha256 file. CI runs it
# for every pull request, and release.yml for every tag; a tag such as v0.2.0 as the
# first argument fails unless every crate is at version 0.2.0.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
tag=${1:-}

# The app compares its own version with the latest release, so all three crates move together.
version_of() { sed -n 's/^version = "\(.*\)"/\1/p' "$1" | head -1; }
version=$(version_of "$root/Cargo.toml")
for crate in core gui; do
  v=$(version_of "$root/$crate/Cargo.toml")
  [ "$v" = "$version" ] || { echo "the crates are at different versions: $version, $crate $v" >&2; exit 1; }
done
if [ -n "$tag" ] && [ "$tag" != "v$version" ]; then
  echo "tag $tag does not match version $version in Cargo.toml" >&2
  exit 1
fi

cargo build --release --workspace --manifest-path "$root/Cargo.toml"
target=${CARGO_TARGET_DIR:-$root/target}/release

name=camless-linux-x86_64.tar.gz
stage=$root/dist/linux/camless
rm -rf "$root/dist/linux"
mkdir -p "$stage"
cp "$target/camless" "$target/camless-gui" "$stage/"
cp "$root/linux/install.sh" "$root/linux/uninstall.sh" "$root/linux/71-camless.rules" "$root/linux/72-camless-fans.rules" \
  "$root/linux/camless.service" "$root/linux/camless.desktop" "$root/linux/camless.png" "$stage/"
cp "$root/LICENSE" "$stage/"
chmod 755 "$stage/camless" "$stage/camless-gui" "$stage/install.sh" "$stage/uninstall.sh"
echo "$version" > "$stage/VERSION"

# The same bytes from the same files: no owner, no times from the build machine.
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$root/dist/linux" -cf - camless | gzip -n9 > "$root/dist/$name"
(cd "$root/dist" && sha256sum "$name" > "$name.sha256")
echo "Built $root/dist/$name (camless $version)"
