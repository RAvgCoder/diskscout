## v0.3.0

**Adds temp-folder build-output scanning and Claude Code session reporting, makes DerivedData safely deletable while keeping its Swift packages, and holds back download-only caches for review.**

### Breaking

- **Download-only caches now require review instead of being auto-deletable**: `node_modules` and the npm, yarn, pnpm, bun, pip, NuGet, Cargo, Gradle, Maven, Go and Homebrew caches, plus Xcode's caches, moved to review-only. A machine cleaned offline has to still be able to build, so anything only a download can restore is no longer removed by default. Scripts that relied on these being cleaned without review must now go through the review path.
  - Before: these caches were cleaned as ordinary deletable categories
  - After: these caches are reported as review-only categories

### Added

- **Build output in `/tmp` and Claude Code's temp folder**: `/tmp` and Claude Code's per-user temp folder are now walked for build output, which becomes its own deletable category.
  - Build output is held back while anything in it was written in the last hour.
  - Cargo targets are found by `.rustc_info.json` and DerivedData by its layout, under any name, so `~/.build_caches/Xcode` is now picked up.
  - On Windows, the `%TEMP%` sweep leaves Claude Code's folder alone.
- **Claude Code session folders reported per session**: What remains of each session folder is reported per session and deleted only through `--delete`, once the session is no longer registered as running and has been idle for an hour.
- **Swift packages kept as a review-only category**: DerivedData's `SourcePackages` folder is reported on its own, because only the network brings packages back, so a trimmed DerivedData still resolves its packages offline.
- **Relocated DerivedData support**: Xcode's own DerivedData setting is read, so a relocated root is one target rather than a scatter of project folders.

### Changed

- **DerivedData is now safe to delete**: Losing it costs Xcode only a clean build and a reindex, both local. Deletion keeps `SourcePackages` and removes everything else.
- **SwiftPM `.build` is trimmed like DerivedData**: Its checkouts, repositories, artifacts and workspace state are kept, since those are downloaded.
- **`/tmp` is scanned like any other path**: Build output under `/tmp` keeps the category it would have anywhere else instead of a temp-only one, so the scan treats `/tmp` exactly as if it had been passed with `--path`. Claude Code session folders remain their own review-only category.
- **Unexplored-directories list no longer repeats reported paths**: Directories already reported inside a category are omitted from the list.

### Fixed

- **`.DS_Store` skewing folder age**: Finder rewrites `.DS_Store` whenever a folder is viewed, so it made folders look recently used. It is no longer used to date a folder.

### Internal

- Replaced the contents-only flag with a `Removal` type (whole, contents, or everything but the packages), and clearing now recurses around a kept folder instead of keeping the whole branch that holds it.
- Changed the prune lock timeout to a `Duration`.
