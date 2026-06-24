//! mouse-sensitivity — set mouse sensitivity on macOS using HID APIs.
//!
//! The CLI talks to a background daemon over a Unix socket. The daemon keeps
//! an IOHIDEventSystemClient alive on a CFRunLoop so HIDPointerResolution
//! changes persist (the same mechanism LinearMouse uses).

mod config;
mod daemon;
mod hid;
mod ipc;

use clap::{Parser, Subcommand};
use hid::{PointerDevice, PointerDeviceManager};
use ipc::{Request, Response};

#[derive(Parser)]
#[command(name = "mouse-sensitivity")]
#[command(version, about = "Set mouse sensitivity on macOS using HID APIs", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the persistent daemon (normally launched via launchd)
    Daemon,

    /// Show whether the daemon is running
    Status,

    /// Ask the running daemon to exit
    Stop,

    /// Write a LaunchAgent plist and load it so the daemon starts at login
    Install,

    /// Remove the LaunchAgent
    Uninstall,

    /// List connected mouse devices
    List,

    /// Show current settings
    Get {
        #[arg(short, long)]
        device: Option<usize>,

        /// Read live HID values from the device instead of the daemon's config
        #[arg(long)]
        live: bool,
    },

    /// Change settings (persisted by the daemon)
    Set {
        #[arg(short, long)]
        device: Option<usize>,

        /// Disable the macOS acceleration curve (linear 1:1 movement)
        #[arg(long)]
        disable_acceleration: bool,

        /// Enable the macOS acceleration curve
        #[arg(long, conflicts_with = "disable_acceleration")]
        enable_acceleration: bool,

        /// Tracking speed, 0.0–40.0. In linear mode this is the cursor speed; in
        /// accelerated mode it's the curve steepness. (LinearMouse: "Tracking speed")
        #[arg(short, long, value_name = "0.0-40.0")]
        acceleration: Option<f64>,

        /// Pointer resolution scale, 0.0–1.0. Only effective when acceleration
        /// is enabled. (LinearMouse: "Pointer speed")
        #[arg(short, long, value_name = "0.0-1.0")]
        speed: Option<f64>,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Daemon => match daemon::run() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("daemon: {e}");
                1
            }
        },
        Commands::Status => cmd_status(),
        Commands::Stop => cmd_stop(),
        Commands::Install => cmd_install(),
        Commands::Uninstall => cmd_uninstall(),
        Commands::List => cmd_list(),
        Commands::Get { device, live } => cmd_get(device, live),
        Commands::Set {
            device,
            disable_acceleration,
            enable_acceleration,
            acceleration,
            speed,
        } => {
            let toggle = if disable_acceleration {
                Some(true)
            } else if enable_acceleration {
                Some(false)
            } else {
                None
            };
            cmd_set(device, toggle, acceleration, speed)
        }
    };
    std::process::exit(code);
}

fn cmd_status() -> i32 {
    match ipc::connect() {
        Some(mut s) => match ipc::roundtrip(&mut s, &Request::Ping) {
            Ok(Response::Pong { version }) => {
                println!("daemon running (v{version})");
                println!("socket: {}", config::socket_path().display());
                println!("config: {}", config::config_path().display());
                0
            }
            Ok(other) => {
                eprintln!("unexpected response: {other:?}");
                1
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        None => {
            println!("daemon not running");
            1
        }
    }
}

fn cmd_stop() -> i32 {
    match ipc::connect() {
        Some(mut s) => match ipc::roundtrip(&mut s, &Request::Shutdown) {
            Ok(_) => {
                println!("daemon stopping");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        None => {
            eprintln!("daemon not running");
            1
        }
    }
}

fn cmd_list() -> i32 {
    if let Some(mut s) = ipc::connect() {
        return print_devices_response(ipc::roundtrip(&mut s, &Request::List), false);
    }
    direct(|devices| {
        println!("Found {} pointer device(s):\n", devices.len());
        for (i, d) in devices.iter().enumerate() {
            println!("[{i}] {}", d.name);
            if let (Some(vid), Some(pid)) = (d.vendor_id, d.product_id) {
                println!("    Vendor ID: 0x{:04X}  Product ID: 0x{:04X}", vid, pid);
            }
        }
        0
    })
}

fn cmd_get(device: Option<usize>, live: bool) -> i32 {
    if !live {
        if let Some(mut s) = ipc::connect() {
            return print_devices_response(ipc::roundtrip(&mut s, &Request::Get { device }), true);
        }
        eprintln!("warning: daemon not running; reading live HID state (values may revert)");
    }
    direct(|devices| {
        for (i, d) in devices.iter().enumerate() {
            if let Some(idx) = device {
                if idx != i {
                    continue;
                }
            }
            print_device_live(i, d);
        }
        0
    })
}

fn cmd_set(
    device: Option<usize>,
    disable_acceleration: Option<bool>,
    acceleration: Option<f64>,
    speed: Option<f64>,
) -> i32 {
    if disable_acceleration.is_none() && acceleration.is_none() && speed.is_none() {
        eprintln!("error: nothing to set; pass --disable-acceleration/--enable-acceleration, --acceleration, or --speed");
        return 1;
    }
    if let Some(mut s) = ipc::connect() {
        let req = Request::Set { device, disable_acceleration, acceleration, speed };
        return match ipc::roundtrip(&mut s, &req) {
            Ok(Response::Ok) => {
                println!("ok");
                0
            }
            Ok(Response::Error { message }) => {
                eprintln!("error: {message}");
                1
            }
            Ok(other) => {
                eprintln!("unexpected response: {other:?}");
                1
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        };
    }

    eprintln!(
        "warning: daemon not running; applying directly. Changes will revert on reconnect/sleep.\n         Run `mouse-sensitivity install` or `mouse-sensitivity daemon &` for persistence."
    );
    direct(|devices| {
        let targets: Vec<(usize, &PointerDevice)> = match device {
            Some(idx) if idx < devices.len() => vec![(idx, &devices[idx])],
            Some(idx) => {
                eprintln!("error: device index {idx} out of range");
                return 1;
            }
            None => devices.iter().enumerate().collect(),
        };
        for (i, d) in targets {
            let da = disable_acceleration.unwrap_or_else(|| d.get_linear_scaling() == Some(1));
            let ac = acceleration.unwrap_or_else(|| d.get_acceleration().unwrap_or(0.6875));
            let sp = speed.unwrap_or_else(|| d.get_speed().unwrap_or(0.5));
            match d.apply_settings(da, ac, sp) {
                Ok(()) => println!(
                    "[{i}] {} — accel-disabled={} acceleration={:.4} speed={:.4}",
                    d.name, da, ac, sp
                ),
                Err(e) => eprintln!("[{i}] {} — error: {e}", d.name),
            }
        }
        0
    })
}

fn print_devices_response(resp: std::io::Result<Response>, detailed: bool) -> i32 {
    match resp {
        Ok(Response::Devices { devices }) => {
            if !detailed {
                println!("Found {} pointer device(s):\n", devices.len());
            }
            for d in devices {
                println!("[{}] {}", d.index, d.name);
                if let (Some(vid), Some(pid)) = (d.vendor_id, d.product_id) {
                    println!("    Vendor ID: 0x{:04X}  Product ID: 0x{:04X}", vid, pid);
                }
                if detailed {
                    println!(
                        "    Mode: {}",
                        if d.disable_acceleration { "Linear (no acceleration)" } else { "Accelerated" }
                    );
                    println!("    Acceleration: {:.4} (0.0-40.0)", d.acceleration);
                    println!(
                        "    Speed: {:.4} (0.0-1.0)  [resolution ≈ {:.0}]",
                        d.speed,
                        PointerDevice::speed_to_resolution(d.speed)
                    );
                }
                println!();
            }
            0
        }
        Ok(Response::Error { message }) => {
            eprintln!("error: {message}");
            1
        }
        Ok(other) => {
            eprintln!("unexpected response: {other:?}");
            1
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn print_device_live(i: usize, d: &PointerDevice) {
    println!("[{i}] {}", d.name);
    let linear = d.get_linear_scaling() == Some(1);
    println!("    Mode: {}", if linear { "Linear (no acceleration)" } else { "Accelerated" });
    if let Some(a) = d.get_acceleration() {
        println!("    Acceleration: {:.4} (0.0-40.0)", a);
    }
    match d.get_resolution() {
        Some(r) => println!(
            "    Resolution: {:.2}  [speed ≈ {:.4}]",
            r,
            PointerDevice::resolution_to_speed(r)
        ),
        None => println!("    Resolution: (not reported)"),
    }
    println!();
}

fn direct<F: FnOnce(&[PointerDevice]) -> i32>(f: F) -> i32 {
    let manager = match PointerDeviceManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let devices = match manager.get_devices() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    f(&devices)
}

const LAUNCH_AGENT_LABEL: &str = "com.wcharczuk.mouse-sensitivity";

fn launch_agent_path() -> std::path::PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

fn cmd_install() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot resolve current executable: {e}");
            return 1;
        }
    };
    let log = config::support_dir().join("daemon.log");
    if let Err(e) = std::fs::create_dir_all(config::support_dir()) {
        eprintln!("error: {e}");
        return 1;
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        exe = exe.display(),
        log = log.display(),
    );
    let path = launch_agent_path();
    if let Err(e) = std::fs::create_dir_all(path.parent().unwrap()) {
        eprintln!("error: {e}");
        return 1;
    }
    if let Err(e) = std::fs::write(&path, plist) {
        eprintln!("error: failed to write {}: {e}", path.display());
        return 1;
    }
    println!("wrote {}", path.display());
    let status = std::process::Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&path)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("launch agent loaded");
            0
        }
        Ok(s) => {
            eprintln!("launchctl exited with {s}");
            1
        }
        Err(e) => {
            eprintln!("error: failed to run launchctl: {e}");
            1
        }
    }
}

fn cmd_uninstall() -> i32 {
    let path = launch_agent_path();
    let _ = std::process::Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(&path)
        .status();
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("removed {}", path.display());
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("not installed");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
