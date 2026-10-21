# Tasks

## 1. README

- [x] 1.1 Add an "Installation" section before "Usage" in `README.md`, with the content listed in proposal.md "What Changes":
  - install with `--locked` and why
  - `~/.cargo/bin` and `$CARGO_HOME/bin`
  - checking `PATH` with `command -v`, adding `~/.cargo/bin` to `~/.zshrc` or `~/.bashrc`, and `rehash` / `hash -r`
  - `--root ~/.local`
  - a system-wide install with `sudo install`
  - updating by re-running, and uninstalling with `cargo uninstall`

  Verify that `cargo test readme_example_parses` still passes and that the section's shell snippets are syntactically valid: run each in `zsh -n` and `bash -n`.
- [x] 1.2 Change "Usage" to start with the installed `mdnsview` command, and keep `cargo build --release` / `./target/release/mdnsview` as the way to run without installing. Verify that every command and path mentioned in "Usage" also appears in, or matches, the Installation section.

## 2. Check the documented commands

- [x] 2.1 On this Mac, follow the Installation section literally, but with `--root` set to a scratch directory instead of `~/.cargo`. Check that:
  - `--root` installs `<root>/bin/mdnsview`, and `<root>/bin/mdnsview --version` prints `mdnsview 0.1.0`
  - with `<root>/bin` prepended to `PATH` in a fresh `zsh -c`, `command -v mdnsview` finds it
  - running the install command again replaces the binary without `--force`
  - `cargo uninstall mdnsview --root <root>` removes it

  Record the output in `notes.md`.
- [x] 2.2 In Docker (`rust:latest`), with the checkout mounted read-only and copied to a writable directory, run the documented `cargo install --path . --locked` as the image's user. Check that the binary lands in `$CARGO_HOME/bin`, which the image sets to `/usr/local/cargo`, and that `command -v mdnsview` finds it in `bash`. This confirms the `$CARGO_HOME` wording. Record the result in `notes.md`.

## Workflow follow-up

- Archive the change after review with `/opsx:archive`.
