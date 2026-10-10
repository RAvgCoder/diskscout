## v0.2.1

**Fixes `__bootstrap` adding a redundant `compinit` to every zsh startup by probing zsh first and editing `.zshenv`/`.zshrc` only when completions are not already loaded.**

### Fixed

- **`__bootstrap` added a second `compinit` to zsh startup**: The zsh path appended its own `fpath` line and `compinit` to `.zshrc` behind a `# diskscout completions` marker. Behind oh-my-zsh or any other `compinit`, this forced a second full completion-cache rebuild on every shell start, and deleting the block brought it back on the next install. Bootstrap now asks a fresh `zsh -i` whether `~/.zsh/completions` already loads and edits only when it does not.
  - The `fpath` line goes into `~/.zshenv`.
  - A `compinit` is added to `.zshrc` only when nothing else runs one.
  - The old marker block is removed as an exact match once a probe has succeeded. If zsh cannot be run, nothing is edited.
