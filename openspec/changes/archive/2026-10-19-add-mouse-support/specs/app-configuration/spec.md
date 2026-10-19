# Spec Delta

## MODIFIED Requirements

### Requirement: Defaults without a file
When no configuration file exists, the system SHALL run with these defaults:
- device timeout of 5 minutes
- automatic interface selection with `veth*` excluded
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
