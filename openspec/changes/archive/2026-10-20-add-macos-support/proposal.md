# Proposal

## Why

`mdnsview` has only been built and tested on Linux. On macOS it does not compile (`set_multicast_all_v4` is Linux-only), and even after that fix it would run wire-only, because the system daemon source exists only for avahi. Macs are also among the busiest mDNS devices on a typical LAN, so the tool should run on them too.

## What Changes

- The application builds and runs on macOS, as well as on Linux.
- New OS daemon backend for macOS: **mDNSResponder**, used through the system `dns_sd` API in libSystem. It browses service types and instances and resolves host, port, TXT data and addresses, the same way the avahi backend does on Linux. The status bar names it `mDNSResponder`.
- The multicast socket works on macOS. It shares UDP port 5353 with mDNSResponder, joins the groups by interface, and reports the receiving interface of each packet.
- The default interface exclude list also skips macOS interfaces that are not useful for LAN discovery: `awdl*`, `llw*`, `utun*` and `anpi*`, as well as `veth*`. A user-set `exclude` list still replaces the default.
- If macOS's Local Network privacy setting blocks the multicast socket, the wire source is reported with a reason that names the Local Network permission. No unexplained failure. This is a safeguard: macOS does not apply the setting to programs started from a terminal.
- On macOS, configuration and logs use the platform locations: `~/Library/Application Support/mdnsview/config.toml` and `~/Library/Caches/mdnsview/mdnsview.log`.
- The D-Bus dependency (`zbus`) becomes Linux-only.
- The README documents macOS: paths, the mDNSResponder source and the Local Network permission.

Out of scope: Windows and other BSDs, packaging (Homebrew, signed or notarized binaries), and macOS-specific UI changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mdns-discovery`: OS daemon discovery names mDNSResponder as the supported daemon on macOS. The default interface exclude list adds the macOS link types. Degraded operation covers a blocked Local Network permission.
- `app-configuration`: the configuration file location is specified for macOS. The default exclude list in the defaults requirement changes to match the discovery change.

## Impact

- **Code:**
  - `src/wire/sys.rs`: Linux-only socket options are put behind `cfg`. Packet-info and the multicast join are checked on macOS.
  - `src/backend/`: new `dnssd.rs` backend (FFI to `dns_sd.h`), and `backend::platform()` selects it on macOS.
  - `src/config.rs`: default exclude list.
  - `src/wire/backend.rs` / `src/main.rs`: wording for a socket blocked by the Local Network permission.
- **Dependencies:** `zbus` moves to `[target.'cfg(target_os = "linux")'.dependencies]`. No new crates: the `dns_sd` functions are in libSystem and are declared by hand.
- **Docs:** README sections on usage, discovery and configuration.
- **Linux:** behavior is unchanged except for the longer default exclude list. Its new patterns do not match usual Linux interface names.
