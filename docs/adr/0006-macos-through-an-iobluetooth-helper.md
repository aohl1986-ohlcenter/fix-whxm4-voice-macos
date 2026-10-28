# Reach the headphones on macOS through a small IOBluetooth helper process

macOS has no BlueZ, and its Bluetooth API, IOBluetooth, is an Objective-C framework that delivers
RFCOMM data through delegate callbacks on a run loop. Rather than drive that from Rust, a Swift helper
(`macos/xm4-rfcomm.swift`) does two jobs: `list` prints the connected devices that offer an SDP UUID,
and `open` opens one RFCOMM channel by SDP UUID and copies bytes between it and its stdin/stdout.
`src/bluetooth_macos.rs` runs one helper per channel and offers the same `Headphones`/`Channel`
interface as the BlueZ module, so nothing above the `Transport` seam changes. `build.rs` compiles the
helper with `swiftc` on macOS only; the BlueZ crates became Linux-only dependencies.

Two macOS rules shape the helper. It must carry an `NSBluetoothAlwaysUsageDescription`, so an
Info.plist is linked into its `__TEXT,__info_plist` section and it is ad-hoc signed with a fixed
identifier, which lets macOS remember the permission across rebuilds. And macOS asks the *responsible*
process for Bluetooth access, which is the terminal or app that started the tool; if that one has no
usage description, the helper is killed. So the helper starts itself again with the private but
long-standing `responsibility_spawnattrs_setdisclaim`, becoming its own responsible process, and
macOS asks the user about `xm4-rfcomm` instead.

Writes are split into pieces no larger than the channel's MTU. Closing the helper's stdin closes the
channel. The alternatives were Rust bindings to IOBluetooth through `objc2`, which needs a run loop on
a thread the tool does not own, and a Linux VM, which on Apple Silicon has no Bluetooth unless a USB
adapter is passed through.
