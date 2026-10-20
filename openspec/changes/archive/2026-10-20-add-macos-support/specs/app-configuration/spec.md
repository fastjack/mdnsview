# Spec Delta

## MODIFIED Requirements

### Requirement: Configuration file location
The system SHALL read its configuration from `config.toml` in the platform configuration directory for `mdnsview`. On Linux this is `$XDG_CONFIG_HOME/mdnsview/config.toml`, falling back to `~/.config/mdnsview/config.toml`. On macOS this is `~/Library/Application Support/mdnsview/config.toml`. A `--config <path>` command-line option SHALL override the location.

#### Scenario: Default location on Linux
- **WHEN** `XDG_CONFIG_HOME` is unset and `~/.config/mdnsview/config.toml` exists
- **THEN** its settings are applied

#### Scenario: Default location on macOS
- **WHEN** the application runs on macOS and `~/Library/Application Support/mdnsview/config.toml` exists
- **THEN** its settings are applied

#### Scenario: Explicit path
- **WHEN** the application is started with `--config /tmp/test.toml`
- **THEN** settings are read from `/tmp/test.toml` only

### Requirement: Defaults without a file
When no configuration file exists, the system SHALL run with these defaults:
- device timeout of 5 minutes
- automatic interface selection with `veth*`, `awdl*`, `llw*`, `utun*` and `anpi*` excluded
- IPv6 enabled
- mouse capture enabled

#### Scenario: No config file
- **WHEN** no configuration file exists
- **THEN** the application starts and uses the defaults
