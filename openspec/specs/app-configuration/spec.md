# app-configuration Specification

## Purpose

Lets users, especially advanced users, override the application's defaults through a TOML configuration file in the platform's standard configuration directory.

## Requirements

### Requirement: Configuration file location
The system SHALL read its configuration from `config.toml` in the platform configuration directory for `mdnsview`. On Linux this is `$XDG_CONFIG_HOME/mdnsview/config.toml`, falling back to `~/.config/mdnsview/config.toml`. On macOS this is `~/Library/Application Support/mdnsview/config.toml`. On Windows this is `%APPDATA%\mdnsview\config\config.toml`. A `--config <path>` command-line option SHALL override the location.

#### Scenario: Default location on Linux
- **WHEN** `XDG_CONFIG_HOME` is unset and `~/.config/mdnsview/config.toml` exists
- **THEN** its settings are applied

#### Scenario: Default location on macOS
- **WHEN** the application runs on macOS and `~/Library/Application Support/mdnsview/config.toml` exists
- **THEN** its settings are applied

#### Scenario: Default location on Windows
- **WHEN** the application runs on Windows and `%APPDATA%\mdnsview\config\config.toml` exists
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

### Requirement: Supported settings
The configuration SHALL support:
- `device_timeout`: a human-readable duration such as `"5m"` or `"90s"`
- `interfaces.include`: a list of interface-name glob patterns; empty means automatic selection
- `interfaces.exclude`: a list of interface-name glob patterns
- `ipv6`: a boolean
- `mouse`: a boolean; whether mouse capture is on at startup

Settings that are omitted SHALL keep their defaults. Setting `interfaces.exclude` SHALL replace the default exclude list.

#### Scenario: Partial file
- **WHEN** the file contains only `device_timeout = "10m"`
- **THEN** the device timeout is 10 minutes, and all other settings use their defaults

#### Scenario: Custom exclude list
- **WHEN** the file sets `interfaces.exclude = ["docker*", "veth*"]`
- **THEN** interfaces matching either pattern are not used

#### Scenario: Mouse disabled
- **WHEN** the file sets `mouse = false`
- **THEN** the application starts with mouse capture off

### Requirement: Invalid configuration
If the configuration file cannot be parsed, or contains an invalid value, the system SHALL exit before starting the UI. The non-zero exit SHALL come with an error message that names the file and the offending setting.

#### Scenario: Bad duration
- **WHEN** the file contains `device_timeout = "five minutes"`
- **THEN** the application exits with an error naming `device_timeout` and the file path

#### Scenario: Unknown key
- **WHEN** the file contains a key the application does not recognise
- **THEN** the application exits with an error naming the unknown key
