// SPDX-License-Identifier: AGPL-3.0-or-later
//! USB transport for LMC boards, using the pure-Rust `nusb` crate.
//!
//! The board enumerates as 9588:9899 and talks over interface 0 with bulk
//! OUT endpoint 0x02 and bulk IN endpoint 0x88, per MeerK40t's
//! `balormk/usb_connection.py` (Copyright (c) 2021 meerk40t, MIT License).
//! Linux needs the udev rule in `packaging/udev/`.

use std::time::Duration;

use libreburn_core::{Error, ErrorCode, MachineProfile, Result};
use nusb::transfer::{Buffer, Bulk, In, Out};
use nusb::{Endpoint, MaybeFuture};

use crate::Transport;

/// Default USB vendor ID of LMC boards.
pub const VENDOR_ID: u16 = 0x9588;
/// Default USB product ID of LMC boards.
pub const PRODUCT_ID: u16 = 0x9899;
/// Bulk OUT endpoint.
pub const WRITE_ENDPOINT: u8 = 0x02;
/// Bulk IN endpoint.
pub const READ_ENDPOINT: u8 = 0x88;
/// Per-transfer timeout. MeerK40t uses 100 ms and retries.
pub const TIMEOUT: Duration = Duration::from_millis(100);
/// Tries per transfer before giving up.
pub const TRIES: usize = 3;

fn unreachable(why: impl Into<String>) -> Error {
    Error::new(
        ErrorCode::DeviceUnreachable,
        "The LMC board did not answer",
        why,
        "check the USB cable and power; on Linux install packaging/udev/70-libreburn.rules; \
         on Windows the board needs the WinUSB driver (Zadig)",
    )
}

/// An open LMC board.
pub struct UsbTransport {
    out: Endpoint<Bulk, Out>,
    inp: Endpoint<Bulk, In>,
    // Keep the interface claimed for as long as the transport lives.
    _interface: nusb::Interface,
}

impl std::fmt::Debug for UsbTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbTransport").finish_non_exhaustive()
    }
}

impl UsbTransport {
    /// Open the first board with these IDs. `index` picks among several.
    pub fn open(vendor_id: u16, product_id: u16, index: usize) -> Result<Self> {
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| unreachable(format!("cannot list USB devices: {e}")))?
            .filter(|d| d.vendor_id() == vendor_id && d.product_id() == product_id)
            .nth(index)
            .ok_or_else(|| {
                unreachable(format!(
                    "no USB device {vendor_id:04x}:{product_id:04x} (number {index}) is connected"
                ))
            })?;
        let device = info
            .open()
            .wait()
            .map_err(|e| unreachable(format!("cannot open the board: {e}")))?;
        let interface = device
            .detach_and_claim_interface(0)
            .wait()
            .map_err(|e| unreachable(format!("cannot claim the board's interface: {e}")))?;
        let out = interface
            .endpoint::<Bulk, Out>(WRITE_ENDPOINT)
            .map_err(|e| unreachable(format!("no OUT endpoint {WRITE_ENDPOINT:#04x}: {e}")))?;
        let inp = interface
            .endpoint::<Bulk, In>(READ_ENDPOINT)
            .map_err(|e| unreachable(format!("no IN endpoint {READ_ENDPOINT:#04x}: {e}")))?;
        Ok(Self {
            out,
            inp,
            _interface: interface,
        })
    }

    /// Open the board named by the profile's `usb_vendor_id`,
    /// `usb_product_id` and optional `usb_index`.
    pub fn from_profile(profile: &MachineProfile) -> Result<Self> {
        let c = &profile.connection;
        let get = |k: &str, d: i64| c.get(k).and_then(|v| v.as_integer()).unwrap_or(d);
        let id = |k: &str, d: u16| {
            u16::try_from(get(k, i64::from(d))).map_err(|_| {
                Error::new(
                    ErrorCode::ProfileInvalid,
                    "Invalid LMC connection settings",
                    format!("connection.{k} is not a 16-bit USB ID"),
                    "use the IDs from lsusb, such as 0x9588 and 0x9899",
                )
            })
        };
        let index = usize::try_from(get("usb_index", 0)).unwrap_or(0);
        Self::open(
            id("usb_vendor_id", VENDOR_ID)?,
            id("usb_product_id", PRODUCT_ID)?,
            index,
        )
    }
}

impl Transport for UsbTransport {
    fn write(&mut self, packet: &[u8]) -> Result<()> {
        let mut last = String::new();
        for _ in 0..TRIES {
            let c = self
                .out
                .transfer_blocking(Buffer::from(packet.to_vec()), TIMEOUT);
            match c.status {
                Ok(()) if c.actual_len == packet.len() => return Ok(()),
                Ok(()) => last = format!("short write: {} of {} bytes", c.actual_len, packet.len()),
                Err(e) => last = e.to_string(),
            }
        }
        Err(unreachable(format!("USB write failed: {last}")))
    }

    fn read(&mut self) -> Result<[u8; 8]> {
        let mut last = String::new();
        for _ in 0..TRIES {
            let len = self.inp.max_packet_size().max(8);
            let c = self.inp.transfer_blocking(Buffer::new(len), TIMEOUT);
            match c.status {
                Ok(()) if c.actual_len >= 8 => {
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&c.buffer[..8]);
                    return Ok(out);
                }
                Ok(()) => last = format!("short reply: {} bytes", c.actual_len),
                Err(e) => last = e.to_string(),
            }
        }
        Err(unreachable(format!("USB read failed: {last}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_board_is_reported_as_unreachable() {
        // No LMC board is attached in CI; the error must be the documented one.
        // An ID nothing uses keeps this true on a developer machine too.
        match UsbTransport::open(0xFFFF, 0xFFFE, 0) {
            Err(e) => assert_eq!(e.code, ErrorCode::DeviceUnreachable),
            Ok(_) => panic!("found a USB device with IDs ffff:fffe"),
        }
    }
}
