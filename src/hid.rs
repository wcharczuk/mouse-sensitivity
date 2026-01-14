//! HID (Human Interface Device) bindings for macOS
//!
//! This module provides Rust bindings to the private IOKit HID SPI (System Programming Interface)
//! used for controlling mouse sensitivity and acceleration at the driver level.

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::array::CFArrayGetValueAtIndex;
use thiserror::Error;

// Opaque types for HID service clients
#[repr(C)]
pub struct __IOHIDServiceClient {
    _private: [u8; 0],
}
pub type IOHIDServiceClientRef = *mut __IOHIDServiceClient;

#[repr(C)]
pub struct __IOHIDEventSystemClient {
    _private: [u8; 0],
}
pub type IOHIDEventSystemClientRef = *mut __IOHIDEventSystemClient;

// HID usage constants
pub const K_HID_PAGE_GENERIC_DESKTOP: i32 = 0x01;
pub const K_HID_USAGE_GD_MOUSE: i32 = 0x02;
pub const K_HID_USAGE_GD_POINTER: i32 = 0x01;

// Property key constants
pub const K_IOHID_POINTER_RESOLUTION_KEY: &str = "HIDPointerResolution";
pub const K_IOHID_POINTER_ACCELERATION_KEY: &str = "HIDPointerAcceleration";
pub const K_IOHID_MOUSE_ACCELERATION_TYPE_KEY: &str = "HIDMouseAcceleration";
pub const K_IOHID_POINTER_ACCELERATION_TYPE_KEY: &str = "HIDPointerAccelerationType";
pub const K_IOHID_USE_LINEAR_SCALING_KEY: &str = "HIDUseLinearScalingMouseAcceleration";
pub const K_IOHID_DEVICE_USAGE_PAGE_KEY: &str = "DeviceUsagePage";
pub const K_IOHID_DEVICE_USAGE_KEY: &str = "DeviceUsage";
pub const K_IOHID_PRODUCT_KEY: &str = "Product";
pub const K_IOHID_VENDOR_ID_KEY: &str = "VendorID";
pub const K_IOHID_PRODUCT_ID_KEY: &str = "ProductID";

// FFI declarations for private IOKit HID APIs
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDEventSystemClientCreate(
        allocator: core_foundation_sys::base::CFAllocatorRef,
    ) -> IOHIDEventSystemClientRef;

    fn IOHIDEventSystemClientSetMatchingMultiple(
        client: IOHIDEventSystemClientRef,
        matching: CFArrayRef,
    );

    fn IOHIDEventSystemClientCopyServices(
        client: IOHIDEventSystemClientRef,
    ) -> CFArrayRef;

    fn IOHIDServiceClientCopyProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
    ) -> core_foundation_sys::base::CFTypeRef;

    fn IOHIDServiceClientSetProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
        value: core_foundation_sys::base::CFTypeRef,
    ) -> bool;
}

#[derive(Error, Debug)]
pub enum HidError {
    #[error("Failed to create HID event system client")]
    ClientCreationFailed,
    #[error("No mouse devices found")]
    NoDevicesFound,
    #[error("Failed to set property: {0}")]
    PropertySetFailed(String),
}

/// Represents a connected HID pointer device (mouse or trackpad)
pub struct PointerDevice {
    service: IOHIDServiceClientRef,
    pub name: String,
    pub vendor_id: Option<i64>,
    pub product_id: Option<i64>,
}

impl PointerDevice {
    /// Get the acceleration type key for this device
    /// Different devices may use different property keys for acceleration
    fn get_acceleration_type_key(&self) -> String {
        // First, try to read the device's acceleration type property
        let type_key = CFString::new(K_IOHID_POINTER_ACCELERATION_TYPE_KEY);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, type_key.as_concrete_TypeRef());
            if !value.is_null() {
                let cf_type: CFType = TCFType::wrap_under_create_rule(value);
                if let Some(s) = cf_type.downcast::<CFString>() {
                    return s.to_string();
                }
            }
        }

        // Fallback: check which property exists on the device
        let pointer_key = CFString::new(K_IOHID_POINTER_ACCELERATION_KEY);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, pointer_key.as_concrete_TypeRef());
            if !value.is_null() {
                core_foundation_sys::base::CFRelease(value);
                return K_IOHID_POINTER_ACCELERATION_KEY.to_string();
            }
        }

        // Default to HIDMouseAcceleration
        K_IOHID_MOUSE_ACCELERATION_TYPE_KEY.to_string()
    }

    /// Get the current pointer resolution (IOFixed format: value * 65536)
    /// Lower values = faster pointer movement
    pub fn get_resolution(&self) -> Option<f64> {
        let key = CFString::new(K_IOHID_POINTER_RESOLUTION_KEY);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, key.as_concrete_TypeRef());
            if value.is_null() {
                return None;
            }
            let cf_type: CFType = TCFType::wrap_under_create_rule(value);
            if let Some(num) = cf_type.downcast::<CFNumber>() {
                // IOFixed is stored as integer, divide by 65536 to get actual value
                let raw: i64 = num.to_i64()?;
                Some(raw as f64 / 65536.0)
            } else {
                None
            }
        }
    }

    /// Set the pointer resolution (IOFixed format)
    /// Range: 10-1995 (clamped)
    pub fn set_resolution(&self, value: f64) -> Result<(), HidError> {
        let clamped = value.clamp(10.0, 1995.0);
        // Convert to IOFixed (16.16 fixed-point)
        let io_fixed = (clamped * 65536.0) as i64;

        let key = CFString::new(K_IOHID_POINTER_RESOLUTION_KEY);
        let cf_value = CFNumber::from(io_fixed);

        unsafe {
            let success = IOHIDServiceClientSetProperty(
                self.service,
                key.as_concrete_TypeRef(),
                cf_value.as_CFTypeRef(),
            );
            if success {
                // Trigger acceleration change to make resolution take effect (LinearMouse hack)
                // We need to re-write the acceleration value without changing linear scaling state
                self.trigger_acceleration_refresh();
                Ok(())
            } else {
                Err(HidError::PropertySetFailed(K_IOHID_POINTER_RESOLUTION_KEY.to_string()))
            }
        }
    }

    /// Refresh properties to trigger system to apply resolution changes
    /// This re-writes either linear scaling or acceleration depending on mode
    fn trigger_acceleration_refresh(&self) {
        // Check if we're in linear mode
        let linear_on = self.get_linear_scaling() == Some(1);

        if linear_on {
            // In linear mode, re-write the linear scaling property
            let key = CFString::new(K_IOHID_USE_LINEAR_SCALING_KEY);
            let cf_value = CFNumber::from(1i32);
            unsafe {
                IOHIDServiceClientSetProperty(
                    self.service,
                    key.as_concrete_TypeRef(),
                    cf_value.as_CFTypeRef(),
                );
            }
        } else {
            // In normal mode, re-write the acceleration property
            let accel_key = self.get_acceleration_type_key();
            let key = CFString::new(&accel_key);

            unsafe {
                let value = IOHIDServiceClientCopyProperty(self.service, key.as_concrete_TypeRef());
                if !value.is_null() {
                    IOHIDServiceClientSetProperty(self.service, key.as_concrete_TypeRef(), value);
                    core_foundation_sys::base::CFRelease(value);
                }
            }
        }
    }

    /// Get the current pointer acceleration
    /// Range: 0-20, or -1 if disabled (pre-Sonoma)
    pub fn get_acceleration(&self) -> Option<f64> {
        // Check if linear scaling is enabled (Sonoma+), which means acceleration is disabled
        if self.get_linear_scaling() == Some(1) {
            return Some(-1.0);
        }

        let accel_key = self.get_acceleration_type_key();
        let key = CFString::new(&accel_key);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, key.as_concrete_TypeRef());
            if !value.is_null() {
                let cf_type: CFType = TCFType::wrap_under_create_rule(value);
                if let Some(num) = cf_type.downcast::<CFNumber>() {
                    let raw: i64 = num.to_i64()?;
                    return Some(raw as f64 / 65536.0);
                }
            }
        }
        None
    }

    /// Set the pointer acceleration
    /// Range: 0-20, or -1 to disable (pre-Sonoma)
    pub fn set_acceleration(&self, value: f64) -> Result<(), HidError> {
        let clamped = if value == -1.0 {
            -1.0
        } else {
            value.clamp(0.0, 20.0)
        };
        let io_fixed = (clamped * 65536.0) as i64;

        let accel_key = self.get_acceleration_type_key();
        let key = CFString::new(&accel_key);
        let cf_value = CFNumber::from(io_fixed);

        unsafe {
            let success = IOHIDServiceClientSetProperty(
                self.service,
                key.as_concrete_TypeRef(),
                cf_value.as_CFTypeRef(),
            );
            if success {
                Ok(())
            } else {
                Err(HidError::PropertySetFailed("acceleration".to_string()))
            }
        }
    }

    /// Get linear scaling mode (macOS 14+ / Sonoma)
    /// 0 = acceleration enabled, 1 = acceleration disabled (linear)
    pub fn get_linear_scaling(&self) -> Option<i32> {
        let key = CFString::new(K_IOHID_USE_LINEAR_SCALING_KEY);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, key.as_concrete_TypeRef());
            if value.is_null() {
                return None;
            }
            let cf_type: CFType = TCFType::wrap_under_create_rule(value);
            if let Some(num) = cf_type.downcast::<CFNumber>() {
                num.to_i32()
            } else {
                None
            }
        }
    }

    /// Set linear scaling mode (macOS 14+ / Sonoma)
    /// true = disable acceleration (linear), false = enable acceleration
    pub fn set_linear_scaling(&self, linear: bool) -> Result<(), HidError> {
        let key = CFString::new(K_IOHID_USE_LINEAR_SCALING_KEY);
        let cf_value = CFNumber::from(if linear { 1i32 } else { 0i32 });

        unsafe {
            let success = IOHIDServiceClientSetProperty(
                self.service,
                key.as_concrete_TypeRef(),
                cf_value.as_CFTypeRef(),
            );
            if success {
                Ok(())
            } else {
                // This may fail on pre-Sonoma, which is expected
                Err(HidError::PropertySetFailed(K_IOHID_USE_LINEAR_SCALING_KEY.to_string()))
            }
        }
    }

    /// Convert user-facing speed (0.0-1.0) to HID resolution
    /// Speed 0 = Resolution 1200 (slowest)
    /// Speed 1 = Resolution 40 (fastest)
    pub fn speed_to_resolution(speed: f64) -> f64 {
        let clamped_speed = speed.clamp(0.0, 1.0);
        // Linear interpolation in inverse space
        // speed 0 -> 1/1200, speed 1 -> 1/40
        let min_rate = 1.0 / 1200.0;
        let max_rate = 1.0 / 40.0;
        let rate = min_rate + clamped_speed * (max_rate - min_rate);
        1.0 / rate
    }

    /// Convert HID resolution to user-facing speed (0.0-1.0)
    pub fn resolution_to_speed(resolution: f64) -> f64 {
        let min_rate = 1.0 / 1200.0;
        let max_rate = 1.0 / 40.0;
        let rate = 1.0 / resolution;
        ((rate - min_rate) / (max_rate - min_rate)).clamp(0.0, 1.0)
    }

    /// Set speed using user-facing value (0.0-1.0)
    /// In linear mode, sets tracking speed. In normal mode, sets resolution.
    pub fn set_speed(&self, speed: f64) -> Result<(), HidError> {
        let linear_on = self.get_linear_scaling() == Some(1);

        if linear_on {
            // In linear mode, speed is controlled by tracking speed (acceleration value)
            // Map 0-1 to a usable tracking speed range
            let tracking = Self::speed_to_tracking(speed);
            self.set_tracking_speed(tracking)
        } else {
            // In normal mode, speed is controlled by resolution
            let resolution = Self::speed_to_resolution(speed);
            self.set_resolution(resolution)
        }
    }

    /// Convert speed (0-1) to tracking speed for linear mode
    fn speed_to_tracking(speed: f64) -> f64 {
        // Map 0-1 to a usable range
        // speed 0.0 -> tracking 0.2 (very slow)
        // speed 0.5 -> tracking 1.0 (moderate)
        // speed 1.0 -> tracking 3.0 (fast)
        let clamped = speed.clamp(0.0, 1.0);
        0.2 + clamped * 2.8
    }

    /// Set tracking speed for linear mode (0.0-20.0 range, matching LinearMouse)
    /// This is the "Tracking speed" slider in LinearMouse when acceleration is disabled
    pub fn set_tracking_speed(&self, value: f64) -> Result<(), HidError> {
        let clamped = value.clamp(0.0, 20.0);
        self.set_acceleration_raw(clamped)
    }

    /// Get tracking speed (0.0-20.0 range) for linear mode
    pub fn get_tracking_speed(&self) -> Option<f64> {
        self.get_acceleration_raw()
    }

    /// Set acceleration without checking linear scaling state
    /// Used internally when we know we want to set the raw value
    fn set_acceleration_raw(&self, value: f64) -> Result<(), HidError> {
        let clamped = value.clamp(0.0, 20.0);
        let io_fixed = (clamped * 65536.0) as i64;

        let accel_key = self.get_acceleration_type_key();
        let key = CFString::new(&accel_key);
        let cf_value = CFNumber::from(io_fixed);

        unsafe {
            let success = IOHIDServiceClientSetProperty(
                self.service,
                key.as_concrete_TypeRef(),
                cf_value.as_CFTypeRef(),
            );
            if success {
                Ok(())
            } else {
                Err(HidError::PropertySetFailed("acceleration".to_string()))
            }
        }
    }

    /// Get raw acceleration value without linear scaling interpretation
    fn get_acceleration_raw(&self) -> Option<f64> {
        let accel_key = self.get_acceleration_type_key();
        let key = CFString::new(&accel_key);
        unsafe {
            let value = IOHIDServiceClientCopyProperty(self.service, key.as_concrete_TypeRef());
            if !value.is_null() {
                let cf_type: CFType = TCFType::wrap_under_create_rule(value);
                if let Some(num) = cf_type.downcast::<CFNumber>() {
                    let raw: i64 = num.to_i64()?;
                    return Some(raw as f64 / 65536.0);
                }
            }
        }
        None
    }
}

/// Manager for discovering and interacting with HID pointer devices
pub struct PointerDeviceManager {
    client: IOHIDEventSystemClientRef,
}

impl PointerDeviceManager {
    /// Create a new pointer device manager
    pub fn new() -> Result<Self, HidError> {
        unsafe {
            let client = IOHIDEventSystemClientCreate(kCFAllocatorDefault);
            if client.is_null() {
                return Err(HidError::ClientCreationFailed);
            }

            // Set up matching for mice and pointer devices
            let mouse_match = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::new(K_IOHID_DEVICE_USAGE_PAGE_KEY),
                    CFNumber::from(K_HID_PAGE_GENERIC_DESKTOP).as_CFType(),
                ),
                (
                    CFString::new(K_IOHID_DEVICE_USAGE_KEY),
                    CFNumber::from(K_HID_USAGE_GD_MOUSE).as_CFType(),
                ),
            ]);

            let pointer_match = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::new(K_IOHID_DEVICE_USAGE_PAGE_KEY),
                    CFNumber::from(K_HID_PAGE_GENERIC_DESKTOP).as_CFType(),
                ),
                (
                    CFString::new(K_IOHID_DEVICE_USAGE_KEY),
                    CFNumber::from(K_HID_USAGE_GD_POINTER).as_CFType(),
                ),
            ]);

            let matching_array = CFArray::from_CFTypes(&[mouse_match, pointer_match]);
            IOHIDEventSystemClientSetMatchingMultiple(client, matching_array.as_concrete_TypeRef());

            Ok(Self { client })
        }
    }

    /// Get all connected pointer devices (mice and trackpads)
    pub fn get_devices(&self) -> Result<Vec<PointerDevice>, HidError> {
        unsafe {
            let services_ref = IOHIDEventSystemClientCopyServices(self.client);
            if services_ref.is_null() {
                return Err(HidError::NoDevicesFound);
            }

            let services: CFArray<*const std::ffi::c_void> =
                TCFType::wrap_under_create_rule(services_ref);

            let count = services.len();
            if count == 0 {
                return Err(HidError::NoDevicesFound);
            }

            let mut devices = Vec::new();
            for i in 0..count {
                let service = CFArrayGetValueAtIndex(services.as_concrete_TypeRef(), i as isize) as IOHIDServiceClientRef;

                // Get device name
                let name = get_string_property(service, K_IOHID_PRODUCT_KEY)
                    .unwrap_or_else(|| "Unknown Device".to_string());

                // Get vendor/product IDs
                let vendor_id = get_int_property(service, K_IOHID_VENDOR_ID_KEY);
                let product_id = get_int_property(service, K_IOHID_PRODUCT_ID_KEY);

                devices.push(PointerDevice {
                    service,
                    name,
                    vendor_id,
                    product_id,
                });
            }

            // Keep services array alive by leaking it (devices hold references to services)
            std::mem::forget(services);

            Ok(devices)
        }
    }
}

fn get_string_property(service: IOHIDServiceClientRef, key: &str) -> Option<String> {
    let cf_key = CFString::new(key);
    unsafe {
        let value = IOHIDServiceClientCopyProperty(service, cf_key.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        let cf_type: CFType = TCFType::wrap_under_create_rule(value);
        cf_type.downcast::<CFString>().map(|s| s.to_string())
    }
}

fn get_int_property(service: IOHIDServiceClientRef, key: &str) -> Option<i64> {
    let cf_key = CFString::new(key);
    unsafe {
        let value = IOHIDServiceClientCopyProperty(service, cf_key.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        let cf_type: CFType = TCFType::wrap_under_create_rule(value);
        cf_type.downcast::<CFNumber>().and_then(|n| n.to_i64())
    }
}

impl Drop for PointerDeviceManager {
    fn drop(&mut self) {
        // The client is a Core Foundation object, release it
        if !self.client.is_null() {
            unsafe {
                core_foundation_sys::base::CFRelease(self.client as *const _);
            }
        }
    }
}
