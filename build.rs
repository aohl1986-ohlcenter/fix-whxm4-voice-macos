//! On macOS, builds the Swift RFCOMM helper that `src/bluetooth_macos.rs` runs.

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=macos/xm4-rfcomm.swift");
    println!("cargo:rerun-if-changed=macos/Info.plist");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = format!("{}/xm4-rfcomm", env::var("OUT_DIR").unwrap());
    let swiftc = Command::new("swiftc")
        .args(["-O", "-o", &out, "macos/xm4-rfcomm.swift"])
        .args([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__info_plist",
        ])
        .args(["-Xlinker", "macos/Info.plist"])
        .status()
        .expect("cannot run swiftc; install the Xcode command line tools");
    assert!(swiftc.success(), "swiftc failed");
    // A stable identifier lets macOS remember the Bluetooth permission across rebuilds.
    let codesign = Command::new("codesign")
        .args(["-s", "-", "-f", "-i", "fix-whxm4-voice.xm4-rfcomm", &out])
        .status()
        .expect("cannot run codesign");
    assert!(codesign.success(), "codesign failed");
}
