//! Read a mouse's sensor DPI over its vendor protocol.
//!
//! Neither macOS nor the HID standard exposes sensor resolution, but the two
//! vendors covered here answer a proprietary query:
//!
//! * Razer: a 90-byte feature report on the mouse's HID interface, command
//!   class 0x04 / id 0x05 ("get DPI XY"), the same packet openrazer sends.
//! * Logitech: HID++ 2.0 feature 0x2201 "Adjustable DPI", function 2
//!   `getSensorDpi`, over long (0x11) reports. Direct Bluetooth or USB
//!   connections only; Unifying/Bolt receivers address devices differently
//!   and aren't handled.
//!
//! Both need the device opened through IOHIDDevice, which macOS gates behind
//! the Input Monitoring permission for pointing devices: the first run prompts
//! for the terminal application.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::kCFRunLoopDefaultMode;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFAllocatorRef, CFIndex, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::runloop::{CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRunInMode};
use core_foundation_sys::set::{CFSetGetCount, CFSetGetValues, CFSetRef};
use core_foundation_sys::string::CFStringRef;
use thiserror::Error;

pub const VENDOR_RAZER: i64 = 0x1532;
pub const VENDOR_LOGITECH: i64 = 0x046D;

type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOReturn = i32;

const IO_RETURN_SUCCESS: IOReturn = 0;
const IO_RETURN_NOT_PERMITTED: IOReturn = 0xE00002E2u32 as i32;
const IO_RETURN_EXCLUSIVE_ACCESS: IOReturn = 0xE00002C5u32 as i32;
const IOHID_OPTIONS_NONE: u32 = 0;
const IOHID_REPORT_TYPE_OUTPUT: u32 = 1;
const IOHID_REPORT_TYPE_FEATURE: u32 = 2;

type IOHIDReportCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    report_type: u32,
    report_id: u32,
    report: *mut u8,
    report_length: CFIndex,
);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: u32) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerCopyDevices(manager: IOHIDManagerRef) -> CFSetRef;
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
    fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceClose(device: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceSetReport(
        device: IOHIDDeviceRef,
        report_type: u32,
        report_id: CFIndex,
        report: *const u8,
        report_length: CFIndex,
    ) -> IOReturn;
    fn IOHIDDeviceGetReport(
        device: IOHIDDeviceRef,
        report_type: u32,
        report_id: CFIndex,
        report: *mut u8,
        report_length: *mut CFIndex,
    ) -> IOReturn;
    fn IOHIDDeviceRegisterInputReportCallback(
        device: IOHIDDeviceRef,
        report: *mut u8,
        report_length: CFIndex,
        callback: Option<IOHIDReportCallback>,
        context: *mut c_void,
    );
    fn IOHIDDeviceScheduleWithRunLoop(device: IOHIDDeviceRef, run_loop: CFRunLoopRef, mode: CFStringRef);
    fn IOHIDDeviceUnscheduleFromRunLoop(device: IOHIDDeviceRef, run_loop: CFRunLoopRef, mode: CFStringRef);
}

#[derive(Error, Debug)]
pub enum VendorError {
    #[error("no vendor protocol known for vendor 0x{0:04X}; supported: Razer (0x1532) and Logitech (0x046D)")]
    Unsupported(i64),
    #[error("no HID interface found for vendor 0x{0:04X} product 0x{1:04X}")]
    NotFound(i64, i64),
    #[error(
        "macOS refused to open the mouse: your terminal needs the Input Monitoring permission.\n       \
         Allow it under System Settings → Privacy & Security → Input Monitoring, then re-run."
    )]
    NotPermitted,
    #[error("{0}")]
    Protocol(String),
}

/// What the mouse reported about its sensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorDpi {
    pub dpi_x: u32,
    pub dpi_y: u32,
    /// Factory default, when the protocol reports one (Logitech does).
    pub default_dpi: Option<u32>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vendor {
    Razer,
    Logitech,
}

/// Ask the mouse identified by `vendor_id`/`product_id` for its sensor DPI.
pub fn read_sensor_dpi(vendor_id: i64, product_id: i64) -> Result<SensorDpi, VendorError> {
    let vendor = match vendor_id {
        VENDOR_RAZER => Vendor::Razer,
        VENDOR_LOGITECH => Vendor::Logitech,
        other => return Err(VendorError::Unsupported(other)),
    };
    let candidates = Candidates::find(vendor_id, product_id)?;
    if candidates.devices.is_empty() {
        return Err(VendorError::NotFound(vendor_id, product_id));
    }

    let mut last_error = String::from("no HID interface answered the DPI query");
    for &dev in &candidates.devices {
        let opened = unsafe { IOHIDDeviceOpen(dev, IOHID_OPTIONS_NONE) };
        match opened {
            IO_RETURN_SUCCESS => {}
            IO_RETURN_NOT_PERMITTED => return Err(VendorError::NotPermitted),
            IO_RETURN_EXCLUSIVE_ACCESS => {
                last_error = "another program holds the mouse open exclusively".to_string();
                continue;
            }
            other => {
                last_error = format!("IOHIDDeviceOpen failed (IOReturn 0x{:08X})", other as u32);
                continue;
            }
        }
        let result = match vendor {
            Vendor::Razer => razer_read(dev),
            Vendor::Logitech => logitech_read(dev),
        };
        unsafe {
            IOHIDDeviceClose(dev, IOHID_OPTIONS_NONE);
        }
        match result {
            Ok(dpi) => return Ok(dpi),
            Err(e) => last_error = e,
        }
    }
    Err(VendorError::Protocol(last_error))
}

/// HID interfaces matching a vendor/product pair, mouse interface first. Holds
/// the manager and the device set so the `IOHIDDeviceRef`s stay valid.
struct Candidates {
    manager: IOHIDManagerRef,
    set: CFSetRef,
    devices: Vec<IOHIDDeviceRef>,
}

impl Candidates {
    fn find(vendor_id: i64, product_id: i64) -> Result<Self, VendorError> {
        unsafe {
            let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOHID_OPTIONS_NONE);
            if manager.is_null() {
                return Err(VendorError::Protocol("IOHIDManagerCreate failed".into()));
            }
            let matching = CFDictionary::from_CFType_pairs(&[
                (CFString::new("VendorID").as_CFType(), CFNumber::from(vendor_id as i32).as_CFType()),
                (CFString::new("ProductID").as_CFType(), CFNumber::from(product_id as i32).as_CFType()),
            ]);
            IOHIDManagerSetDeviceMatching(manager, matching.as_concrete_TypeRef());
            let set = IOHIDManagerCopyDevices(manager);
            let mut devices = Vec::new();
            if !set.is_null() {
                let count = CFSetGetCount(set);
                let mut raw: Vec<*const c_void> = vec![std::ptr::null(); count as usize];
                CFSetGetValues(set, raw.as_mut_ptr());
                devices = raw.into_iter().map(|p| p as IOHIDDeviceRef).collect();
            }
            // Vendor commands go to the mouse interface; try it first.
            devices.sort_by_key(|&d| match device_int_property(d, "PrimaryUsage") {
                Some(2) => 0,
                Some(1) => 1,
                _ => 2,
            });
            Ok(Self { manager, set, devices })
        }
    }
}

impl Drop for Candidates {
    fn drop(&mut self) {
        unsafe {
            if !self.set.is_null() {
                CFRelease(self.set as CFTypeRef);
            }
            if !self.manager.is_null() {
                CFRelease(self.manager as CFTypeRef);
            }
        }
    }
}

fn device_int_property(dev: IOHIDDeviceRef, key: &str) -> Option<i64> {
    let key = CFString::new(key);
    unsafe {
        let value = IOHIDDeviceGetProperty(dev, key.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        let value: CFType = TCFType::wrap_under_get_rule(value);
        value.downcast::<CFNumber>().and_then(|n| n.to_i64())
    }
}

// ---------------------------------------------------------------------------
// Razer
// ---------------------------------------------------------------------------

const RAZER_REPORT_LEN: usize = 90;
const RAZER_STATUS_BUSY: u8 = 0x01;
const RAZER_STATUS_SUCCESS: u8 = 0x02;
const RAZER_STATUS_NOT_SUPPORTED: u8 = 0x05;
const RAZER_CLASS_MISC: u8 = 0x04;
const RAZER_CMD_GET_DPI_XY: u8 = 0x05;
const RAZER_VARSTORE: u8 = 0x01;

/// Build the "get DPI XY" request. Layout (openrazer `razer_report`): status,
/// transaction id, remaining packets (2), protocol type, data size, command
/// class, command id, 80 argument bytes, crc, reserved.
fn razer_dpi_request(transaction_id: u8) -> [u8; RAZER_REPORT_LEN] {
    let mut r = [0u8; RAZER_REPORT_LEN];
    r[1] = transaction_id;
    r[5] = 7; // data size
    r[6] = RAZER_CLASS_MISC;
    r[7] = RAZER_CMD_GET_DPI_XY;
    r[8] = RAZER_VARSTORE;
    r[88] = razer_crc(&r);
    r
}

/// XOR of bytes 2..88, as the firmware computes it.
fn razer_crc(report: &[u8; RAZER_REPORT_LEN]) -> u8 {
    report[2..88].iter().fold(0u8, |acc, b| acc ^ b)
}

/// Parse a response to `razer_dpi_request`; `None` if it isn't one.
fn razer_parse_dpi(resp: &[u8; RAZER_REPORT_LEN]) -> Option<(u32, u32)> {
    if resp[6] != RAZER_CLASS_MISC || resp[7] != RAZER_CMD_GET_DPI_XY {
        return None;
    }
    let dpi_x = u16::from_be_bytes([resp[9], resp[10]]) as u32;
    let dpi_y = u16::from_be_bytes([resp[11], resp[12]]) as u32;
    if dpi_x == 0 {
        return None;
    }
    Some((dpi_x, if dpi_y == 0 { dpi_x } else { dpi_y }))
}

fn razer_read(dev: IOHIDDeviceRef) -> Result<SensorDpi, String> {
    let mut last = String::from("the mouse did not answer the Razer DPI query");
    // Newer firmware wants 0x1f or 0x3f; older wants 0xff. Ask until one sticks.
    for transaction_id in [0xFFu8, 0x1F, 0x3F] {
        let request = razer_dpi_request(transaction_id);
        for attempt in 0..3 {
            let sent = unsafe {
                IOHIDDeviceSetReport(
                    dev,
                    IOHID_REPORT_TYPE_FEATURE,
                    0,
                    request.as_ptr(),
                    RAZER_REPORT_LEN as CFIndex,
                )
            };
            if sent != IO_RETURN_SUCCESS {
                last = format!("sending the Razer feature report failed (IOReturn 0x{:08X})", sent as u32);
                break;
            }
            std::thread::sleep(Duration::from_millis(30 + 30 * attempt));
            let mut resp = [0u8; RAZER_REPORT_LEN];
            let mut len = RAZER_REPORT_LEN as CFIndex;
            let got = unsafe {
                IOHIDDeviceGetReport(dev, IOHID_REPORT_TYPE_FEATURE, 0, resp.as_mut_ptr(), &mut len)
            };
            if got != IO_RETURN_SUCCESS {
                last = format!("reading the Razer feature report failed (IOReturn 0x{:08X})", got as u32);
                break;
            }
            match resp[0] {
                RAZER_STATUS_SUCCESS => match razer_parse_dpi(&resp) {
                    Some((x, y)) => {
                        return Ok(SensorDpi {
                            dpi_x: x,
                            dpi_y: y,
                            default_dpi: None,
                            source: "Razer protocol, get DPI XY (class 0x04, command 0x05)",
                        })
                    }
                    None => {
                        last = "the mouse answered, but not with a DPI report".to_string();
                        break;
                    }
                },
                RAZER_STATUS_BUSY => {
                    last = "the mouse reported busy".to_string();
                    continue;
                }
                RAZER_STATUS_NOT_SUPPORTED => {
                    last = format!("transaction id 0x{transaction_id:02X} not supported");
                    break;
                }
                other => {
                    last = format!("unexpected Razer status 0x{other:02X}");
                    break;
                }
            }
        }
    }
    Err(last)
}

// ---------------------------------------------------------------------------
// Logitech HID++ 2.0
// ---------------------------------------------------------------------------

const HIDPP_LONG_REPORT_ID: u8 = 0x11;
const HIDPP_SHORT_REPORT_ID: u8 = 0x10;
const HIDPP_LONG_LEN: usize = 20;
const HIDPP_DEVICE_INDEX_DIRECT: u8 = 0xFF;
const HIDPP_SOFTWARE_ID: u8 = 0x0A;
const HIDPP_ROOT_FEATURE_INDEX: u8 = 0x00;
const HIDPP_ERROR_FEATURE_INDEX: u8 = 0xFF;
const HIDPP1_ERROR_FEATURE_INDEX: u8 = 0x8F;
const HIDPP_FEATURE_ADJUSTABLE_DPI: u16 = 0x2201;
const HIDPP_TIMEOUT: Duration = Duration::from_secs(2);

/// Build a long HID++ report: id, device index, feature index, function and
/// software id nibbles, then up to 16 parameter bytes.
fn hidpp_request(feature_index: u8, function: u8, params: &[u8]) -> [u8; HIDPP_LONG_LEN] {
    let mut r = [0u8; HIDPP_LONG_LEN];
    r[0] = HIDPP_LONG_REPORT_ID;
    r[1] = HIDPP_DEVICE_INDEX_DIRECT;
    r[2] = feature_index;
    r[3] = (function << 4) | HIDPP_SOFTWARE_ID;
    let n = params.len().min(16);
    r[4..4 + n].copy_from_slice(&params[..n]);
    r
}

/// Outcome of matching one inbound report against a pending request.
#[derive(Debug, PartialEq, Eq)]
enum HidppReply {
    /// Response parameters (everything after the function/software byte).
    Params(Vec<u8>),
    /// HID++ error code for our request.
    Error(u8),
    /// Not for us (device traffic, another software id, etc.).
    Unrelated,
}

/// Interpret an inbound report. IOKit may or may not prepend the report id,
/// so accept both shapes.
fn hidpp_match(raw: &[u8], feature_index: u8, function_sw: u8) -> HidppReply {
    let payload: &[u8] = match raw.first() {
        Some(&HIDPP_LONG_REPORT_ID) | Some(&HIDPP_SHORT_REPORT_ID) => &raw[1..],
        _ => raw,
    };
    if payload.len() < 4 {
        return HidppReply::Unrelated;
    }
    let (feat, fs) = (payload[1], payload[2]);
    if (feat == HIDPP_ERROR_FEATURE_INDEX || feat == HIDPP1_ERROR_FEATURE_INDEX)
        && payload.len() >= 6
        && payload[3] == feature_index
        && payload[4] == function_sw
    {
        return HidppReply::Error(payload[5]);
    }
    if feat == feature_index && fs == function_sw {
        return HidppReply::Params(payload[3..].to_vec());
    }
    HidppReply::Unrelated
}

struct Inbox {
    reports: Vec<Vec<u8>>,
}

extern "C" fn on_input_report(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    _report_type: u32,
    _report_id: u32,
    report: *mut u8,
    report_length: CFIndex,
) {
    if context.is_null() || report.is_null() || report_length <= 0 {
        return;
    }
    // Safe: context is the Inbox owned by the HidppSession that registered us,
    // and it outlives the registration (see Drop).
    let inbox = unsafe { &mut *(context as *mut Inbox) };
    let bytes = unsafe { std::slice::from_raw_parts(report, report_length as usize) };
    inbox.reports.push(bytes.to_vec());
}

/// One opened Logitech device with an input-report callback scheduled on the
/// current run loop, so requests can wait for their answers.
struct HidppSession {
    dev: IOHIDDeviceRef,
    run_loop: CFRunLoopRef,
    buffer: Box<[u8; 64]>,
    inbox: Box<Inbox>,
}

impl HidppSession {
    fn new(dev: IOHIDDeviceRef) -> Self {
        let mut session = Self {
            dev,
            run_loop: unsafe { CFRunLoopGetCurrent() },
            buffer: Box::new([0u8; 64]),
            inbox: Box::new(Inbox { reports: Vec::new() }),
        };
        unsafe {
            IOHIDDeviceRegisterInputReportCallback(
                dev,
                session.buffer.as_mut_ptr(),
                session.buffer.len() as CFIndex,
                Some(on_input_report),
                &mut *session.inbox as *mut Inbox as *mut c_void,
            );
            IOHIDDeviceScheduleWithRunLoop(dev, session.run_loop, kCFRunLoopDefaultMode);
        }
        session
    }

    fn call(&mut self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, String> {
        let request = hidpp_request(feature_index, function, params);
        self.inbox.reports.clear();
        let sent = unsafe {
            IOHIDDeviceSetReport(
                self.dev,
                IOHID_REPORT_TYPE_OUTPUT,
                HIDPP_LONG_REPORT_ID as CFIndex,
                request.as_ptr(),
                HIDPP_LONG_LEN as CFIndex,
            )
        };
        if sent != IO_RETURN_SUCCESS {
            return Err(format!("sending the HID++ report failed (IOReturn 0x{:08X})", sent as u32));
        }
        let deadline = Instant::now() + HIDPP_TIMEOUT;
        while Instant::now() < deadline {
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1);
            }
            for raw in std::mem::take(&mut self.inbox.reports) {
                match hidpp_match(&raw, feature_index, request[3]) {
                    HidppReply::Params(p) => return Ok(p),
                    HidppReply::Error(code) => {
                        return Err(format!(
                            "HID++ error 0x{code:02X} for feature index {feature_index} function {function}"
                        ))
                    }
                    HidppReply::Unrelated => {}
                }
            }
        }
        Err("the mouse did not answer the HID++ request in time (if this keeps happening, \
             check that your terminal has the Input Monitoring permission)"
            .to_string())
    }
}

impl Drop for HidppSession {
    fn drop(&mut self) {
        unsafe {
            IOHIDDeviceUnscheduleFromRunLoop(self.dev, self.run_loop, kCFRunLoopDefaultMode);
            IOHIDDeviceRegisterInputReportCallback(
                self.dev,
                self.buffer.as_mut_ptr(),
                self.buffer.len() as CFIndex,
                None,
                std::ptr::null_mut(),
            );
        }
    }
}

fn logitech_read(dev: IOHIDDeviceRef) -> Result<SensorDpi, String> {
    let mut session = HidppSession::new(dev);
    // Root feature, function 0: getFeature(featureId) → [featureIndex, type, version]
    let id = HIDPP_FEATURE_ADJUSTABLE_DPI.to_be_bytes();
    let root = session.call(HIDPP_ROOT_FEATURE_INDEX, 0, &id)?;
    let feature_index = *root.first().ok_or("empty getFeature response")?;
    if feature_index == 0 {
        return Err("this Logitech mouse does not implement Adjustable DPI (HID++ feature 0x2201)".into());
    }
    // Adjustable DPI, function 2: getSensorDpi(sensorIdx) → [sensorIdx, dpi(2), defaultDpi(2)]
    let resp = session.call(feature_index, 2, &[0x00])?;
    if resp.len() < 5 {
        return Err(format!("short getSensorDpi response ({} bytes)", resp.len()));
    }
    let dpi = u16::from_be_bytes([resp[1], resp[2]]) as u32;
    let default_dpi = u16::from_be_bytes([resp[3], resp[4]]) as u32;
    if dpi == 0 {
        return Err("the mouse reported a DPI of 0".into());
    }
    Ok(SensorDpi {
        dpi_x: dpi,
        dpi_y: dpi,
        default_dpi: if default_dpi > 0 { Some(default_dpi) } else { None },
        source: "Logitech HID++ 2.0, Adjustable DPI (0x2201) getSensorDpi",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn razer_request_matches_openrazer_layout() {
        let r = razer_dpi_request(0xFF);
        assert_eq!(&r[..9], &[0x00, 0xFF, 0x00, 0x00, 0x00, 0x07, 0x04, 0x05, 0x01]);
        assert_eq!(r[88], razer_crc(&r));
        assert_eq!(r[89], 0);
        // CRC covers bytes 2..88 only, so the transaction id doesn't change it.
        assert_eq!(razer_dpi_request(0x1F)[88], r[88]);
    }

    #[test]
    fn razer_response_parsing() {
        let mut resp = razer_dpi_request(0xFF);
        resp[0] = RAZER_STATUS_SUCCESS;
        resp[9] = 0x07; // 1800 = 0x0708
        resp[10] = 0x08;
        resp[11] = 0x07;
        resp[12] = 0x08;
        assert_eq!(razer_parse_dpi(&resp), Some((1800, 1800)));
        resp[6] = 0x03; // wrong command class
        assert_eq!(razer_parse_dpi(&resp), None);
    }

    #[test]
    fn hidpp_request_layout() {
        let r = hidpp_request(0x00, 0, &[0x22, 0x01]);
        assert_eq!(&r[..6], &[0x11, 0xFF, 0x00, 0x0A, 0x22, 0x01]);
        let r = hidpp_request(0x09, 2, &[0x00]);
        assert_eq!(r[3], 0x2A);
    }

    #[test]
    fn hidpp_matching_accepts_both_report_shapes_and_errors() {
        // With the report id prepended, from feature 9 function 2 (0x2A).
        let with_id = [0x11, 0xFF, 0x09, 0x2A, 0x00, 0x03, 0xE8, 0x03, 0xE8];
        assert_eq!(
            hidpp_match(&with_id, 0x09, 0x2A),
            HidppReply::Params(vec![0x00, 0x03, 0xE8, 0x03, 0xE8])
        );
        // Without the report id.
        assert_eq!(hidpp_match(&with_id[1..], 0x09, 0x2A), HidppReply::Params(vec![0x00, 0x03, 0xE8, 0x03, 0xE8]));
        // Another software id: not ours.
        let other = [0x11, 0xFF, 0x09, 0x21, 0x00];
        assert_eq!(hidpp_match(&other, 0x09, 0x2A), HidppReply::Unrelated);
        // HID++ 2.0 error for our request.
        let err = [0x11, 0xFF, 0xFF, 0x09, 0x09, 0x2A, 0x06];
        assert_eq!(hidpp_match(&err, 0x09, 0x2A), HidppReply::Error(0x06));
        // Ordinary mouse traffic.
        assert_eq!(hidpp_match(&[0x02, 0x00, 0x05, 0x00], 0x09, 0x2A), HidppReply::Unrelated);
    }
}
