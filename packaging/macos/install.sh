#!/bin/bash
# Installs diskscout for the current user: no password, no source code, no Rust.
#
#   bash install.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="${HOME}/.local/bin"
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

mkdir -p "$bin_dir"
cp -X "${here}/diskscout" "$installed"
chmod 0755 "$installed"
# AirDrop marks the file as downloaded, and macOS will not run a downloaded
# binary that Apple has not notarized.
xattr -d com.apple.quarantine "$installed" 2>/dev/null || true

"$installed" --version >/dev/null 2>&1 ||
    die "installed, but macOS refused to run it; open System Settings > Privacy & Security and allow diskscout"

case "$(basename "${SHELL:-/bin/zsh}")" in
    zsh) rc="${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) rc="${HOME}/.bash_profile" ;;
    *) rc="" ;;
esac

case ":${PATH}:" in
    *":${bin_dir}:"*) ;;
    *)
        if [ -z "$rc" ]; then
            printf 'Add %s to your PATH to run diskscout by name.\n' "$bin_dir"
        elif ! grep -qF '# diskscout install' "$rc" 2>/dev/null; then
            printf '\n# diskscout install\nexport PATH="$HOME/.local/bin:$PATH"\n' >>"$rc"
        fi
        ;;
esac

"$installed" __bootstrap --only-completions >/dev/null 2>&1 ||
    printf 'note: tab completion was not set up; diskscout itself is installed\n'

printf '\n%s installed to %s\n\n' "$("$installed" --version)" "$installed"
printf 'Open a new Terminal window, then:\n'
printf '  diskscout                 scan and report, deletes nothing\n'
printf '  diskscout --delete-safe   delete what the report marks safe, permanently\n'
