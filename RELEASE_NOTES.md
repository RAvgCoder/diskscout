## v0.2.0

**Adds a standalone Apple-silicon packaging script, moves browser caches to review-only on both platforms, and fixes false-positive deletions of nvm's npm and Service Worker data.**

### Breaking

- **Browser caches no longer auto-deleted; macOS now scans them too**: `BrowserCache` was Windows-only and classified as safe to auto-delete. It's now `review-carefully` on both platforms, and Chrome, Edge, Firefox, Safari, and Opera caches are all detected and excluded from automatic deletion.
  - Before: `--delete-safe` removed `BrowserCache` (Windows only)
  - After: browser caches are reported but require manual review, on macOS and Windows alike
- **Installed binary moved from `~/.local/bin` to `/usr/local/bin`**: `install.sh` no longer writes to `.zshrc`, since `/etc/paths` already puts `/usr/local/bin` on every Mac's `PATH`. Avoids the ~27ms shell-startup cost the old completion setup added.
  - Before: installed to `~/.local/bin`, appended PATH and a `compinit` completion block to `.zshrc`
  - After: installs to `/usr/local/bin`, no dotfiles touched, may prompt for `sudo` on directories that aren't already writable

### Added

- **Standalone Apple silicon package (`scripts/package-macos.sh`)**: builds an arm64-only, ad-hoc-signed binary bundle with an installer and README for a Mac that has neither the repo nor a Rust toolchain.
  - Pinned to the M1 CPU baseline and macOS 11.0 minimum, and remaps build-machine paths out of the binary.
  - Refuses to package a binary that isn't arm64-only, needs a newer macOS, or links a library from outside the OS (e.g. a Homebrew dylib).
  - `install.sh` clears the AirDrop quarantine flag after a checksum check, needs no password, and runs on the system bash 3.2.

### Fixed

- **Chrome and Opera caches escaped browser-cache protections**: Chrome's cache sits inside the shared `~/Library/Caches/Google` vendor folder next to Android Studio, so deleting "Google" as one target would have taken Chrome with it; that folder is now split per-app so Android Studio's cache is still removed but Chrome's is kept. Opera on Windows lives under `%APPDATA%`, outside the named browser sweep, and is now caught by the Electron sweep instead.
- **`nvm`'s npm and corepack no longer offered for deletion**: `~/.nvm/versions/node/vX/lib/node_modules` (npm and corepack) was flagged as deletable build output. Runtime version manager roots — nvm, fnm, Volta, nodenv, asdf, rbenv, mise, and pyenv — are now excluded from the walk entirely.
- **Service Worker registrations preserved during cache cleanup**: only the `CacheStorage` subdirectory is deleted now; the sibling `Database` directory, which holds push-notification and offline-app registrations, was previously swept along with it on macOS. Brings macOS in line with existing Windows behavior.
- **Four macOS-only safety guards added on Windows**: Scoop installs, home-directory conda, `%LOCALAPPDATA%\Programs`, and OneDrive sync roots (matching the existing `~/Library/CloudStorage` guard) are now excluded from deletion the same way their macOS counterparts already were.
