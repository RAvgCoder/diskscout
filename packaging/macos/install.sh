#!/bin/bash
# Copies diskscout into /usr/local/bin, which is already on every Mac's PATH.
# Touches no shell config.
#
#   bash install.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="${DISKSCOUT_BIN_DIR:-/usr/local/bin}"
installed="${bin_dir}/diskscout"

die() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

[ "$(uname -s)" = Darwin ] || die "this build is for macOS only"
# uname -m reports x86_64 inside a Rosetta terminal even on an M1.
[ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ] ||
    die "this build needs an Apple silicon Mac (M1 or later)"

[ -f "${here}/diskscout" ] || die "the diskscout binary is not next to this script"
(cd "$here" && shasum -a 256 -c SHA256SUMS >/dev/null 2>&1) ||
    die "the binary does not match its checksum; the copy is damaged, get the zip again"

sudo=""
if ! mkdir -p "$bin_dir" 2>/dev/null || [ ! -w "$bin_dir" ]; then
    printf 'Installing to %s needs your Mac password.\n' "$bin_dir"
    sudo="sudo"
fi

$sudo mkdir -p "$bin_dir"
$sudo cp -X "${here}/diskscout" "$installed"
$sudo chmod 0755 "$installed"
# AirDrop marks the file as downloaded, and macOS will not run a downloaded
# binary that Apple has not notarized.
$sudo xattr -d com.apple.quarantine "$installed" 2>/dev/null || true

"$installed" --version >/dev/null 2>&1 ||
    die "installed, but macOS refused to run it; open System Settings > Privacy & Security and allow diskscout"

printf '\n%s installed to %s\n\n' "$("$installed" --version)" "$installed"
printf '  diskscout                 scan and report, deletes nothing\n'
printf '  diskscout --delete-safe   delete what the report marks safe, permanently\n'
