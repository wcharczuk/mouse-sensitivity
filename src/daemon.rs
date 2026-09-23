//! Persistent daemon: keeps an IOHIDEventSystemClient alive on a CFRunLoop so
//! HIDPointerResolution writes actually stick, re-applies settings on device
//! hotplug and on a 1s safety tick, and serves a Unix-socket control API.

use std::ffi::c_void;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use core_foundation::runloop::kCFRunLoopDefaultMode;
use core_foundation_sys::runloop::CFRunLoopRunInMode;

use crate::config::{self, Config};
use crate::hid::{IOHIDServiceClientRef, PointerDevice, PointerDeviceManager};
use crate::ipc::{DeviceInfo, Request, Response};

struct Shared {
    config: Mutex<Config>,
    dirty: AtomicBool,
    shutdown: AtomicBool,
}

pub fn run() -> std::io::Result<()> {
    let dir = config::support_dir();
    fs::create_dir_all(&dir)?;
    // ~/Library/Application Support is already 0700 on macOS, but lock our
    // subdir too so the bind→chmod window on the socket is unreachable.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    let shared = Arc::new(Shared {
        config: Mutex::new(config::load()?),
        dirty: AtomicBool::new(true),
        shutdown: AtomicBool::new(false),
    });

    let listener = bind_socket()?;
    let ipc_shared = Arc::clone(&shared);
    thread::Builder::new()
        .name("ipc".into())
        .spawn(move || serve_ipc(listener, ipc_shared))?;

    run_hid_loop(shared);
    let _ = fs::remove_file(config::socket_path());
    Ok(())
}

fn bind_socket() -> std::io::Result<UnixListener> {
    let path = config::socket_path();
    if path.exists() {
        // Refuse to start a second daemon.
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("daemon already running at {}", path.display()),
            ));
        }
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// HID thread: owns the long-lived event-system client. The client being
/// scheduled on a run loop is what makes resolution changes persist (matching
/// LinearMouse). Device-matching callbacks mark state dirty; the loop wakes at
/// least once per second to re-apply, which also covers WindowServer resets.
fn run_hid_loop(shared: Arc<Shared>) {
    let manager = match PointerDeviceManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("daemon: failed to create HID client: {e}");
            return;
        }
    };

    extern "C" fn on_device_match(target: *mut c_void, _refcon: *mut c_void, _svc: IOHIDServiceClientRef) {
        // Safe: target is &Shared, kept alive for the duration of the run loop.
        let shared = unsafe { &*(target as *const Shared) };
        shared.dirty.store(true, Ordering::Release);
    }
    manager.schedule_on_current_runloop(on_device_match, Arc::as_ptr(&shared) as *mut c_void);

    let mut tick: u32 = 0;
    loop {
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0);
        }

        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }

        tick = tick.wrapping_add(1);
        let force = tick.is_multiple_of(3); // periodic re-assert against System Settings
        if force || shared.dirty.swap(false, Ordering::AcqRel) {
            let cfg = shared.config.lock().unwrap().clone();
            apply_config(&manager, &cfg);
        }
    }
}

fn apply_config(manager: &PointerDeviceManager, cfg: &Config) {
    let devices = match manager.get_devices() {
        Ok(d) => d,
        Err(_) => return,
    };
    for dev in &devices {
        if let Some(s) = cfg.settings_for(dev.vendor_id, dev.product_id) {
            if let Err(e) = dev.apply_settings(s.disable_acceleration, s.tracking_speed, s.speed) {
                eprintln!("daemon: failed to apply settings to {}: {e}", dev.name);
            }
        }
    }
}

fn serve_ipc(listener: UnixListener, shared: Arc<Shared>) {
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        let client_shared = Arc::clone(&shared);
        thread::spawn(move || handle_client(stream, client_shared));
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
    }
}

fn handle_client(stream: UnixStream, shared: Arc<Shared>) {
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle_request(req, &shared),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
    }
}

fn handle_request(req: Request, shared: &Arc<Shared>) -> Response {
    match req {
        Request::Ping => Response::Pong { version: env!("CARGO_PKG_VERSION").to_string() },
        Request::List => list_devices(shared, None),
        Request::Get { device } => list_devices(shared, device),
        Request::Reload => match config::load() {
            Ok(cfg) => {
                *shared.config.lock().unwrap() = cfg;
                shared.dirty.store(true, Ordering::Release);
                Response::Ok
            }
            Err(e) => Response::Error { message: e.to_string() },
        },
        Request::Shutdown => {
            shared.shutdown.store(true, Ordering::Release);
            Response::Ok
        }
        Request::Set { device, disable_acceleration, tracking_speed, speed, dpi } => {
            handle_set(shared, device, disable_acceleration, tracking_speed, speed, dpi)
        }
    }
}

fn list_devices(shared: &Arc<Shared>, only: Option<usize>) -> Response {
    let manager = match PointerDeviceManager::new() {
        Ok(m) => m,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    let devices = match manager.get_devices() {
        Ok(d) => d,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    let cfg = shared.config.lock().unwrap();
    let mut out = Vec::new();
    for (i, dev) in devices.iter().enumerate() {
        if let Some(idx) = only {
            if idx != i {
                continue;
            }
        }
        out.push(device_info(i, dev, &cfg));
    }
    Response::Devices { devices: out }
}

fn device_info(index: usize, dev: &PointerDevice, cfg: &Config) -> DeviceInfo {
    // Prefer configured values; fall back to live device readings so `get`
    // reflects what the daemon will enforce.
    let configured = cfg.settings_for(dev.vendor_id, dev.product_id);
    DeviceInfo {
        index,
        name: dev.name.clone(),
        vendor_id: dev.vendor_id,
        product_id: dev.product_id,
        disable_acceleration: configured
            .map(|s| s.disable_acceleration)
            .unwrap_or_else(|| dev.get_linear_scaling() == Some(1)),
        tracking_speed: configured
            .map(|s| s.tracking_speed)
            .or_else(|| dev.get_acceleration())
            .unwrap_or(0.6875),
        speed: configured
            .map(|s| s.speed)
            .or_else(|| dev.get_speed())
            .unwrap_or(0.5),
        dpi: configured.and_then(|s| s.dpi),
    }
}

fn handle_set(
    shared: &Arc<Shared>,
    device: Option<usize>,
    disable_acceleration: Option<bool>,
    tracking_speed: Option<f64>,
    speed: Option<f64>,
    dpi: Option<f64>,
) -> Response {
    let dpi = match dpi {
        Some(v) => match config::sanitize_dpi(v) {
            Some(d) => Some(d),
            None => {
                return Response::Error { message: format!("invalid dpi {v}; must be a positive number") }
            }
        },
        None => None,
    };
    let manager = match PointerDeviceManager::new() {
        Ok(m) => m,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    let devices = match manager.get_devices() {
        Ok(d) => d,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    if let Some(idx) = device {
        if idx >= devices.len() {
            return Response::Error { message: format!("device index {idx} out of range") };
        }
    }

    let mut cfg = shared.config.lock().unwrap();
    let targets: Vec<&PointerDevice> = match device {
        Some(idx) => vec![&devices[idx]],
        None => devices.iter().collect(),
    };
    for dev in targets {
        cfg.upsert(
            dev.vendor_id,
            dev.product_id,
            &dev.name,
            || config::live_settings(dev),
            |s| {
                if let Some(v) = disable_acceleration {
                    s.disable_acceleration = v;
                }
                if let Some(v) = tracking_speed {
                    s.tracking_speed = v.clamp(0.0, 40.0);
                }
                if let Some(v) = speed {
                    s.speed = v.clamp(0.0, 1.0);
                }
                if let Some(v) = dpi {
                    s.dpi = Some(v);
                }
            },
        );
    }
    if let Err(e) = config::save(&cfg) {
        return Response::Error { message: format!("failed to save config: {e}") };
    }
    let snapshot = cfg.clone();
    drop(cfg);

    shared.dirty.store(true, Ordering::Release);
    // Apply immediately on this ephemeral client too so the CLI feels instant;
    // the long-lived run-loop client will re-assert within a second.
    apply_config(&manager, &snapshot);
    Response::Ok
}
