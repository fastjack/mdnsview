# Proposal

## Why

The README only shows `cargo build --release` and running `./target/release/mdnsview` from the checkout. Nothing explains how to install the binary so that typing `mdnsview` works in any shell. Rust's answer to `make install` is `cargo install --path .`, but that has pitfalls a newcomer won't guess:
- **It ignores `Cargo.lock` unless given `--locked`.**
- **Its target directory may not be on `PATH`.** It installs into `~/.cargo/bin`, which rustup adds to `PATH` but Homebrew's or a distribution's Rust may not.

## What Changes

- **New "Installation" section** in the README, before "Usage", covering:
  - **Install:** `cargo install --path . --locked`. It installs `mdnsview` into `~/.cargo/bin`, or `$CARGO_HOME/bin` if that is set. Explain why `--locked` matters.
  - **Making sure the shell finds it:** check with `command -v mdnsview`. If `~/.cargo/bin` is missing from `PATH`, as with Homebrew's or a distribution's Rust, add it in `~/.zshrc` or `~/.bashrc`. After installing, run `rehash` in an already-open zsh session, or `hash -r` in bash, so that session finds the new command.
  - **A different prefix**, like `make install PREFIX=…`: `cargo install --path . --locked --root ~/.local` installs into `~/.local/bin`.
  - **System-wide install:** build as the normal user, then `sudo install -m 755 target/release/mdnsview /usr/local/bin/`. Running cargo as root would leave root-owned files in the checkout and in `~/.cargo`.
  - **Updating:** run the same `cargo install` command again. For a path install it always rebuilds and replaces the binary, so no `--force` is needed.
  - **Uninstalling:** `cargo uninstall mdnsview`, with the same `--root` if one was used. For the system-wide copy, remove the file.
- **"Usage" section:** start with `mdnsview` as an installed command, and keep `./target/release/mdnsview` as the way to run from the checkout without installing.

Out of scope: packages (Homebrew formula, `.deb`/`.rpm`), publishing to crates.io (`cargo install mdnsview` without `--path`), shell completions, man pages, and Windows. The Windows instructions belong with the separate, unfinished `add-windows-support` work on its own branch.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This changes documentation only, so the change sets `skip_specs: true`.

## Impact

- **Files:** `README.md` only. No code, configuration or dependency changes.
- **The test `readme_example_parses`** (`src/config.rs`) reads the first ```` ```toml ```` block in the README. The new section adds `sh` blocks only, so that test is unaffected; the tasks confirm it.
