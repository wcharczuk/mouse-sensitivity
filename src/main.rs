//! mouse-sensitivity - A CLI utility to set mouse sensitivity on macOS
//!
//! This tool uses private IOKit HID APIs to control mouse sensitivity and acceleration
//! at the driver level, similar to LinearMouse.

mod hid;

use clap::{Parser, Subcommand};
use hid::{PointerDevice, PointerDeviceManager};

#[derive(Parser)]
#[command(name = "mouse-sensitivity")]
#[command(author = "Will Charczuk")]
#[command(version = "0.1.0")]
#[command(about = "Set mouse sensitivity on macOS using HID APIs", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all connected mouse/pointer devices
    List,

    /// Get current settings for all devices
    Get {
        /// Device index (from list command). If not specified, shows all devices.
        #[arg(short, long)]
        device: Option<usize>,
    },

    /// Set mouse settings
    Set {
        /// Device index (from list command). If not specified, applies to all devices.
        #[arg(short, long)]
        device: Option<usize>,

        /// Enable acceleration curve
        #[arg(long, group = "accel_toggle")]
        acceleration: bool,

        /// Disable acceleration curve (use linear mode)
        #[arg(long, group = "accel_toggle")]
        no_acceleration: bool,

        /// Tracking speed (1.0-20.0, used with --acceleration)
        #[arg(short, long)]
        tracking_speed: Option<f64>,

        /// Pointer speed (0.0-1.0, used with --no-acceleration)
        #[arg(short, long)]
        speed: Option<f64>,
    },
}

fn main() {
    let cli = Cli::parse();

    let manager = match PointerDeviceManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: Failed to initialize HID manager: {}", e);
            std::process::exit(1);
        }
    };

    let devices = match manager.get_devices() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    match cli.command {
        Commands::List => {
            list_devices(&devices);
        }
        Commands::Get { device } => {
            get_settings(&devices, device);
        }
        Commands::Set {
            device,
            acceleration,
            no_acceleration,
            tracking_speed,
            speed,
        } => {
            set_settings(&devices, device, acceleration, no_acceleration, tracking_speed, speed);
        }
    }
}

fn list_devices(devices: &[PointerDevice]) {
    println!("Found {} pointer device(s):\n", devices.len());

    for (i, device) in devices.iter().enumerate() {
        println!("[{}] {}", i, device.name);
        if let Some(vid) = device.vendor_id {
            print!("    Vendor ID: 0x{:04X}", vid);
        }
        if let Some(pid) = device.product_id {
            print!("  Product ID: 0x{:04X}", pid);
        }
        println!();
    }
}

fn get_settings(devices: &[PointerDevice], device_index: Option<usize>) {
    let target_devices: Vec<(usize, &PointerDevice)> = match device_index {
        Some(idx) => {
            if idx >= devices.len() {
                eprintln!("Error: Device index {} out of range (0-{})", idx, devices.len() - 1);
                std::process::exit(1);
            }
            vec![(idx, &devices[idx])]
        }
        None => devices.iter().enumerate().collect(),
    };

    for (i, device) in target_devices {
        println!("[{}] {}", i, device.name);

        let linear_on = device.get_linear_scaling() == Some(1);

        if linear_on {
            // Linear mode: tracking speed controls pointer speed
            println!("    Mode: Linear (no acceleration)");
            if let Some(tracking) = device.get_tracking_speed() {
                // Convert tracking speed back to 0-1 scale for display
                let speed = ((tracking - 0.2) / 2.8).clamp(0.0, 1.0);
                println!("    Speed: {:.2} (0.0-1.0 scale)", speed);
                println!("    Raw tracking speed: {:.2}", tracking);
            }
        } else {
            // Normal mode: resolution controls base speed, acceleration adds curve
            println!("    Mode: Accelerated");
            if let Some(tracking) = device.get_tracking_speed() {
                println!("    Tracking speed: {:.2} (1.0-20.0 scale)", tracking);
            }
            if let Some(resolution) = device.get_resolution() {
                println!("    Resolution: {:.2}", resolution);
            }
        }

        println!();
    }
}

fn set_settings(
    devices: &[PointerDevice],
    device_index: Option<usize>,
    acceleration: bool,
    no_acceleration: bool,
    tracking_speed: Option<f64>,
    speed: Option<f64>,
) {
    // Validate flag combinations
    if !acceleration && !no_acceleration {
        eprintln!("Error: Must specify either --acceleration or --no-acceleration");
        std::process::exit(1);
    }

    if acceleration && speed.is_some() {
        eprintln!("Error: --speed can only be used with --no-acceleration");
        std::process::exit(1);
    }

    if no_acceleration && tracking_speed.is_some() {
        eprintln!("Error: --tracking-speed can only be used with --acceleration");
        std::process::exit(1);
    }

    // Validate ranges
    if let Some(ts) = tracking_speed {
        if ts < 1.0 || ts > 20.0 {
            eprintln!("Warning: Tracking speed {} out of range, will be clamped to 1.0-20.0", ts);
        }
    }

    if let Some(s) = speed {
        if s < 0.0 || s > 1.0 {
            eprintln!("Warning: Speed {} out of range, will be clamped to 0.0-1.0", s);
        }
    }

    let target_devices = get_target_devices(devices, device_index);

    for (i, device) in target_devices {
        if acceleration {
            // Enable acceleration mode
            let _ = device.set_linear_scaling(false);

            let ts = tracking_speed.unwrap_or(1.0);
            match device.set_tracking_speed(ts) {
                Ok(()) => {
                    println!("[{}] {} - Acceleration enabled, tracking speed: {:.2}", i, device.name, ts);
                }
                Err(e) => {
                    eprintln!("[{}] {} - Error: {}", i, device.name, e);
                }
            }
        } else {
            // Disable acceleration (linear mode)
            match device.set_linear_scaling(true) {
                Ok(()) => {
                    let s = speed.unwrap_or(0.5);
                    match device.set_speed(s) {
                        Ok(()) => {
                            println!("[{}] {} - Acceleration disabled, speed: {:.2}", i, device.name, s);
                        }
                        Err(e) => {
                            eprintln!("[{}] {} - Acceleration disabled, but failed to set speed: {}", i, device.name, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[{}] {} - Error: {}", i, device.name, e);
                }
            }
        }
    }
}

fn get_target_devices(devices: &[PointerDevice], device_index: Option<usize>) -> Vec<(usize, &PointerDevice)> {
    match device_index {
        Some(idx) => {
            if idx >= devices.len() {
                eprintln!("Error: Device index {} out of range (0-{})", idx, devices.len() - 1);
                std::process::exit(1);
            }
            vec![(idx, &devices[idx])]
        }
        None => devices.iter().enumerate().collect(),
    }
}
