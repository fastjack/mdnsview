# Proposal

## Why

`mdnsview` runs on Linux and macOS, but on Windows it does not compile. `cargo check` on Windows 11 (x86_64-pc-windows-msvc) stops with 10 errors: `nix`, `std::os::fd`, `tokio::io::unix`, `set_reuse_port` and `libc::localtime_r` are Unix-only. Windows PCs are common on LANs, and Windows ships its own mDNS responder (the DNS Client service). So the tool should run there too, with the same two sources as on the other platforms.

## What Changes

- The application builds and runs on Windows 10 and 11, as well as on Linux and macOS.
- **The multicast socket works on Windows.**
  - It shares UDP port 5353 with the DNS Client service and other mDNS software, such as browsers.
  - It joins the groups by interface and reports the receiving interface of each packet.
  - Interfaces are listed by their Windows names, such as `WLAN`, `Ethernet` or `vEthernet (WSL)`, so include and exclude patterns use those names.
- **New OS daemon backend for Windows: the native Windows DNS-SD API** (`DnsServiceBrowse` / `DnsServiceResolve` in `dnsapi.dll`), served by the DNS Client service.
  - It browses service types and instances and resolves host, port, TXT data and addresses.
  - The status bar names it `Windows DNS`.
  - If the API is missing or the service is stopped, the system source is reported unavailable, and `mdnsview` continues with the wire source only.
- **Windows Firewall hint.** By default, the firewall allows incoming mDNS only for the DNS Client service. For `mdnsview.exe`, it lets in traffic that follows its own queries and drops unsolicited announcements, so new devices and departures show up later until the user allows incoming mDNS.
  - As a safeguard for setups that also drop replies, the status bar shows a hint naming Windows Firewall when the wire source hears nothing from any other host while Windows DNS has found services.
  - The hint disappears once a reply arrives.
  - `mdnsview` never changes firewall settings itself. The README shows the command that adds an allow rule.
- On Windows, configuration and logs use the platform locations: `%APPDATA%\mdnsview\config\config.toml` and `%LOCALAPPDATA%\mdnsview\cache\mdnsview.log`.
- The Unix-only dependency `nix` becomes Unix-only in `Cargo.toml`. Windows uses `windows-sys`.
- The README documents Windows: paths, the Windows DNS source, interface names and the firewall rule.

Out of scope:
- installers, packages (winget, Scoop), and signed binaries
- Apple's Bonjour for Windows (`dnssd.dll`) as a backend
- terminals older than Windows Terminal / conhost on Windows 10
- adding firewall rules automatically

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mdns-discovery`:
  - OS daemon discovery names the Windows DNS-SD API as the supported daemon on Windows.
  - Multicast listening covers sharing the port with the Windows DNS Client service.
  - Interface selection is specified for Windows interface names.
  - A new requirement covers the Windows Firewall hint.
- `app-configuration`: the configuration file location is specified for Windows.
- `browser-ui`: the status bar requirement adds the firewall hint.

## Impact

- **Code:**
  - `src/wire/sys.rs` becomes the Unix implementation. A new Windows implementation provides the same `list_interfaces` / `MdnsSocket` API, using `GetAdaptersAddresses`, `WSARecvMsg` with `IP_PKTINFO` / `IPV6_PKTINFO`, and tokio's `UdpSocket` readiness.
  - `src/backend/`: new `windns.rs` backend, which `backend::platform()` selects on Windows.
  - `src/wire/backend.rs`: firewall-silence tracking, and the macOS `EHOSTUNREACH` check behind `cfg`.
  - `src/model/` and `src/ui/view.rs`: a status flag for the hint, and local-time formatting that works on Windows.
- **Dependencies:**
  - `nix` moves to `[target.'cfg(unix)'.dependencies]`. `libc` stays shared: it builds on Windows and provides `localtime_s`.
  - `windows-sys` 0.61, already in `Cargo.lock` through `socket2`, is added for Windows with the networking, DNS and IP-helper features.
- **Docs:** README sections on usage, discovery, configuration and a new Windows section.
- **Linux and macOS:** behavior is unchanged. The code moves into `cfg(unix)` modules.
