//! D700 HID-interface sniffer. The Asparion D700 exposes a vendor **HID**
//! interface (USB interface 0) alongside its class-compliant USB-MIDI interface
//! — the HID side is the "native"/flexible channel the Configurator uses.
//!
//! This reads the device->host HID **input reports** (controls / status) via
//! `hidapi`, co-opening the device alongside the Configurator. It does NOT show
//! the host->device output/feature reports (LED colour, config) the Configurator
//! SENDS — capturing those needs a true USB bus sniff (Windows + USBPcap, or a
//! hardware analyzer). Throwaway diagnostic; not part of S21_HiJack.

use hidapi::HidApi;
use std::time::{Duration, Instant};

const VID: u16 = 0x04D8; // Microchip (Asparion's MCU vendor)
const PID: u16 = 0xE44E; // D 700

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);

    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("hidapi init failed: {e}");
            return;
        }
    };

    println!("HID interfaces matching {VID:04X}:{PID:04X}:");
    let mut any = false;
    for d in api.device_list() {
        if d.vendor_id() == VID && d.product_id() == PID {
            any = true;
            println!(
                "  iface {:>2}  usage_page={:#06x} usage={:#06x}  product={:?}  path={:?}",
                d.interface_number(),
                d.usage_page(),
                d.usage(),
                d.product_string().unwrap_or(""),
                d.path()
            );
        }
    }
    if !any {
        println!("  (none — is the D700 connected?)");
        return;
    }

    let dev = match api.open(VID, PID) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "\nopen failed: {e}\n(if 'exclusive'/'privilege', the Configurator holds the HID \
                 interface — quit it and retry, or this confirms it owns the vendor channel)"
            );
            return;
        }
    };
    let _ = dev.set_blocking_mode(false);

    println!("\n--- move faders / knobs / buttons for {secs}s (HID input reports) ---");
    println!("len  bytes");
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 64];
    let mut count = 0usize;
    while Instant::now() < deadline {
        match dev.read_timeout(&mut buf, 100) {
            Ok(0) => {}
            Ok(n) => {
                count += 1;
                let hex: Vec<String> = buf[..n].iter().map(|b| format!("{b:02X}")).collect();
                println!("[{n:2}] {}", hex.join(" "));
            }
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }
    }
    println!("--- done: {count} input reports ---");
    if count == 0 {
        println!("No HID input reports. Likely the controls report over the USB-MIDI");
        println!("interface (not HID), and HID carries only host->device LED/config —");
        println!("which a bus sniff (USBPcap/analyzer) would be needed to see.");
    }
}
