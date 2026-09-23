//! Interactive sensitivity calibration.
//!
//! The HID pointer filter turns sensor counts into cursor points; in linear
//! mode the ratio is proportional to the tracking speed. A mouse's sensor
//! resolution isn't readable over HID, but it is measurable: ask the user to
//! move the mouse a known physical distance and watch how far the cursor goes.
//!
//! Cursor position comes from CoreGraphics (`CGEventCreate` + `CGEventGetLocation`),
//! which works without the Input Monitoring permission that a raw HID event tap
//! would need.

use std::ffi::c_void;
use std::io::{self, BufRead, Write};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

type CGEventRef = *mut c_void;
type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut CGDirectDisplayID,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
}

/// Current cursor location in global display coordinates (points, origin at
/// the top-left of the main display).
pub fn cursor_position() -> Option<CGPoint> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        core_foundation_sys::base::CFRelease(event as *const c_void);
        Some(p)
    }
}

/// Bounds of every active display, in the same coordinate space as
/// [`cursor_position`].
pub fn display_bounds() -> Vec<CGRect> {
    const MAX: usize = 16;
    let mut ids = [0 as CGDirectDisplayID; MAX];
    let mut count: u32 = 0;
    unsafe {
        if CGGetActiveDisplayList(MAX as u32, ids.as_mut_ptr(), &mut count) != 0 {
            return Vec::new();
        }
        ids[..(count as usize).min(MAX)]
            .iter()
            .map(|&id| CGDisplayBounds(id))
            .collect()
    }
}

fn contains(r: &CGRect, p: CGPoint) -> bool {
    p.x >= r.origin.x
        && p.x <= r.origin.x + r.size.width
        && p.y >= r.origin.y
        && p.y <= r.origin.y + r.size.height
}

/// True when `p` sits on an outer edge of the display arrangement, meaning the
/// cursor was probably clamped there and the measured travel is too short.
/// Edges shared with a neighbouring display don't count: the cursor crosses
/// those freely.
pub fn on_outer_edge(p: CGPoint, displays: &[CGRect]) -> bool {
    const EDGE: f64 = 1.5;
    const PROBE: f64 = 4.0;
    let Some(r) = displays.iter().find(|r| contains(r, p)) else {
        return false;
    };
    let x0 = r.origin.x;
    let y0 = r.origin.y;
    let x1 = x0 + r.size.width;
    let y1 = y0 + r.size.height;
    let probes = [
        (p.x - x0 <= EDGE, CGPoint { x: x0 - PROBE, y: p.y }),
        (x1 - p.x <= EDGE, CGPoint { x: x1 + PROBE, y: p.y }),
        (p.y - y0 <= EDGE, CGPoint { x: p.x, y: y0 - PROBE }),
        (y1 - p.y <= EDGE, CGPoint { x: p.x, y: y1 + PROBE }),
    ];
    probes
        .iter()
        .any(|(near, beyond)| *near && !displays.iter().any(|d| contains(d, *beyond)))
}

/// Result of one calibration run.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// Mean cursor travel per centimetre of hand movement.
    pub points_per_cm: f64,
    /// Per-pass values that went into the mean.
    pub passes: Vec<f64>,
}

impl Measurement {
    pub fn from_passes(passes: Vec<f64>) -> Option<Self> {
        if passes.is_empty() {
            return None;
        }
        let points_per_cm = passes.iter().sum::<f64>() / passes.len() as f64;
        Some(Self { points_per_cm, passes })
    }

    /// Relative spread between the best and worst pass, as a fraction of the
    /// mean. `None` for a single pass.
    pub fn spread(&self) -> Option<f64> {
        if self.passes.len() < 2 || self.points_per_cm <= 0.0 {
            return None;
        }
        let max = self.passes.iter().cloned().fold(f64::MIN, f64::max);
        let min = self.passes.iter().cloned().fold(f64::MAX, f64::min);
        Some((max - min) / self.points_per_cm)
    }
}

/// Effective sensor resolution in counts per inch: the cursor travel the mouse
/// would produce per inch at tracking speed 1.0. Whatever constant scale macOS
/// applies in linear mode is folded in, which is fine — `match` only ever
/// uses the ratio between two devices measured the same way.
pub fn effective_dpi(points_per_cm: f64, tracking_speed: f64) -> f64 {
    points_per_cm * 2.54 / tracking_speed
}

/// Cursor travel per centimetre for a device with a known effective DPI at a
/// given tracking speed. Inverse of [`effective_dpi`].
pub fn points_per_cm(dpi: f64, tracking_speed: f64) -> f64 {
    dpi * tracking_speed / 2.54
}

/// Walk the user through `passes` measurements of `distance_cm` with the mouse
/// called `name` and average them. Prompts go to stdout; the user answers with
/// Enter on stdin.
pub fn measure(name: &str, distance_cm: f64, passes: u32) -> io::Result<Measurement> {
    let travels = measure_travel(name, &format!("{distance_cm} cm in a straight line"), passes)?;
    let per_cm: Vec<f64> = travels.iter().map(|t| t / distance_cm).collect();
    for (t, p) in travels.iter().zip(&per_cm) {
        println!("  {t:.0} pt → {p:.1} pt/cm");
    }
    Measurement::from_passes(per_cm).ok_or_else(|| io::Error::other("no passes recorded"))
}

/// Run `passes` measurements with the mouse called `name` and return the
/// cursor travel of each in points. `movement` completes the sentence
/// "Move the mouse …" in the prompt. The physical distance is the caller's
/// business: it may be a known length or an unmeasured span shared with
/// another mouse.
pub fn measure_travel(name: &str, movement: &str, passes: u32) -> io::Result<Vec<f64>> {
    let displays = display_bounds();
    let stdin = io::stdin();
    let mut out = io::stdout();
    let mut results = Vec::with_capacity(passes as usize);

    for n in 1..=passes {
        print!("[{name} {n}/{passes}] Put the mouse at the START of the span, then press Enter... ");
        out.flush()?;
        wait_for_enter(&stdin)?;
        let start = cursor_position()
            .ok_or_else(|| io::Error::other("could not read the cursor position"))?;

        print!("[{name} {n}/{passes}] Move the mouse {movement}, then press Enter... ");
        out.flush()?;
        wait_for_enter(&stdin)?;
        let end = cursor_position()
            .ok_or_else(|| io::Error::other("could not read the cursor position"))?;

        let travel = ((end.x - start.x).powi(2) + (end.y - start.y).powi(2)).sqrt();
        if travel < 1.0 {
            return Err(io::Error::other(format!(
                "the cursor did not move; make sure you moved {name} and not another mouse"
            )));
        }
        if on_outer_edge(end, &displays) {
            eprintln!(
                "warning: the cursor ended on a screen edge and was probably clamped; \
                 start further from the edge (or use a shorter span) and re-run"
            );
        }
        println!("cursor moved {travel:.0} pt");
        results.push(travel);
    }

    Ok(results)
}

/// Arithmetic mean; `None` for an empty slice.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// Everything `match` needs from a side-by-side comparison of two mice moved
/// across the same (unmeasured) span.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Sensor resolution of the new mouse relative to the reference:
    /// `dpi_new / dpi_ref`.
    pub ratio: f64,
    /// Tracking speed the new mouse needs to travel as far as the reference did.
    pub matched_tracking_speed: f64,
}

/// Compare a reference mouse (`ref_travel` points at `ref_speed`) with a new
/// one (`new_travel` points at `new_speed`) moved across the same span. The
/// span length cancels out, so it never has to be known.
pub fn compare(ref_travel: f64, ref_speed: f64, new_travel: f64, new_speed: f64) -> Comparison {
    // Travel per unit of hand movement is dpi × tracking speed, so dpi ∝ travel / speed.
    let ratio = (new_travel / new_speed) / (ref_travel / ref_speed);
    // Scale the new mouse's tracking speed by how much shorter/longer it went.
    let matched_tracking_speed = (new_speed * ref_travel / new_travel).clamp(0.0, 40.0);
    Comparison { ratio, matched_tracking_speed }
}

fn wait_for_enter(stdin: &io::Stdin) -> io::Result<()> {
    let mut line = String::new();
    if stdin.lock().read_line(&mut line)? == 0 {
        return Err(io::Error::other("stdin closed before calibration finished"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
    }

    #[test]
    fn effective_dpi_roundtrips_through_points_per_cm() {
        let dpi = effective_dpi(196.85, 0.5);
        assert!((dpi - 1000.0).abs() < 0.01, "{dpi}");
        let back = points_per_cm(dpi, 0.5);
        assert!((back - 196.85).abs() < 1e-9, "{back}");
    }

    #[test]
    fn compare_cancels_the_span_and_agrees_with_match_tracking_speed() {
        // Reference: 1800 DPI at 0.25; new mouse: 1000 DPI at 0.5, same 10 cm span.
        let span_in = 10.0 / 2.54;
        let ref_travel = 1800.0 * 0.25 * span_in;
        let new_travel = 1000.0 * 0.5 * span_in;
        let c = compare(ref_travel, 0.25, new_travel, 0.5);
        assert!((c.ratio - 1000.0 / 1800.0).abs() < 1e-9, "{}", c.ratio);
        // Same answer as match_tracking_speed(0.25, 1800, 1000) = 0.45.
        assert!((c.matched_tracking_speed - 0.45).abs() < 1e-9, "{}", c.matched_tracking_speed);
        // And a different span gives the same answer.
        let c2 = compare(ref_travel * 3.0, 0.25, new_travel * 3.0, 0.5);
        assert!((c.ratio - c2.ratio).abs() < 1e-9);
        assert!((c.matched_tracking_speed - c2.matched_tracking_speed).abs() < 1e-9);
    }

    #[test]
    fn mean_of_values() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[2.0, 4.0]), Some(3.0));
    }

    #[test]
    fn measurement_mean_and_spread() {
        let m = Measurement::from_passes(vec![190.0, 210.0]).unwrap();
        assert!((m.points_per_cm - 200.0).abs() < 1e-9);
        assert!((m.spread().unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(Measurement::from_passes(vec![5.0]).unwrap().spread(), None);
        assert!(Measurement::from_passes(vec![]).is_none());
    }

    #[test]
    fn outer_edge_detection_ignores_shared_seams() {
        // Two displays side by side: [0,1000) and [1000,2000).
        let displays = [rect(0.0, 0.0, 1000.0, 800.0), rect(1000.0, 0.0, 1000.0, 800.0)];
        // Right edge of the right display: outer.
        assert!(on_outer_edge(CGPoint { x: 1999.0, y: 400.0 }, &displays));
        // Seam between the two: not outer.
        assert!(!on_outer_edge(CGPoint { x: 999.5, y: 400.0 }, &displays));
        // Middle of a display: not an edge at all.
        assert!(!on_outer_edge(CGPoint { x: 500.0, y: 400.0 }, &displays));
        // Bottom edge: outer.
        assert!(on_outer_edge(CGPoint { x: 500.0, y: 799.0 }, &displays));
    }
}
