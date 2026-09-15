#!/bin/bash
# Builds a standalone Apple silicon binary and zips it with an installer, for
# someone who has neither the source nor a Rust toolchain.
#
#   scripts/package-macos.sh      ->  dist/diskscout-<version>-macos-arm64.zip
set -euo pipefail

cd "$(dirname "$0")/.."

target=aarch64-apple-darwin
version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)"
name="diskscout-${version}-macos-arm64"

die() {
    printf 'package-macos: %s\n' "$1" >&2
    exit 1
}

# The build machine can be a newer chip than the Mac this ships to, so pin the
# M1 instruction set; RUSTFLAGS also overrides any target-cpu=native in a cargo
# config. 11.0 is the oldest macOS any Apple silicon Mac shipped with.
export RUSTFLAGS="-C target-cpu=apple-m1 --remap-path-prefix=${HOME}=~"
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add "$target" >/dev/null 2>&1

bin="$(cargo build --release --locked --target "$target" --message-format=json |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -n 1)"
[ -n "$bin" ] || die "the build produced no binary"

stage="$(mktemp -d)/${name}"
mkdir -p "$stage"
install -m 0755 "$bin" "${stage}/diskscout"
# Apple silicon kills unsigned code outright. An ad-hoc signature is enough to
# run; it does not satisfy Gatekeeper, which is what install.sh handles.
codesign --force --sign - "${stage}/diskscout"

[ "$(lipo -archs "${stage}/diskscout")" = arm64 ] || die "binary is not arm64-only"
[ "$(vtool -show-build "${stage}/diskscout" | awk '/minos/ {print $2}')" = 11.0 ] ||
    die "binary requires a newer macOS than 11.0"
# A library from outside the OS, Homebrew's say, loads here and is missing there.
if otool -L "${stage}/diskscout" | tail -n +2 | grep -vqE '^[[:space:]]+/(usr/lib|System/Library)/'; then
    die "binary links a library that is not part of macOS"
fi
codesign --verify --strict "${stage}/diskscout" || die "signature does not verify"

install -m 0755 packaging/macos/install.sh "${stage}/install.sh"
install -m 0644 packaging/macos/README.txt "${stage}/README.txt"
(cd "$stage" && shasum -a 256 diskscout >SHA256SUMS)

mkdir -p dist
# Without the two flags, ditto stores this machine's extended attributes in the
# zip as ._ AppleDouble entries.
ditto -c -k --norsrc --noextattr --keepParent "$stage" "dist/${name}.zip.part"
mv -f "dist/${name}.zip.part" "dist/${name}.zip"

printf '%s\n' "dist/${name}.zip"
