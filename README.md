# diskscout

A disk-space scanner and cleaner for developer machines, on macOS, Linux and Windows.

Storage panes tell you that "System Data" is 400 GB. They do not tell you that 54 GB of it
is Instruments recordings nothing ever prunes, or that 53 GB is simulator clones your test
runs left behind. diskscout finds where the space actually went, groups it by what owns it,
and deletes the parts that are safe to delete.

    diskscout                 # scan and report, delete nothing
    diskscout --delete-safe   # delete every regenerable category, one prompt

The interesting problem is not finding big directories. It is knowing which ones you can
remove without breaking something, and reporting a number that matches what deleting will
actually give back. Most of this codebase is about those two things.

## Install

    cargo run --manifest-path /path/to/diskscout -- __bootstrap

The hidden `__bootstrap` subcommand runs `cargo install --path . --force`, writes shell
completions for your shell (zsh, bash or fish detected from `$SHELL`, PowerShell on
Windows), patches the rc file or `$PROFILE` to load them, and records `DISKSCOUT_SRC` so
later runs skip source discovery.

    __bootstrap --only-completions    # refresh completions without rebuilding
    __bootstrap --debug               # debug profile, faster link

Requires Rust 1.95 or newer (edition 2024). The only dependencies are `walkdir`, `rayon`,
`clap`, `colored` and `indicatif`.

## Use

    diskscout                          # scan $HOME, report, delete nothing
    diskscout --path ~/Developer       # scan one subtree instead
    diskscout --delete                 # walk categories, prompt for each
    diskscout --delete-safe            # delete every regenerable category, one prompt
    diskscout --delete-safe -y         # ...without the confirmation
    diskscout --delete-targets         # delete every Rust target/ dir, no prompts

| Flag | Default | What it does |
| --- | --- | --- |
| `--path <DIR>` | `$HOME` | Scan this subtree instead of the home directory. |
| `--delete` | off | Prompt per category after the report. |
| `--delete-safe` | off | Delete every category marked regenerable, one confirmation. |
| `--delete-targets` | off | Delete every Rust `target/` directory, no prompts. |
| `-y`, `--yes` | off | Answer yes to the `--delete-safe` confirmation. |
| `--except <CAT>` | none | Categories `--delete-safe` must leave alone. Comma-separated. |
| `--old-after <DAYS>` | 180 | Age before a file is reported as stale. |
| `--large-over <MB>` | 100 | Size before a file is reported as large. |
| `--no-system` | off | Skip OS caches, app caches and other fixed system locations. |

Scanning never writes anything. Deleting only happens behind one of the three delete flags.

### What gets scanned

The walk covers your home directory, not the whole disk. That is deliberate: outside `$HOME`
almost everything matching a build-artifact pattern belongs to a package manager rather than
to you. A Homebrew formula's `node_modules` is the CLI that formula installed, and a conda
prefix's `__pycache__` belongs to the interpreter it shipped, so those prefixes are skipped
and the report points at `brew cleanup` and `conda clean --all` instead. `/System`, `/dev`
and the OS caches are skipped for the same reason: they are not yours to delete.

`--path` overrides that. Naming a root explicitly is a request, so a default skip that
contains it is dropped rather than pruning the scan on its own root, which is what lets
`--path /Volumes/disk` reach an external drive. Skips that sit *inside* the requested root
still apply, so `--path /` still leaves `/System` alone.

### Holding categories back

Every line of the `--delete-safe` preview prints the slug to exclude it:

    About to delete these categories:
      Instruments traces        54.4 GB  (10 dirs, --except instruments-traces)
      macOS Caches              36.4 GB  (193 dirs, --except macos-caches)
      App web caches            18.6 GB  (123 dirs, --except app-web-caches)
      TOTAL                    269.4 GB

    diskscout --delete-safe --except instruments-traces,macos-caches

Naming a category this machine does not have is a no-op, so one fixed `--except` set works
across machines. Naming something that is not a category at all is rejected rather than
silently ignored, because a typo would otherwise delete the thing you meant to keep.

## What it finds

Forty-eight categories in six sections. A section whose categories are all empty is not
printed, so each machine sees only what applies to it.

**Build artifacts** are found by walking, and a directory only counts when the marker that
explains it sits beside it: `target/` next to a `Cargo.toml`, `build/` next to a
`build.gradle`, `bin/` and `obj/` next to a `.csproj`. Covers Rust, `node_modules`,
`__pycache__`, virtualenvs, Next.js, Nuxt, SvelteKit, CocoaPods, SwiftPM `.build/`, .NET,
Visual Studio `.vs/`, Gradle `build/`, `.gradle/` and NDK `.cxx/`. A matched directory is
not descended into, so a 4 GB `node_modules` is one entry rather than 30,000.

**macOS and Xcode** covers DerivedData wherever it lives, XCTest simulator clones, the
simulators and their runtimes, per-OS device support, the documentation cache, Instruments
recordings, `~/Library/Caches` broken out one entry per app, and iPhone backups.

**App data and caches** covers the Chromium cache directories every Electron app and
browser profile carries, sandboxed apps' caches inside their own containers, the local
mirror a cloud provider keeps, and a small set whose contents cost hours of CPU rather than
a download to rebuild.

**Windows system** covers `%TEMP%`, Windows Update and servicing leftovers, `Windows.old`,
the recycle bin on every fixed drive, crash dumps, shell and shader caches, the thumbnail
cache, the installer payload cache and per-profile browser caches.

**Package and tool caches** covers npm, yarn, pnpm and bun, Homebrew, uv, pip, NuGet, the
Cargo registry, Gradle, Maven, Go modules and IDE caches.

**VMs, SDKs and containers** covers Docker and WSL data, the Android SDK and emulator
images.

On top of the categories the report lists large files, files untouched for six months,
forgotten disk images and installers, and the large directories that matched no pattern at
all. Those are reported only and never deleted.

Some of this lives outside the tree the walk covers and is measured directly: Instruments
keeps every recording it has ever taken under `/var/folders`, simulator runtimes live under
`/Library`, and the Windows recycle bins are one per drive. Those are reported even when
`--path` narrows the scan. `--no-system` drops them.

Where deleting files is the wrong way to reclaim space, the report prints the command that
is right instead of offering to delete: `rustup toolchain uninstall` for extra toolchains,
`xcrun simctl runtime delete` for simulator runtimes, `powercfg /h off` for `hiberfil.sys`.

## Reading the report

The terminal report truncates long lists to stay readable. The full untruncated listing is
written on every run to `/tmp/diskscout-report.txt`, or `%TEMP%\diskscout-report.txt` on
Windows. It is worth reading before a first delete: it is the only place you see all 193
cache directories rather than the top eight.

## Deleting

Deletion is permanent. Not the Trash, no undo.

Thirty-two of the forty-eight categories are tagged **safe to delete**: regenerable on
demand, costing a rebuild or a re-download but never data. `--delete-safe` takes exactly
these.

The other sixteen are tagged **review carefully** and `--delete-safe` never touches them,
whatever `--except` says. Virtualenvs, framework build directories, DerivedData, simulators
and their runtimes, iPhone backups, the cloud mirror, the caches that cost hours to rebuild,
`Windows.old`, the installer payload cache still needed to repair or uninstall, Docker and
WSL data, the Android SDK and emulator images. They are still measured and still reported,
because knowing where the space went is useful even when removing it is your call.

Directories the OS and running apps expect to keep existing are emptied rather than removed:
`%TEMP%`, the browser caches, an app's own cache directory. A cache with open handles
deletes partially, and the report says how much actually came back rather than assuming the
whole figure did.

## What it refuses to delete

This is the part that matters, and every rule here exists because the alternative broke
something real.

**Anything inside a `.app` bundle.** A JetBrains IDE ships its plugins' dependencies as
`node_modules` and its embedded Python's bytecode as `__pycache__`. Both match a build
artifact pattern exactly. Removing either breaks the bundle's code signature rather than
freeing a cache.

**A package manager's global prefix.** `~/.bun/install/global/node_modules` is a
`node_modules` tree like any other, but it backs every binary in `~/.bun/bin`. Deleting it
uninstalls them. Same for npm's global prefix and editor extension directories, which are
installed software wearing a dependency tree's clothes. The sibling `install/cache` is a
real cache and stays deletable.

**A cloud provider's sync state.** What looks like a cache inside a File Provider
extension's container is the provider's record of the local mirror. Clearing Google Drive's
made it treat every synced file as unknown, re-download the whole account, and quarantine
what it could not reconcile. On macOS the containers belonging to iCloud, OneDrive, Drive
and anything installed later are identified from `pluginkit`, the registry macOS itself
keeps, so no vendor list is involved; if that lookup gives no answer, no container is swept
at all. Windows has no equivalent, and the registry's sync-root list misses providers that
ship their own virtual filesystem, so there the paths are named.

**Anywhere a placeholder lives.** `~/Library/CloudStorage` and `~/Library/Mobile Documents`
are where every cloud account is mounted. Reading a placeholder downloads the file it
stands for, so walking those would pull entire accounts onto the disk the tool is supposed
to be freeing.

**Group containers, and Store app data on Windows.** A messaging app keeps the only copy of
every photo it ever received in its group container, and a podcast app keeps downloaded
episodes in a directory it named `Cache`. On Windows, `LocalCache` under `Packages` is
documented as storage the app manages itself, not the purgeable directory beside it.

**Caches that cost more than a download.** Photo and media analysis run over the whole
library, so clearing them buys a few hundred megabytes and spends hours of CPU re-deriving
what was already there. Siri's downloaded voices, offline map tiles, Spotify's offline
playlists and Office's co-authoring state are the same trade in bandwidth. Reported, never
swept. An IDE index is deliberately not in that set: rebuilding it is exactly the trade the
sweep exists to make.

`~/Library/Caches` is regenerable by definition and the sweep takes all of it, so the few
entries where that is untrue are named individually rather than assumed.

## How sizes are measured

The number has to match what deleting actually returns, which rules out the obvious
implementations.

**Sparse files** advertise the size they could grow to. A `Docker.raw` or a simulator image
reports 64 GB while occupying 2. On Unix the figure is `st_blocks * 512`, capped at the
logical length so block slack cannot push an ordinary file above what Windows would report
for the same file. On Windows it is `GetCompressedFileSizeW`, which also handles
NTFS-compressed files, and the common case pays only an attribute test.

**Cloud placeholders** carry their full logical size in metadata while the bytes sit in the
cloud. A OneDrive Files On-Demand stub is skipped rather than counted.

**Hard links** are counted once. Cargo and pnpm link one blob under many names, and counting
each name separately inflated a 57 GB cargo cache to 76 GB. Deduplication is per directory
walk, so a blob linked from two different targets is still counted twice.

**Nested targets** are dropped. A sandboxed app's cache sits inside its own container, and
both were targets, so both were sized. Only the outermost of an overlap survives.

Known limitation: APFS transparent-compression stores file data in an extended attribute, so
`st_blocks` reads zero and those files under-report. `du` behaves the same way, and
under-reporting is the safe direction for a tool that promises reclaimable space.

## Platform differences

The scanner, report and cleaner contain no `cfg` blocks. Everything that differs lives in
`src/platform.rs` and is reached through one function per question: where is home, where is
temp, which roots must the walker not descend into, which fixed OS directories exist, how
big is this file really.

| | macOS and Linux | Windows |
| --- | --- | --- |
| On-disk size | `st_blocks * 512` | `GetCompressedFileSizeW` |
| Cloud placeholders | not detected | `FILE_ATTRIBUTE_OFFLINE` and recall flags |
| Hard links | counted once | counted per link, no stable file index in std |
| Sync roots | `pluginkit` File Provider registry | named paths, no registry query |
| Report path | `/tmp` | `%TEMP%` |
| Completions | zsh, bash, fish | PowerShell |

Read-only file handling also differs: a permission denial on Windows is usually
`FILE_ATTRIBUTE_READONLY`, which is worth clearing once before retrying. On Unix the same
denial is an ownership problem that clearing a bit cannot fix, and doing it anyway would
widen the mode for everyone, so the retry only exists on Windows.

## Development

    scripts/ci-check.sh

A shim over `cex ci-check --rust`: formatting, clippy with `-D warnings`, tests, doc and a
dependency audit. Run it before every commit.

Windows is not covered by that gate, so check it explicitly when touching `platform.rs`:

    cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings

Forty-four tests cover directory classification and the marker files it requires, the size
accounting including the sparse and hard-link cases, category slugs and their uniqueness,
the deletion policy, and each refusal rule above. The rules that exist because something
broke are pinned by a test naming the failure, so a future change that reintroduces the bug
fails rather than ships.

### Layout

    src/main.rs        CLI surface, wiring only
    src/scanner.rs     the walk, the Category enum and its policy, size accounting
    src/report.rs      terminal report, and the section list every other module walks
    src/cleaner.rs     the three delete modes
    src/dump.rs        the untruncated report file
    src/bootstrap.rs   install and shell completions
    src/platform.rs    every OS difference, and nothing else

`report::SECTION_CATS` is the single list of categories in display order. The cleaner walks
it too, so a category cannot be reported without being deletable or the reverse.

`Category` carries its own policy: `safe_to_delete()` for whether `--delete-safe` may take
it, `delete_contents_only()` for whether the directory itself must survive, and `slug()`
derived from the display label so the CLI name cannot drift from what the report prints.
Adding a category means adding a variant, a label, and a line in `SECTION_CATS`; the
compiler finds the rest.
