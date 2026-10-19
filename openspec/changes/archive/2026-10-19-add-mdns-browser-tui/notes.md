# Verification notes

Manual checks run on 2026-10-19 on the development machine: Arch Linux, avahi-daemon 0.9-rc5, Rust 1.98.1. Interfaces: Wi-Fi, a USB Ethernet interface and tailscale0.

## Real LAN (office network, roughly 25 devices)

- Started as an unprivileged user while avahi-daemon was running: status `avahi + wire │ 3 interfaces`.
  - About 23 devices were listed within 6 s.
  - Instances first shown under "(unresolved)" moved to their hosts within about 20 s, once refinement queries were answered.
- Raw view on a Brother printer showed PTR/SRV/TXT/A records with `ttl x/y`, `shared` / `cache-flush`, `via both`, source address, interface and first/last-seen times.
- `tab` pivot kept the selected service selected (IPP type → same instance). `/printer` narrowed the list to the 4 printers.
- `q` exits with status 0 and restores the terminal. A forced panic (scratch build with a panic on F12) also left the alternate screen and printed the panic message.
- `avahi-publish -s … _http._tcp 8080`:
  - Seen by the wire backend on wlp5s0 and tailscale0 over IPv4 and IPv6, including the TTL-0 goodbye when the publish stopped.
  - The avahi backend reported it with host, port and TXT, then sent one `SystemRemoved` after its last interface/protocol scope was removed.

## Degraded modes

- D-Bus unreachable (`DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent`): `wire only`, 19 devices.
- Port 5353 held exclusively by another socket, inside `unshare --map-user=$UID -n`: `system only`, 24 devices from avahi.
- Both unavailable: exit status 1. The error names both the bind failure and the D-Bus failure.

## Interface changes (network namespace with a dummy interface)

- dummy0 taken down, then brought up 3 s later with a new address: the multicast group was rejoined 4 s after the interface came up, within the 10 s requirement.
- Not covered: actual Wi-Fi disconnect/reconnect on the real interface. The same detection path was exercised with the dummy interface instead.

## Silent disappearance and retention (network namespace, `device_timeout = "30s"`)

- Three test responders (alpha, bravo, charlie) appeared as live devices. Alpha was selected; alpha and bravo were killed with SIGKILL, so no goodbye was sent.
- After about 42 s:
  - bravo had been removed
  - alpha was still listed as `gone 46s`, with its details still shown
  - charlie stayed live (`5s`), because it answered the liveness refresh queries
- Moving the selection to charlie removed alpha immediately.

## Observations for follow-up

- Device labels from `_device-info` `model=` (e.g. `Mac14,10`) were less friendly than instance names and repeated across devices. Resolved: the label order now puts the most common instance name first, and uses the model only for devices without services.
- Our own host's packets arrive on several interfaces (lo, LAN, tailscale0), so the source address of our host's records changes depending on which copy was received last.
