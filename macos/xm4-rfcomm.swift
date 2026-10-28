// A small bridge between the tool and one RFCOMM channel on macOS, where there is no BlueZ.
//
//   xm4-rfcomm list <uuid>            prints "address<TAB>name" for each connected device offering <uuid>
//   xm4-rfcomm open <address> <uuid>  opens the channel with SDP UUID <uuid>, then copies stdin to the
//                                     channel and the channel to stdout until either side closes
//
// Status goes to stderr: "ready ..." once the channel is open, "error: ..." before a failing exit.

import Foundation
import IOBluetooth

func status(_ message: String) {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
}

func fail(_ message: String) -> Never {
    status("error: " + message)
    exit(1)
}

func hex(_ code: IOReturn) -> String { String(format: "0x%08x", UInt32(bitPattern: code)) }

func sdpUUID(_ text: String) -> IOBluetoothSDPUUID {
    guard let uuid = UUID(uuidString: text) else { fail("not a UUID: \(text)") }
    var bytes = uuid.uuid
    return withUnsafeBytes(of: &bytes) { IOBluetoothSDPUUID(bytes: $0.baseAddress!, length: 16) }
}

/// Runs the run loop until `done` or the deadline, so IOBluetooth can deliver its callbacks.
func spin(seconds: TimeInterval, until done: () -> Bool) {
    let deadline = Date(timeIntervalSinceNow: seconds)
    while !done() && Date() < deadline {
        RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
    }
}

final class SDPQuery: NSObject {
    var done = false
    @objc func sdpQueryComplete(_ device: IOBluetoothDevice!, status: IOReturn) { done = true }
}

/// The service record for `uuid`, from the cache or, failing that, from a fresh SDP query.
func serviceRecord(_ device: IOBluetoothDevice, _ uuid: IOBluetoothSDPUUID) -> IOBluetoothSDPServiceRecord? {
    if let record = device.getServiceRecord(for: uuid) { return record }
    let query = SDPQuery()
    guard device.performSDPQuery(query, uuids: [uuid]) == kIOReturnSuccess else { return nil }
    spin(seconds: 10) { query.done }
    return device.getServiceRecord(for: uuid)
}

final class Bridge: NSObject, IOBluetoothRFCOMMChannelDelegate {
    func rfcommChannelData(_ channel: IOBluetoothRFCOMMChannel!, data: UnsafeMutableRawPointer!, length: Int) {
        FileHandle.standardOutput.write(Data(bytes: data, count: length))
    }

    func rfcommChannelClosed(_ channel: IOBluetoothRFCOMMChannel!) {
        status("closed")
        exit(0)
    }
}

/// Writes `data` in pieces no larger than the channel's MTU.
func write(_ channel: IOBluetoothRFCOMMChannel, _ data: Data) {
    var bytes = [UInt8](data)
    let mtu = max(1, Int(channel.getMTU()))
    var offset = 0
    while offset < bytes.count {
        let length = min(mtu, bytes.count - offset)
        let code = bytes.withUnsafeMutableBytes {
            channel.writeSync($0.baseAddress! + offset, length: UInt16(length))
        }
        if code != kIOReturnSuccess { fail("write failed: \(hex(code))") }
        offset += length
    }
}

@_silgen_name("responsibility_spawnattrs_setdisclaim")
func responsibility_spawnattrs_setdisclaim(
    _ attrs: UnsafeMutablePointer<posix_spawnattr_t?>, _ disclaim: Int32
) -> Int32

/// macOS asks the process responsible for us (the terminal or app that started the tool) for
/// Bluetooth access, and kills us if that one has no usage description. So we run ourselves again as
/// our own responsible process, with stdio inherited, and macOS asks about this helper instead.
func runDisclaimed() -> Never {
    var attrs: posix_spawnattr_t?
    posix_spawnattr_init(&attrs)
    _ = responsibility_spawnattrs_setdisclaim(&attrs, 1)
    setenv("XM4_RFCOMM_DISCLAIMED", "1", 1)
    guard let path = Bundle.main.executablePath else { fail("cannot find own executable") }
    var pid: pid_t = 0
    let code = posix_spawn(&pid, path, nil, &attrs, CommandLine.unsafeArgv, environ)
    if code != 0 { fail("cannot start helper: \(String(cString: strerror(code)))") }
    var waitStatus: Int32 = 0
    while waitpid(pid, &waitStatus, 0) == -1 && errno == EINTR {}
    exit(waitStatus & 0x7f == 0 ? (waitStatus >> 8) & 0xff : 1)
}

if ProcessInfo.processInfo.environment["XM4_RFCOMM_DISCLAIMED"] == nil { runDisclaimed() }

let args = CommandLine.arguments
switch (args.count, args.count > 1 ? args[1] : "") {
case (3, "list"):
    let uuid = sdpUUID(args[2])
    let devices = (IOBluetoothDevice.pairedDevices() ?? []).compactMap { $0 as? IOBluetoothDevice }
    for device in devices where device.isConnected() && serviceRecord(device, uuid) != nil {
        print("\(device.addressString ?? "")\t\(device.name ?? "")")
    }

case (4, "open"):
    guard let device = IOBluetoothDevice(addressString: args[2]) else { fail("no device \(args[2])") }
    guard let record = serviceRecord(device, sdpUUID(args[3])) else {
        fail("the headphones do not offer \(args[3])")
    }
    var channelID: BluetoothRFCOMMChannelID = 0
    let lookup = record.getRFCOMMChannelID(&channelID)
    if lookup != kIOReturnSuccess { fail("no RFCOMM channel in the service record: \(hex(lookup))") }

    let bridge = Bridge()
    var opened: IOBluetoothRFCOMMChannel?
    let code = device.openRFCOMMChannelSync(&opened, withChannelID: channelID, delegate: bridge)
    guard code == kIOReturnSuccess, let channel = opened else {
        fail("cannot open RFCOMM channel \(channelID): \(hex(code))")
    }
    status("ready channel=\(channelID) mtu=\(channel.getMTU())")

    Thread.detachNewThread {
        while true {
            let data = FileHandle.standardInput.availableData
            if data.isEmpty {
                DispatchQueue.main.async {
                    channel.close()
                    exit(0)
                }
                return
            }
            DispatchQueue.main.sync { write(channel, data) }
        }
    }
    withExtendedLifetime(bridge) { RunLoop.main.run() }

default:
    fail("usage: xm4-rfcomm list <uuid> | open <address> <uuid>")
}
