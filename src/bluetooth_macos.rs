//! The real device link on macOS: RFCOMM channels to the headphones through IOBluetooth.
//!
//! macOS has no BlueZ, and IOBluetooth is an Objective-C framework that needs a run loop. So a small
//! Swift helper (`macos/xm4-rfcomm.swift`, built by `build.rs`) opens each channel and copies bytes
//! between it and its stdio. This module offers the same interface as the BlueZ one.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::link::{Transport, hex};

/// An SDP service UUID.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Uuid(u128);

impl Uuid {
    pub const fn from_u128(value: u128) -> Self {
        Uuid(value)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = format!("{:032x}", self.0);
        write!(
            f,
            "{}-{}-{}-{}-{}",
            &s[0..8],
            &s[8..12],
            &s[12..16],
            &s[16..20],
            &s[20..32]
        )
    }
}

/// Sony's MDR control channel, `Serial HPC` in SDP.
pub const MDR_UUID: Uuid = Uuid::from_u128(0x96cc203e_5068_46ad_b32d_e316f5e069ba);
/// Airoha's RACE channel, `Airoha_APP` in SDP.
pub const RACE_UUID: Uuid = Uuid::from_u128(0x8901dfa8_5c7e_4d8f_9f0c_c2b70683f5f0);

/// How long the helper may take to open a channel.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// The helper: the one `XM4_RFCOMM` names, else the one next to this program (as in a release
/// archive), else the one `build.rs` built.
fn helper() -> PathBuf {
    if let Some(path) = std::env::var_os("XM4_RFCOMM") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("xm4-rfcomm"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(concat!(env!("OUT_DIR"), "/xm4-rfcomm")))
}

/// Connected headphones.
pub struct Headphones {
    pub name: String,
    pub address: String,
}

impl Headphones {
    /// Finds the headphones among the connected devices: the one that offers Sony's MDR channel.
    pub fn find() -> io::Result<Self> {
        let output = Command::new(helper())
            .args(["list", &MDR_UUID.to_string()])
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| {
                io::Error::new(e.kind(), format!("cannot run {}: {e}", helper().display()))
            })?;
        if !output.status.success() {
            return Err(io::Error::other("cannot list Bluetooth devices"));
        }
        let mut found: Vec<Headphones> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(address, name)| Headphones {
                name: name.to_string(),
                address: address.replace('-', ":").to_uppercase(),
            })
            .collect();
        match found.len() {
            0 => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no headphones found\n\
                 Connect the WH-1000XM4 to this Mac first.",
            )),
            1 => Ok(found.pop().unwrap()),
            _ => {
                let names: Vec<String> = found
                    .iter()
                    .map(|h| format!("\n  {} ({})", h.name, h.address))
                    .collect();
                Err(io::Error::other(format!(
                    "several connected devices offer Sony's control channel:{}\n\
                     Disconnect all but the WH-1000XM4.",
                    names.concat()
                )))
            }
        }
    }

    /// Opens the channel with SDP UUID `uuid`, `label` naming it in errors and logs. With
    /// `verbose`, every byte sent and received is logged to stderr.
    pub fn open(&self, uuid: Uuid, label: &'static str, verbose: bool) -> io::Result<Channel> {
        let opening =
            |e: String| io::Error::other(format!("cannot open the {label} channel ({uuid}): {e}"));
        let mut child = Command::new(helper())
            .args(["open", &self.address, &uuid.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| opening(e.to_string()))?;

        // The helper reports on stderr: "ready ..." once open, or "error: ..." before it exits.
        let (ready_tx, ready_rx) = mpsc::channel();
        let stderr = BufReader::new(child.stderr.take().unwrap());
        thread::spawn(move || {
            let mut ready = Some(ready_tx);
            for line in stderr.lines().map_while(Result::ok) {
                match ready.take() {
                    Some(tx) => {
                        let _ = tx.send(line);
                    }
                    None if verbose || line.starts_with("error") => {
                        eprintln!("{label} helper: {line}")
                    }
                    None => {}
                }
            }
        });
        let line = match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{label} channel did not open in time"),
                ));
            }
            Err(RecvTimeoutError::Disconnected) => "error: the helper exited".to_string(),
        };
        if !line.starts_with("ready") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(opening(line.trim_start_matches("error: ").to_string()));
        }
        if verbose {
            eprintln!("{label} helper: {line}");
        }

        let (data_tx, data_rx) = mpsc::channel();
        let mut stdout = child.stdout.take().unwrap();
        thread::spawn(move || {
            let mut buffer = vec![0; 1024];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if data_tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Channel {
            label,
            verbose,
            stdin: Some(child.stdin.take().unwrap()),
            data: data_rx,
            child,
        })
    }
}

/// One RFCOMM channel to the headphones, held open by a helper process.
pub struct Channel {
    label: &'static str,
    verbose: bool,
    stdin: Option<ChildStdin>,
    data: Receiver<Vec<u8>>,
    child: Child,
}

impl Transport for Channel {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.verbose {
            eprintln!("{} > {}", self.label, hex(bytes));
        }
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes)?;
        stdin.flush()
    }

    fn receive(&mut self, wait: Duration) -> io::Result<Vec<u8>> {
        let buffer = match self.data.recv_timeout(wait) {
            Ok(buffer) => buffer,
            Err(RecvTimeoutError::Timeout) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("the headphones closed the {} channel", self.label),
                ));
            }
        };
        if self.verbose {
            eprintln!("{} < {}", self.label, hex(&buffer));
        }
        Ok(buffer)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // Closing stdin makes the helper close the channel and exit.
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}
