# Spec Delta

## MODIFIED Requirements

### Requirement: Status bar
The UI SHALL show a status bar with:
- the active discovery sources (for example "avahi + wire", "wire only", "system only")
- the number of active interfaces
- the number of listed devices
- whether mouse capture is on or off
- a short key hint

When discovery reports that Windows Firewall seems to block incoming mDNS, the sources entry SHALL carry a hint naming Windows Firewall.

#### Scenario: Avahi missing
- **WHEN** the OS daemon is unavailable
- **THEN** the status bar shows "wire only"

#### Scenario: Mouse state shown
- **WHEN** the user turns mouse capture off with `m`
- **THEN** the status bar shows that the mouse is off

#### Scenario: Firewall hint shown
- **WHEN** on Windows discovery reports that Windows Firewall seems to block incoming mDNS
- **THEN** the status bar shows "Windows DNS + wire (no replies: check Windows Firewall)"

#### Scenario: Firewall hint cleared
- **WHEN** the firewall report is withdrawn
- **THEN** the status bar shows "Windows DNS + wire" without the hint
