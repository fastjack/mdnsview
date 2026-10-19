# mdnsview

A terminal browser for mDNS / DNS-SD devices and services on the local network.

## Usage

```sh
cargo build --release
./target/release/mdnsview
./target/release/mdnsview --config ./my-config.toml
```

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
| `←` `→` / `h` `l` | Move focus between panes |
| `PgUp` `PgDn` | Move by ten entries |
| `tab` | Switch the left pane between devices and service types |
| `/` | Filter the left pane (`enter` applies, `esc` clears) |
| `d` | Switch the details pane between the simple view and raw records |
| `q` / `ctrl-c` | Quit |

The raw records view lists every record mDNS reported for the service and its
host, with remaining and original TTL, cache-flush bit, source address,
interface, which source reported it (`system`, `wire` or `both`) and when it
was first and last seen.

### How discovery works

`mdnsview` combines two sources:

- **The OS mDNS daemon** (avahi on Linux, over D-Bus) for a quick first
  overview.
- **Its own multicast socket** on UDP port 5353, IPv4 and IPv6. It listens to
  all mDNS traffic and sends its own queries to fill in details and to check
  that known hosts are still there.

No root privileges are needed. The socket shares port 5353 with avahi
(`SO_REUSEADDR` / `SO_REUSEPORT`), so both keep working. If avahi is not
running, the status bar shows `wire only`; if the port cannot be shared, it
shows `system only`. If neither works, `mdnsview` exits with an error naming
both problems.

Logs are written to `~/.local/state/mdnsview/mdnsview.log`; set
`RUST_LOG=mdnsview=debug` for more detail.

## Configuration

`mdnsview` reads `config.toml` from the platform configuration directory
(`$XDG_CONFIG_HOME/mdnsview/config.toml`, usually `~/.config/mdnsview/config.toml`
on Linux). Use `--config <path>` to read a different file. Without a file, the
defaults below apply. Every setting is optional; unknown keys and invalid values
are reported and the program exits before starting.

```toml
# Remove a device or service once it has not been seen for this long.
# Human-readable duration: "90s", "5m", "1h".
device_timeout = "5m"

# Listen and query over IPv6 as well as IPv4.
ipv6 = true

[interfaces]
# Glob patterns of interfaces to use. Empty = every interface that is up,
# multicast-capable and not loopback.
include = []
# Glob patterns of interfaces to skip. Setting this replaces the default list.
exclude = ["veth*"]
```
