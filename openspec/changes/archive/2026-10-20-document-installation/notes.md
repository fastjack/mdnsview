# Verification notes

## README checks (tasks 1.1, 1.2)

- `cargo test readme_example_parses` passes. The new section adds only `sh` blocks, so the first `toml` block is still the configuration example.
- All 20 lines in the `sh` blocks of "Installation" and "Usage" pass both `zsh -n` and `bash -n`.

## Documented commands on macOS (task 2.1)

Checked on macOS 27.2 (arm64) with Homebrew Rust 1.99.0, following the section with `--root` pointing at a scratch directory instead of `~/.cargo`:

- **Install.** `cargo install --path . --locked --root <scratch>` printed `Installing <scratch>/bin/mdnsview`, and `<scratch>/bin/mdnsview --version` printed `mdnsview 0.1.0`. Cargo also warned that `<scratch>/bin` should be added to `PATH`.
- **`PATH`.** In a fresh `zsh -f -c`:
  - with `<scratch>/bin` prepended, `command -v mdnsview` printed `<scratch>/bin/mdnsview`
  - without it, `command -v` printed nothing
- **Updating.** Running the same install command again printed `Replacing …` / `Replaced package …` without `--force`.
- **Uninstalling.** `cargo uninstall mdnsview --root <scratch>` printed `Removing <scratch>/bin/mdnsview` and left `bin/` empty. Cargo's bookkeeping files `.crates.toml` and `.crates2.json` stay in the root.
- **Nothing was installed into the real `~/.cargo/bin`.** It is on `PATH` on this Mac.

## Documented install on Linux (task 2.2)

Checked in Docker `rust:latest` (Rust 1.99.1, aarch64 Debian). The image sets `CARGO_HOME=/usr/local/cargo`. The checkout was mounted read-only and copied to `/work`, then `target/` was removed.

- **`$CARGO_HOME` wording confirmed.** `cargo install --path . --locked` printed `Installing /usr/local/cargo/bin/mdnsview`, which is `$CARGO_HOME/bin`.
- **Found in a normal shell.** In a non-login `bash -c`, `command -v mdnsview` printed `/usr/local/cargo/bin/mdnsview`, and `mdnsview --version` printed `mdnsview 0.1.0`.
- **Not found in a login shell.** In `bash -l`, Debian's `/etc/profile` replaces the `PATH` that Docker set, which drops `/usr/local/cargo/bin`. This is the case the README handles by adding `~/.cargo/bin` to `PATH` in the shell's startup file. The README needs no change.

## Windows note (added after archiving)

- **Why it was added.** After this change was archived, `add-windows-support` landed on `origin/master`, and the commit was rebased onto it without conflicts.
- **What it says.** A short "On Windows" paragraph at the end of "Installation":
  - the same `cargo install --path . --locked`, run in PowerShell
  - it installs `%USERPROFILE%\.cargo\bin\mdnsview.exe`, which the rustup installer puts on the user `PATH`
  - check with `Get-Command mdnsview` in a new terminal
  - the `PATH` line, `rehash` and `sudo install` steps do not apply
- **Not verified.** It describes standard cargo and rustup behavior on Windows and was not checked on a Windows machine.
