## v0.2.2

**Changes `--delete-safe` to prune the uv cache with `uv cache prune` instead of skipping it, and adds a prune option to `--delete`.**

### Changed

- **`--delete-safe` prunes the uv cache**: `--delete-safe` now runs `uv cache prune`, which drops only entries nothing references, and reports how much it freed. The cache is no longer held back (an intermediate approach that treated it as review-carefully was replaced), so a bulk sweep no longer forces a full re-download on the next `uvx` run or new virtualenv.
  - The prune sets `UV_LOCK_TIMEOUT` because uv otherwise waits on a held cache for as long as the holder runs. A busy cache is reported as skipped.
  - `--force` is never passed, because prune also removes cached environments that a running `uvx` tool may be executing from.
  - `--delete` offers three choices for the uv cache: prune, delete all, or skip.
  - The `--delete-safe` help text mentions the uv prune.

### Internal

- Modeled per-category cleanup policy as a `Sweep` (clean by removal, clean by prune, or review) matched exhaustively, so a new category must choose a policy and a review-carefully category cannot reach the cleaner.
