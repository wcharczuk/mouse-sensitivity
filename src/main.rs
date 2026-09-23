//! mouse-sensitivity — set mouse sensitivity on macOS using HID APIs.
//!
//! The CLI talks to a background daemon over a Unix socket. The daemon keeps
//! an IOHIDEventSystemClient alive on a CFRunLoop so HIDPointerResolution
//! changes persist (the same mechanism LinearMouse uses).

mod calibrate;
mod config;
mod daemon;
mod hid;
mod ipc;
mod known;
mod vendor;

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

        /// Tracking speed, 0.0–40.0. In linear mode this is the cursor speed; with
        /// the curve on it's the steepness. (LinearMouse: "Tracking speed".
        /// --acceleration still works as an alias.)
        #[arg(short = 't', short_alias = 'a', long, alias = "acceleration", value_name = "0.0-40.0")]
        tracking_speed: Option<f64>,

        /// Pointer resolution scale, 0.0–1.0. Only effective when the curve is
        /// enabled. (LinearMouse: "Pointer speed")
        #[arg(short, long, value_name = "0.0-1.0")]
        speed: Option<f64>,

        /// Sensor resolution in counts per inch, if you know it. `calibrate`
        /// measures this for you; `match` uses it.
        #[arg(long, value_name = "DPI")]
        dpi: Option<f64>,
    },

    /// Measure a mouse's effective DPI by moving it a known distance
    Calibrate {
        /// Device index (default: the only connected mouse)
        #[arg(short, long)]
        device: Option<usize>,

        /// Physical distance you will move the mouse on each pass, in centimetres
        #[arg(long, default_value_t = 10.0, value_name = "CM")]
        distance_cm: f64,

        /// Number of passes to average
        #[arg(long, default_value_t = 2)]
        passes: u32,

        /// No ruler? Compare against another connected mouse instead: move both
        /// across the same span (a mousepad edge to edge, say) and the length
        /// cancels out. Index or name substring of a connected mouse.
        #[arg(long, value_name = "MOUSE")]
        against: Option<String>,

        /// With --against: apply the matching tracking speed right away
        #[arg(long, requires = "against")]
        apply: bool,
    },

    /// Get a mouse's sensor DPI without measuring: ask the mouse itself (Razer
    /// and Logitech protocols), or fall back to its factory default
    Dpi {
        /// Device index (default: the only connected mouse)
        #[arg(short, long)]
        device: Option<usize>,

        /// Record the value for `match`, as `calibrate` would
        #[arg(long)]
        save: bool,

        /// Don't ask the mouse; use the factory-default DPI from the built-in
        /// table (needs no Input Monitoring permission)
        #[arg(long)]
        known: bool,

        /// A mouse from the config by name substring instead of a connected
        /// one; it need not be plugged in. Uses the built-in table (implies --known).
        #[arg(long, value_name = "NAME", conflicts_with = "device")]
        saved: Option<String>,

        /// Print the built-in table of factory DPI values and exit
        #[arg(long, exclusive = true)]
        list_known: bool,
    },

    /// Set a mouse's tracking speed so the cursor travels the same distance per
    /// centimetre as on another (calibrated) mouse
    Match {
        /// Target device index (default: the only connected mouse)
        #[arg(short, long)]
        device: Option<usize>,

        /// Reference mouse: case-insensitive substring of a name in the config,
        /// e.g. "razer". It does not need to be connected.
        #[arg(long, value_name = "NAME")]
        from: String,

        /// Print the computed tracking speed without applying it
        #[arg(long)]
        dry_run: bool,
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
            tracking_speed,
            speed,
            dpi,
        } => {
            let toggle = if disable_acceleration {
                Some(true)
            } else if enable_acceleration {
                Some(false)
            } else {
                None
            };
            cmd_set(device, toggle, tracking_speed, speed, dpi)
        }
        Commands::Calibrate { device, distance_cm, passes, against, apply } => match against {
            Some(reference) => cmd_calibrate_against(device, &reference, distance_cm, passes, apply),
            None => cmd_calibrate(device, distance_cm, passes),
        },
        Commands::Dpi { device, save, known, saved, list_known } => {
            if list_known {
                cmd_dpi_list_known()
            } else if let Some(name) = saved {
                cmd_dpi_saved(&name, save)
            } else {
                cmd_dpi(device, save, known)
            }
        }
        Commands::Match { device, from, dry_run } => cmd_match(device, &from, dry_run),
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
    tracking_speed: Option<f64>,
    speed: Option<f64>,
    dpi: Option<f64>,
) -> i32 {
    if disable_acceleration.is_none() && tracking_speed.is_none() && speed.is_none() && dpi.is_none() {
        eprintln!("error: nothing to set; pass --disable-acceleration/--enable-acceleration, --tracking-speed, --speed, or --dpi");
        return 1;
    }
    if let Some(v) = dpi {
        if config::sanitize_dpi(v).is_none() {
            eprintln!("error: --dpi must be a positive number");
            return 1;
        }
    }
    if ipc::connect().is_some() {
        return send_set(Request::Set { device, disable_acceleration, tracking_speed, speed, dpi });
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
            if let Some(v) = dpi {
                match persist_dpi_without_daemon(d, v) {
                    Ok(()) => println!("[{i}] {} — dpi={v:.0} (saved to config)", d.name),
                    Err(e) => eprintln!("[{i}] {} — failed to save dpi: {e}", d.name),
                }
                if disable_acceleration.is_none() && tracking_speed.is_none() && speed.is_none() {
                    continue;
                }
            }
            let da = disable_acceleration.unwrap_or_else(|| d.get_linear_scaling() == Some(1));
            let ac = tracking_speed.unwrap_or_else(|| d.get_acceleration().unwrap_or(0.6875));
            let sp = speed.unwrap_or_else(|| d.get_speed().unwrap_or(0.5));
            match d.apply_settings(da, ac, sp) {
                Ok(()) => println!(
                    "[{i}] {} — linear={} tracking-speed={:.4} speed={:.4}",
                    d.name, da, ac, sp
                ),
                Err(e) => eprintln!("[{i}] {} — error: {e}", d.name),
            }
        }
        0
    })
}

/// Send a `set` request to the running daemon and report the outcome.
fn send_set(req: Request) -> i32 {
    let Some(mut s) = ipc::connect() else {
        eprintln!("error: daemon not running");
        return 1;
    };
    match ipc::roundtrip(&mut s, &req) {
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
    }
}

/// With no daemon there is nothing to race against, so write config.json
/// directly. Seeds a new entry from the device's live values so the daemon,
/// once it starts, enforces what the user already has.
fn persist_dpi_without_daemon(d: &PointerDevice, dpi: f64) -> std::io::Result<()> {
    let mut cfg = config::load()?;
    cfg.upsert(d.vendor_id, d.product_id, &d.name, || config::live_settings(d), |s| {
        s.dpi = config::sanitize_dpi(dpi);
    });
    config::save(&cfg)
}

/// The daemon and the CLI enumerate devices separately, so resolve the
/// daemon's index for a device by identity rather than trusting ours.
fn resolve_daemon_index(d: &PointerDevice) -> Result<usize, String> {
    let mut s = ipc::connect().ok_or_else(|| "daemon not running".to_string())?;
    match ipc::roundtrip(&mut s, &Request::List) {
        Ok(Response::Devices { devices }) => devices
            .iter()
            .find(|x| x.vendor_id == d.vendor_id && x.product_id == d.product_id && x.name == d.name)
            .map(|x| x.index)
            .ok_or_else(|| format!("the daemon does not see {}", d.name)),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// Pick the device to operate on: the given index, or the only mouse present.
fn pick_device(devices: &[PointerDevice], device: Option<usize>) -> Option<(usize, &PointerDevice)> {
    match device {
        Some(idx) if idx < devices.len() => Some((idx, &devices[idx])),
        Some(idx) => {
            eprintln!("error: device index {idx} out of range ({} connected)", devices.len());
            None
        }
        None if devices.len() == 1 => Some((0, &devices[0])),
        None => {
            eprintln!("error: {} mice connected; pick one with -d <index>:", devices.len());
            for (i, d) in devices.iter().enumerate() {
                eprintln!("  [{i}] {}", d.name);
            }
            None
        }
    }
}

fn cmd_calibrate(device: Option<usize>, distance_cm: f64, passes: u32) -> i32 {
    if !(distance_cm.is_finite() && distance_cm > 0.0) {
        eprintln!("error: --distance-cm must be a positive number");
        return 1;
    }
    if passes == 0 {
        eprintln!("error: --passes must be at least 1");
        return 1;
    }
    direct(|devices| {
        let Some((idx, dev)) = pick_device(devices, device) else {
            return 1;
        };
        let Some(accel) = linear_tracking_speed(idx, dev) else {
            return 1;
        };

        println!("Calibrating [{idx}] {} (linear mode, tracking speed {accel:.4})", dev.name);
        println!();
        println!("Set up a {distance_cm} cm span on the desk. No ruler? A credit card's long edge is");
        println!("8.56 cm (pass --distance-cm 8.56), or compare two mice with --against instead.");
        println!("Keep the cursor away from the screen edges, and press Enter on the keyboard");
        println!("rather than a mouse button.");
        println!();

        let m = match calibrate::measure(&dev.name, distance_cm, passes) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        let dpi = calibrate::effective_dpi(m.points_per_cm, accel);
        println!();
        println!(
            "Cursor travel: {:.1} pt/cm at tracking speed {accel:.4} (average of {} pass{})",
            m.points_per_cm,
            m.passes.len(),
            if m.passes.len() == 1 { "" } else { "es" }
        );
        println!("Effective DPI: {dpi:.0}");
        if let Some(spread) = m.spread() {
            if spread > 0.10 {
                eprintln!(
                    "warning: passes differed by {:.0}%; re-run with more care if you want a tighter number",
                    spread * 100.0
                );
            }
        }

        if let Err(e) = save_dpi(dev, dpi) {
            eprintln!("error: failed to save DPI for {}: {e}", dev.name);
            return 1;
        }
        println!("Saved DPI {dpi:.0} for {}.", dev.name);
        println!();
        print_match_hint(dev);
        0
    })
}

/// Ruler-free calibration: move the reference mouse and the new mouse across
/// the same span; the span's length cancels out of the comparison.
fn cmd_calibrate_against(
    device: Option<usize>,
    reference: &str,
    nominal_span_cm: f64,
    passes: u32,
    apply: bool,
) -> i32 {
    if !(nominal_span_cm.is_finite() && nominal_span_cm > 0.0) {
        eprintln!("error: --distance-cm must be a positive number");
        return 1;
    }
    if passes == 0 {
        eprintln!("error: --passes must be at least 1");
        return 1;
    }
    direct(|devices| {
        if devices.len() < 2 {
            eprintln!(
                "error: --against needs both mice connected at once; only {} is connected.\n       \
                 Plug the other one in, or calibrate each mouse on its own with a known distance.",
                devices.first().map(|d| d.name.as_str()).unwrap_or("nothing")
            );
            return 1;
        }
        let (ref_idx, ref_dev) = match resolve_connected(devices, reference) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        let (idx, dev) = match device {
            Some(i) if i < devices.len() => (i, &devices[i]),
            Some(i) => {
                eprintln!("error: device index {i} out of range ({} connected)", devices.len());
                return 1;
            }
            None => {
                let others: Vec<usize> = (0..devices.len()).filter(|&i| i != ref_idx).collect();
                match others.as_slice() {
                    [only] => (*only, &devices[*only]),
                    _ => {
                        eprintln!("error: several mice besides the reference are connected; pick one with -d <index>:");
                        for i in others {
                            eprintln!("  [{i}] {}", devices[i].name);
                        }
                        return 1;
                    }
                }
            }
        };
        if idx == ref_idx {
            eprintln!("error: {} is the reference mouse itself", dev.name);
            return 1;
        }
        let Some(ref_accel) = linear_tracking_speed(ref_idx, ref_dev) else {
            return 1;
        };
        let Some(accel) = linear_tracking_speed(idx, dev) else {
            return 1;
        };

        println!("Calibrating [{idx}] {} against [{ref_idx}] {}", dev.name, ref_dev.name);
        println!();
        println!("Pick a span you can repeat exactly with both mice: a mousepad edge to edge, the");
        println!("width of your keyboard, two marks on the desk. Its length doesn't matter. Keep the");
        println!("cursor away from the screen edges and press Enter on the keyboard, not a button.");
        println!();

        let ref_travel = match calibrate::measure_travel(&ref_dev.name, "across the span", passes) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        println!();
        let new_travel = match calibrate::measure_travel(&dev.name, "across the same span", passes) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        let ref_mean = calibrate::mean(&ref_travel).unwrap_or_default();
        let new_mean = calibrate::mean(&new_travel).unwrap_or_default();
        let cmp = calibrate::compare(ref_mean, ref_accel, new_mean, accel);

        println!();
        println!("{}: {ref_mean:.0} pt across the span at tracking speed {ref_accel:.4}", ref_dev.name);
        println!("{}: {new_mean:.0} pt across the span at tracking speed {accel:.4}", dev.name);
        println!(
            "{} needs tracking speed {:.4} to match ({:.2}× the sensor resolution of {}).",
            dev.name, cmp.matched_tracking_speed, cmp.ratio, ref_dev.name
        );

        // Anchor the DPI numbers on the reference's recorded DPI when there is
        // one; otherwise assume the span was `nominal_span_cm` long. Either way
        // the two values share a scale, which is all `match` needs.
        let recorded_ref_dpi = config::load()
            .ok()
            .and_then(|cfg| {
                cfg.devices
                    .iter()
                    .find(|d| d.matcher.matches(ref_dev.vendor_id, ref_dev.product_id))
                    .and_then(|d| d.settings.dpi)
            });
        let (ref_dpi, new_dpi) = match recorded_ref_dpi {
            Some(d) => {
                let span_cm = ref_mean * 2.54 / (d * ref_accel);
                println!(
                    "Using {}'s recorded {d:.0} DPI as the anchor; your span works out to about {span_cm:.1} cm.",
                    ref_dev.name
                );
                (d, d * cmp.ratio)
            }
            None => {
                println!(
                    "The span wasn't measured, so both DPI values assume it was {nominal_span_cm} cm (pass"
                );
                println!(
                    "--distance-cm if you know better). They're relative, but consistent with each other."
                );
                (
                    calibrate::effective_dpi(ref_mean / nominal_span_cm, ref_accel),
                    calibrate::effective_dpi(new_mean / nominal_span_cm, accel),
                )
            }
        };
        if recorded_ref_dpi.is_none() {
            if let Err(e) = save_dpi(ref_dev, ref_dpi) {
                eprintln!("error: failed to save DPI for {}: {e}", ref_dev.name);
                return 1;
            }
            println!("Saved DPI {ref_dpi:.0} for {}.", ref_dev.name);
        }
        if let Err(e) = save_dpi(dev, new_dpi) {
            eprintln!("error: failed to save DPI for {}: {e}", dev.name);
            return 1;
        }
        println!("Saved DPI {new_dpi:.0} for {}.", dev.name);
        println!();

        if apply {
            return apply_linear_tracking_speed(dev, cmp.matched_tracking_speed);
        }
        println!(
            "Apply it with `mouse-sensitivity match --from \"{}\" -d {idx}`, or re-run with --apply.",
            ref_dev.name
        );
        0
    })
}

/// Live tracking speed of a device that is in linear mode, or `None` after
/// printing why calibration can't proceed.
fn linear_tracking_speed(idx: usize, dev: &PointerDevice) -> Option<f64> {
    if dev.get_linear_scaling() != Some(1) {
        eprintln!(
            "error: {} is using the acceleration curve; calibration needs linear mode.\n       \
             Run `mouse-sensitivity set -d {idx} --disable-acceleration` first.",
            dev.name
        );
        return None;
    }
    let Some(accel) = dev.get_acceleration() else {
        eprintln!("error: could not read the tracking speed from {}", dev.name);
        return None;
    };
    if accel <= 0.0 {
        eprintln!(
            "error: {} has tracking speed 0, so its cursor cannot move; set a nonzero --tracking-speed first",
            dev.name
        );
        return None;
    }
    Some(accel)
}

/// Find a connected device by index or by case-insensitive name substring.
fn resolve_connected<'a>(devices: &'a [PointerDevice], key: &str) -> Result<(usize, &'a PointerDevice), String> {
    if let Ok(i) = key.parse::<usize>() {
        return devices
            .get(i)
            .map(|d| (i, d))
            .ok_or_else(|| format!("device index {i} out of range ({} connected)", devices.len()));
    }
    let needle = key.to_lowercase();
    let hits: Vec<(usize, &PointerDevice)> = devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.name.to_lowercase().contains(&needle))
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => {
            let names: Vec<String> = devices.iter().enumerate().map(|(i, d)| format!("[{i}] {}", d.name)).collect();
            Err(format!("no connected mouse matches \"{key}\"; connected: {}", names.join(", ")))
        }
        many => {
            let names: Vec<String> = many.iter().map(|(i, d)| format!("[{i}] {}", d.name)).collect();
            Err(format!("\"{key}\" is ambiguous; matches {}", names.join(", ")))
        }
    }
}

/// Record a device's DPI through the daemon, or straight into config.json
/// when no daemon is running.
fn save_dpi(dev: &PointerDevice, dpi: f64) -> Result<(), String> {
    if ipc::connect().is_none() {
        eprintln!("warning: daemon not running; writing config.json directly");
        return persist_dpi_without_daemon(dev, dpi).map_err(|e| e.to_string());
    }
    let didx = resolve_daemon_index(dev)?;
    let req = Request::Set {
        device: Some(didx),
        disable_acceleration: None,
        tracking_speed: None,
        speed: None,
        dpi: Some(dpi),
    };
    let mut s = ipc::connect().ok_or_else(|| "daemon went away".to_string())?;
    match ipc::roundtrip(&mut s, &req) {
        Ok(Response::Ok) => Ok(()),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(e) => Err(e.to_string()),
    }
}

/// Put a device in linear mode at `accel`, via the daemon when it's running.
fn apply_linear_tracking_speed(dev: &PointerDevice, accel: f64) -> i32 {
    if ipc::connect().is_some() {
        let didx = match resolve_daemon_index(dev) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        return send_set(Request::Set {
            device: Some(didx),
            disable_acceleration: Some(true),
            tracking_speed: Some(accel),
            speed: None,
            dpi: None,
        });
    }
    eprintln!("warning: daemon not running; applying directly. Changes will revert on reconnect/sleep.");
    match dev.apply_settings(true, accel, dev.get_speed().unwrap_or(0.5)) {
        Ok(()) => {
            println!("applied");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// Wording shared by everything that hands out a factory default.
const FACTORY_CAVEAT: &str = "A factory default is right only if the DPI stage was never changed (with a DPI\n\
button or the vendor's software). If it was, ask the mouse with `dpi` (Razer and\n\
Logitech), measure with `calibrate`, or type the real value in with `set --dpi`.";

fn not_in_table(name: &str) -> String {
    format!(
        "{name} is not in the built-in table of factory DPI values (`dpi --list-known`); \
         measure it with `calibrate` or record it by hand with `set --dpi <value>`"
    )
}

/// Ask the mouse for its DPI over its vendor protocol. `Err` carries the
/// reason so the caller can print it as a note before falling back.
fn query_sensor_dpi(dev: &PointerDevice) -> Result<vendor::SensorDpi, String> {
    let (Some(vid), Some(pid)) = (dev.vendor_id, dev.product_id) else {
        return Err(format!("{} reports no vendor/product id", dev.name));
    };
    vendor::read_sensor_dpi(vid, pid).map_err(|e| e.to_string())
}

fn cmd_dpi(device: Option<usize>, save: bool, use_table: bool) -> i32 {
    direct(|devices| {
        let Some((idx, dev)) = pick_device(devices, device) else {
            return 1;
        };
        let table = known::lookup(dev.vendor_id, dev.product_id, &dev.name);
        let (dpi, factory): (f64, Option<known::KnownDpi>) = if use_table {
            match table {
                Some(k) => (k.dpi as f64, Some(k)),
                None => {
                    eprintln!("error: {}", not_in_table(&dev.name));
                    return 1;
                }
            }
        } else {
            println!("Asking [{idx}] {} for its sensor DPI...", dev.name);
            match query_sensor_dpi(dev) {
                Ok(reading) => {
                    if reading.dpi_x == reading.dpi_y {
                        println!("Sensor DPI: {}", reading.dpi_x);
                    } else {
                        println!("Sensor DPI: {} × {} (x × y)", reading.dpi_x, reading.dpi_y);
                        eprintln!("warning: x and y resolutions differ; using the x value");
                    }
                    if let Some(d) = reading.default_dpi {
                        println!("Factory default: {d}");
                    }
                    println!("Source: {}", reading.source);
                    (reading.dpi_x as f64, None)
                }
                Err(e) => match table {
                    Some(k) => {
                        eprintln!("note: {e}");
                        println!("Falling back to the built-in table of factory DPI values.");
                        (k.dpi as f64, Some(k))
                    }
                    None => {
                        eprintln!("error: {e}");
                        eprintln!("       {}", not_in_table(&dev.name));
                        return 1;
                    }
                },
            }
        };
        if let Some(k) = factory {
            println!("[{idx}] {}: {} DPI, {}.", dev.name, k.dpi, k.describe());
            println!("{FACTORY_CAVEAT}");
        }
        if !save {
            println!("Re-run with --save to record it for `match`.");
            return 0;
        }
        if let Err(e) = save_dpi(dev, dpi) {
            eprintln!("error: failed to save DPI for {}: {e}", dev.name);
            return 1;
        }
        println!("Saved DPI {dpi:.0} for {}.", dev.name);
        println!();
        print_match_hint(dev);
        0
    })
}

/// `dpi --saved NAME`: the factory default for a configured mouse that need
/// not be connected, optionally written into its config entry.
fn cmd_dpi_saved(name: &str, save: bool) -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: cannot read config: {e}");
            return 1;
        }
    };
    let entry = match find_saved(&cfg, name) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let Some(k) = known::lookup(entry.matcher.vendor_id, entry.matcher.product_id, &entry.name) else {
        eprintln!("error: {}", not_in_table(&entry.name));
        return 1;
    };
    println!("{}: {} DPI, {}.", entry.name, k.dpi, k.describe());
    if let Some(recorded) = entry.settings.dpi {
        println!("Currently on record: {recorded:.0} DPI.");
    }
    println!("{FACTORY_CAVEAT}");
    if !save {
        println!("Re-run with --save to record it for `match`.");
        return 0;
    }
    if let Err(e) = persist_dpi_for_entry(&entry.matcher, k.dpi as f64) {
        eprintln!("error: failed to save DPI for {}: {e}", entry.name);
        return 1;
    }
    println!("Saved DPI {} for {}.", k.dpi, entry.name);
    0
}

/// Record a DPI for a config entry whose mouse may not be connected. The
/// daemon's `set` needs a live device, so write config.json directly and then
/// have the daemon reload, or its in-memory copy would clobber the change on
/// its next save.
fn persist_dpi_for_entry(matcher: &config::DeviceMatch, dpi: f64) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    let entry = cfg
        .devices
        .iter_mut()
        .find(|d| d.matcher == *matcher)
        .ok_or_else(|| "the config entry disappeared".to_string())?;
    entry.settings.dpi = config::sanitize_dpi(dpi);
    config::save(&cfg).map_err(|e| e.to_string())?;
    let Some(mut s) = ipc::connect() else {
        return Ok(());
    };
    match ipc::roundtrip(&mut s, &Request::Reload) {
        Ok(Response::Ok) => Ok(()),
        Ok(Response::Error { message }) => Err(format!("saved, but the daemon failed to reload it: {message}")),
        Ok(other) => Err(format!("saved, but the daemon answered unexpectedly: {other:?}")),
        Err(e) => Err(format!("saved, but the daemon failed to reload it: {e}")),
    }
}

fn cmd_dpi_list_known() -> i32 {
    println!("Factory DPI values built into this tool. `match` uses them for any mouse without a");
    println!("recorded DPI; `dpi --known --save` (or `dpi --saved <name> --save`) records one.");
    println!();
    println!("Models (vendor:product):");
    for m in known::models() {
        println!("  {:04X}:{:04X}  {:<44} {:>5}", m.vendor_id, m.product_id, m.name, m.default_dpi);
    }
    println!();
    println!("Family rules, for models not listed:");
    for f in known::families() {
        println!("  {:<55} {:>5}", f.description, f.default_dpi);
    }
    println!();
    println!("{FACTORY_CAVEAT}");
    0
}

/// Find a config entry by case-insensitive name substring; the mouse need not
/// be connected.
fn find_saved<'a>(cfg: &'a config::Config, needle: &str) -> Result<&'a config::DeviceConfig, String> {
    let candidates = cfg.find_by_name(needle);
    match candidates.as_slice() {
        [one] => Ok(*one),
        [] => {
            let names: Vec<&str> = cfg.devices.iter().map(|d| d.name.as_str()).collect();
            Err(format!(
                "no configured mouse matches \"{needle}\"; known: {}",
                if names.is_empty() { "(none)".to_string() } else { names.join(", ") }
            ))
        }
        many => {
            let names: Vec<&str> = many.iter().map(|d| d.name.as_str()).collect();
            Err(format!("\"{needle}\" is ambiguous; matches {}", names.join(", ")))
        }
    }
}

/// A DPI figure and where it came from: recorded in the config (measured, read
/// from the mouse, or typed in) or a factory default from the built-in table.
struct DpiFigure {
    dpi: f64,
    factory: Option<known::KnownDpi>,
}

impl DpiFigure {
    fn recorded(dpi: f64) -> Self {
        Self { dpi, factory: None }
    }

    fn factory(k: known::KnownDpi) -> Self {
        Self { dpi: k.dpi as f64, factory: Some(k) }
    }

    fn is_factory(&self) -> bool {
        self.factory.is_some()
    }

    fn provenance(&self) -> &'static str {
        if self.is_factory() {
            "factory default"
        } else {
            "recorded"
        }
    }
}

/// The DPI to use for a config entry: what's recorded, else the factory default.
fn dpi_for_entry(entry: &config::DeviceConfig) -> Option<DpiFigure> {
    entry.settings.dpi.map(DpiFigure::recorded).or_else(|| {
        known::lookup(entry.matcher.vendor_id, entry.matcher.product_id, &entry.name).map(DpiFigure::factory)
    })
}

/// The DPI to use for a connected device: what's recorded for it, else the
/// factory default.
fn dpi_for_device(cfg: &config::Config, dev: &PointerDevice) -> Option<DpiFigure> {
    cfg.devices
        .iter()
        .find(|d| d.matcher.matches(dev.vendor_id, dev.product_id))
        .and_then(|d| d.settings.dpi)
        .map(DpiFigure::recorded)
        .or_else(|| known::lookup(dev.vendor_id, dev.product_id, &dev.name).map(DpiFigure::factory))
}

/// After a calibration, tell the user what they can match against.
fn print_match_hint(dev: &PointerDevice) {
    let Ok(cfg) = config::load() else {
        return;
    };
    let others: Vec<(&config::DeviceConfig, DpiFigure)> = cfg
        .devices
        .iter()
        .filter(|d| !d.matcher.matches(dev.vendor_id, dev.product_id))
        .filter(|d| d.settings.disable_acceleration)
        .filter_map(|d| dpi_for_entry(d).map(|f| (d, f)))
        .collect();
    if others.is_empty() {
        println!("No other mouse has a DPI on record or in the built-in table yet. Connect one and");
        println!("run `calibrate` or `dpi --save`, or set it by hand with `set -d <index> --dpi <value>`;");
        println!("then run `match --from <name>`.");
        return;
    }
    println!("Mice you can match against:");
    for (d, f) in others {
        println!(
            "  {} — {:.0} DPI ({}) × tracking speed {:.4} = {:.1} pt/cm",
            d.name,
            f.dpi,
            f.provenance(),
            d.settings.tracking_speed,
            calibrate::points_per_cm(f.dpi, d.settings.tracking_speed)
        );
    }
    println!("Run `mouse-sensitivity match --from <name>` to apply one to {}.", dev.name);
}

fn cmd_match(device: Option<usize>, from: &str, dry_run: bool) -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: cannot read config: {e}");
            return 1;
        }
    };
    let src = match find_saved(&cfg, from) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let Some(src_figure) = dpi_for_entry(src) else {
        eprintln!(
            "error: {} has no DPI on record and isn't in the built-in table (`dpi --list-known`).\n       \
             Connect it and run `mouse-sensitivity calibrate` or `mouse-sensitivity dpi --save`,\n       \
             or set it by hand with `mouse-sensitivity set -d <index> --dpi <value>`.",
            src.name
        );
        return 1;
    };
    if !src.settings.disable_acceleration {
        eprintln!(
            "error: {} uses the acceleration curve; match only works between mice in linear mode",
            src.name
        );
        return 1;
    }

    direct(|devices| {
        let Some((idx, dev)) = pick_device(devices, device) else {
            return 1;
        };
        if src.matcher.matches(dev.vendor_id, dev.product_id) {
            eprintln!("error: {} is the reference mouse itself", dev.name);
            return 1;
        }
        let Some(dst_figure) = dpi_for_device(&cfg, dev) else {
            eprintln!(
                "error: {} has no DPI on record and isn't in the built-in table; run\n       \
                 `mouse-sensitivity calibrate -d {idx}` or `mouse-sensitivity dpi -d {idx} --save` first",
                dev.name
            );
            return 1;
        };

        let accel = config::match_tracking_speed(src.settings.tracking_speed, src_figure.dpi, dst_figure.dpi);
        println!(
            "Reference: {} — {:.0} DPI ({}) × tracking speed {:.4} = {:.1} pt/cm",
            src.name,
            src_figure.dpi,
            src_figure.provenance(),
            src.settings.tracking_speed,
            calibrate::points_per_cm(src_figure.dpi, src.settings.tracking_speed)
        );
        println!(
            "Target:    {} — {:.0} DPI ({}) → tracking speed {:.4} (linear mode)",
            dev.name,
            dst_figure.dpi,
            dst_figure.provenance(),
            accel
        );
        for f in [&src_figure, &dst_figure] {
            if let Some(k) = f.factory {
                println!("           {:.0} DPI is {}, not a measurement.", f.dpi, k.describe());
            }
        }
        if src_figure.is_factory() || dst_figure.is_factory() {
            println!("{FACTORY_CAVEAT}");
        }
        if dry_run {
            println!("(dry run; nothing applied)");
            return 0;
        }
        apply_linear_tracking_speed(dev, accel)
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
                    println!("    Tracking speed: {:.4} (0.0-40.0)", d.tracking_speed);
                    println!(
                        "    Speed: {:.4} (0.0-1.0)  [resolution ≈ {:.0}]",
                        d.speed,
                        PointerDevice::speed_to_resolution(d.speed)
                    );
                    match d.dpi {
                        Some(dpi) => println!(
                            "    DPI: {dpi:.0} (effective)  [{:.1} pt/cm at this tracking speed]",
                            calibrate::points_per_cm(dpi, d.tracking_speed)
                        ),
                        None => {
                            if let Some(k) = known::lookup(d.vendor_id, d.product_id, &d.name) {
                                println!(
                                    "    DPI: {} (factory default, not recorded)  [{:.1} pt/cm at this tracking speed]",
                                    k.dpi,
                                    calibrate::points_per_cm(k.dpi as f64, d.tracking_speed)
                                );
                            }
                        }
                    }
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
        println!("    Tracking speed: {:.4} (0.0-40.0)", a);
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
