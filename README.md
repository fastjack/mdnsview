# mdnsview

A terminal browser for mDNS / DNS-SD devices and services on the local network.

## Usage

```sh
cargo build --release
./target/release/mdnsview
./target/release/mdnsview --config ./my-config.toml
```

On Windows, run `.\target\release\mdnsview.exe` in Windows Terminal or a
console window.

The screen has three panes: **devices** on the left, the selected device's
**services** in the middle, and the selected service's **details** on the
right. Everything refreshes in the background. Each device shows how long ago
it was last seen; devices that stop answering are dimmed and disappear once
they have not been seen for the configured timeout (5 minutes by default). The
entry you have selected stays visible, marked as gone, until you move away
from it.

| Key | Action |
|-----|--------|
| `↑` `↓` / `k` `j` | Move within the focused pane (scroll in the details pane) |
| `←` `→` / `h` `l` | Move focus between panes (`enter` also moves right) |
| `PgUp` `PgDn` | Move by ten entries |
| `tab` | Switch the left pane between devices and service types |
| `/` | Filter the left pane (`enter` applies, `esc` clears) |
| `d` | Switch the details pane between the simple view and raw records |
| `m` | Turn mouse capture off / on |
| `q` / `ctrl-c` | Quit |

With the mouse, click a row to select it (and focus its pane), click the
details pane to focus it, and use the scroll wheel over any pane: it moves the
selection in the two lists and scrolls the details. The status bar shows
`mouse on` or `mouse off`.

While the mouse is captured, your terminal's own click-and-drag text selection
is unavailable. Press `m` to release the mouse and copy text (most terminals
also allow selecting with `Shift` held), then `m` again to get mouse control
back. Set `mouse = false` in the configuration to start with the mouse
released.

The raw records view lists every record mDNS reported for the service and its
host, with remaining and original TTL, cache-flush bit, source address,
interface, which source reported it (`system`, `wire` or `both`) and when it
was first and last seen.

### One device, several host names

Some machines advertise under more than one host name. For example, a Mac may
use `Hyperion.local` for most services and a generated name for a management
service. `mdnsview` shows such host names as **one device** when they have the
same name (ignoring case) **and** share at least one IP address. The device
lists the services of all its host names, and the service details show a
`Device` line with every host name. Devices with the same name but no common
address stay separate. If the host names stop sharing an address, the device
splits again.

### One service name, several machines

Some software announces the same service name on every machine it runs on.
For example, TeamViewer on two Windows PCs can both announce a service named
`0`. `mdnsview` shows such a service **once per machine**, each under its own
device with its own port, addresses and TXT data. The service details show a
`Shared name` line listing the other host names that announce the same name.

### How discovery works

`mdnsview` combines two sources:

- **The OS mDNS daemon** for a quick first overview: avahi on Linux (over
  D-Bus), mDNSResponder on macOS (through the system `dns_sd` API), and the
  DNS Client service on Windows (through the Windows DNS-SD API).
- **Its own multicast socket** on UDP port 5353, IPv4 and IPv6. It listens to
  all mDNS traffic and sends its own queries to fill in details and to check
  that known hosts are still there.

No root or administrator rights are needed. The socket shares port 5353 with
the daemon (`SO_REUSEADDR` / `SO_REUSEPORT`), so both keep working. With both
sources active, the status bar shows `avahi + wire`, `mDNSResponder + wire` or
`Windows DNS + wire`. If the daemon is not running, it shows `wire only`; if
the port cannot be shared, it shows `system only`. If neither works,
`mdnsview` exits with an error naming both problems.

Logs are written to `~/.local/state/mdnsview/mdnsview.log` on Linux,
`~/Library/Caches/mdnsview/mdnsview.log` on macOS and
`%LOCALAPPDATA%\mdnsview\cache\mdnsview.log` on Windows; set
`RUST_LOG=mdnsview=debug` for more detail.

### macOS

`mdnsview` runs on macOS as well as on Linux and Windows. On macOS:

- When `mdnsview` is started from a terminal, macOS does not apply its
  **Local Network** privacy setting to it, so nothing needs to be allowed.
  If it is ever started in a way where the setting does apply and access is
  denied, the status bar shows `(Local Network access blocked)` and only
  mDNSResponder results appear; allow access in **System Settings › Privacy
  & Security › Local Network**.
- AWDL, tunnel and similar links (`awdl*`, `llw*`, `utun*`, `anpi*`) are
  skipped by default.
- The configuration file is
  `~/Library/Application Support/mdnsview/config.toml`.

### Windows

`mdnsview` runs on Windows 10 and 11, next to Windows' own DNS Client
service. On Windows:

- **Windows Firewall** allows incoming mDNS only for the DNS Client service.
  The first start of `mdnsview.exe` may show a firewall prompt. Without an
  allow rule (including when the prompt is cancelled, which adds a block
  rule), Windows still lets in the answers to `mdnsview`'s own queries, but
  drops announcements that other hosts send unprompted. Windows DNS still
  reports new and departing services right away; only their wire records
  (with TTL, source address and interface) wait for the next query. To let
  all mDNS in, run this in an elevated PowerShell:

  ```powershell
  New-NetFirewallRule -DisplayName "mdnsview (mDNS-In)" -Direction Inbound `
    -Protocol UDP -LocalPort 5353 -Action Allow `
    -Program "C:\path\to\mdnsview.exe"
  ```

  If a firewall drops the answers too, so that no other host is heard while
  Windows DNS lists services, the status bar shows
  `(no replies: check Windows Firewall)`. `mdnsview` never changes firewall
  settings itself.
- **Interface names** are the connection names Windows shows, such as `WLAN`,
  `Ethernet 2` or `vEthernet (WSL)`. They are localized and can be renamed;
  `Get-NetAdapter -IncludeHidden` lists them (Hyper-V and WSL switches are
  hidden without `-IncludeHidden`). To skip those switches and a VPN:

  ```toml
  [interfaces]
  exclude = ["vEthernet (*", "Tailscale"]
  ```
- The configuration file is `%APPDATA%\mdnsview\config\config.toml`, and the
  log is `%LOCALAPPDATA%\mdnsview\cache\mdnsview.log`.

## Configuration

`mdnsview` reads `config.toml` from the platform configuration directory:
`$XDG_CONFIG_HOME/mdnsview/config.toml` (usually `~/.config/mdnsview/config.toml`)
on Linux, `~/Library/Application Support/mdnsview/config.toml` on macOS, and
`%APPDATA%\mdnsview\config\config.toml` on Windows. Use `--config <path>` to read
a different file. Without a file, the
defaults below apply. Every setting is optional; unknown keys and invalid values
are reported and the program exits before starting.

```toml
# Remove a device or service once it has not been seen for this long.
# Human-readable duration: "90s", "5m", "1h".
device_timeout = "5m"

# Listen and query over IPv6 as well as IPv4.
ipv6 = true

# Capture the mouse at startup (click to select, wheel to scroll).
# Press `m` to toggle it while running.
mouse = true

[interfaces]
# Glob patterns of interfaces to use. Empty = every interface that is up,
# multicast-capable and not loopback.
include = []
# Glob patterns of interfaces to skip. Setting this replaces the default list,
# which skips container veth pairs and macOS links that carry no LAN mDNS
# (AWDL, tunnels).
exclude = ["veth*", "awdl*", "llw*", "utun*", "anpi*"]
```
