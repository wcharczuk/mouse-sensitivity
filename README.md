# mouse-sensitivity

A command-line utility and background daemon for setting mouse sensitivity on macOS, using the same private IOKit HID APIs as [LinearMouse](https://github.com/linearmouse/linearmouse).

## Why a daemon?

Writing `HIDPointerResolution` from a one-shot process doesn't stick: the value reverts as soon as the `IOHIDEventSystemClient` that wrote it is released, and again whenever the device reconnects or WindowServer reasserts its own settings. LinearMouse solves this by keeping a client alive on a run loop and re-applying on device hotplug. This tool does the same — the `daemon` subcommand owns a long-lived client scheduled on a CFRunLoop, re-applies on `IOHIDEventSystemClientRegisterDeviceMatchingCallback`, and re-asserts every few seconds as a safety net.

The CLI talks to the daemon over a Unix domain socket; if the daemon isn't running, the CLI applies directly and warns that the change won't persist.

## Install

```bash
cargo build --release
./target/release/mouse-sensitivity install   # writes a LaunchAgent and loads it
```

`install` writes `~/Library/LaunchAgents/com.wcharczuk.mouse-sensitivity.plist` pointing at the current binary path and runs `launchctl load -w` so the daemon starts now and at every login. `uninstall` reverses it.

Rebuilding while the daemon is running is fine: the kernel kills the old process the moment its binary changes on disk (`last exit reason = OS_REASON_CODESIGNING` in `launchctl print`), and launchd's `KeepAlive` starts the new one within a few seconds. If `status` says the daemon is down right after a build, wait a moment or run `launchctl kickstart gui/$UID/com.wcharczuk.mouse-sensitivity`.

## Usage

```bash
# Is the daemon up?
mouse-sensitivity status

# List connected mice
mouse-sensitivity list

# Show current settings (per device)
mouse-sensitivity get

# Disable the macOS acceleration curve and set tracking speed (0.0–40.0)
# In linear mode, --tracking-speed IS the cursor speed.
mouse-sensitivity set --disable-acceleration --tracking-speed 0.6875

# Re-enable the macOS curve; now --tracking-speed is curve steepness and
# --speed (0.0–1.0, maps to HIDPointerResolution) is base sensitivity.
mouse-sensitivity set --enable-acceleration --tracking-speed 0.875 --speed 0.36

# Verify what the device actually reports (bypasses config)
mouse-sensitivity get --live

# Target a single device
mouse-sensitivity set -d 0 --disable-acceleration --speed 0.36

# Stop the daemon (launchd will restart it if installed)
mouse-sensitivity stop
```

`set` is incremental: any flag you omit keeps its previously-saved value for that device. A device with no config entry yet is seeded from its live values, so a partial `set` never silently changes the knobs you didn't name.

## Calibrating a new mouse to feel like an old one

Tracking speed alone doesn't transfer between mice. In linear mode the cursor moves `sensor DPI × tracking speed` points per inch of hand movement, and every mouse has a different sensor. Once both DPIs are known, `match` solves for the tracking speed. There are three ways to get a DPI: the built-in table of factory defaults, asking the mouse, and measuring.

### By proxy: factory defaults

Most mice ship at a documented DPI and most people never change it. The tool knows the out-of-the-box value for common mice (Razer ships everything at 1800, Logitech's MX line at 1000, the Magic Mouse is 1300; `dpi --list-known` prints the table), and `match` falls back to it for any mouse without a recorded DPI. So with the reference mouse in the config from an earlier `set` and the new mouse connected, no measuring is needed:

```bash
mouse-sensitivity match --from razer --dry-run   # show the math, apply nothing
mouse-sensitivity match --from razer             # set the new mouse's tracking speed
```

The reference doesn't need to be plugged in. Each line of the output says whether a DPI was recorded or is a factory default. To record a table value so it shows up in `get`: `dpi -d 0 --known --save` for a connected mouse, or `dpi --saved razer --save` for one that isn't.

A factory default is only right if the DPI stage was never changed, by a DPI button or the vendor's software. If it was, get the real value one of the ways below.

### By measuring

```bash
# 1. With the reference mouse connected, measure it: the tool asks you to put the
#    mouse at a mark, press Enter, move it 10 cm along a ruler, press Enter.
mouse-sensitivity calibrate
#    …or, if you know its DPI from the vendor software, skip the measurement:
mouse-sensitivity set -d 0 --dpi 1800

# 2. With the new mouse connected, measure it the same way.
mouse-sensitivity calibrate --distance-cm 10 --passes 2

# 3. Set the new mouse's tracking speed so it travels the same distance per cm.
#    --from is a case-insensitive substring of a name in the config; the
#    reference mouse does not need to be plugged in.
mouse-sensitivity match --from razer
mouse-sensitivity match --from razer --dry-run   # just show the math
```

### No ruler?

Four ways around it, in order of preference:

- **Ask the mouse.** Razer and Logitech mice will tell you their sensor DPI over their vendor protocols (the same queries openrazer and Solaar use). `dpi --save` reads it and records it, no measuring at all:

  ```bash
  mouse-sensitivity dpi -d 0 --save     # each mouse in turn
  mouse-sensitivity match --from razer
  ```

  macOS gates opening a pointing device behind the Input Monitoring permission, so the first run prompts you to allow your terminal under System Settings → Privacy & Security → Input Monitoring. Logitech mice must be connected directly (Bluetooth or cable), not through a Unifying/Bolt receiver. If the mouse can't be asked (another vendor, no permission yet, a Logitech behind a receiver), `dpi` falls back to the factory-default table and says so; `dpi --known` skips the ask entirely and never triggers the permission prompt. Don't mix this with a measured `calibrate` on another mouse unless you've confirmed the two scales agree: the reading is the hardware count rate, while `calibrate` records cursor travel at tracking speed 1.0, and macOS may apply a constant factor between them.
- **Compare the two mice directly.** With both connected, `calibrate --against razer` (index or name substring of the reference) asks you to move the reference and then the new mouse across the *same* span: a mousepad edge to edge, the width of your keyboard, two marks on the desk. The span's length cancels out of the math, so it never has to be known. Add `--apply` to set the matching tracking speed in the same step. If the reference already has a DPI on record it anchors the numbers; otherwise both are stored on a shared relative scale, which is all `match` needs.
- **Use a credit card.** Every ISO card is 8.56 cm on its long edge: `calibrate --distance-cm 8.56`. That fits on screen at ordinary tracking speeds and is about as accurate as a ruler.
- **Type in the DPI.** If you know a mouse's sensor setting (its onboard DPI stage, or the value in the vendor's software), `set -d <index> --dpi <value>` records it without any measuring.

`calibrate` reads the cursor position through CoreGraphics, so it needs no Input Monitoring permission. Because it measures cursor travel rather than raw sensor counts, the number it records is *effective* DPI: cursor points per inch at tracking speed 1.0. That is exactly the quantity `match` needs, and it cancels out whatever constant scale macOS applies in linear mode. Both mice must be in linear mode; `get` shows the stored DPI and the resulting cursor travel per centimetre.

Tips for a clean measurement: keep the cursor well away from the screen edges (the tool warns if it ends on one), move in a straight line, and press Enter on the keyboard rather than clicking. A two-pass average at 10 cm is typically within a couple of percent, which is below what you can feel.

## Settings model (matches LinearMouse)

| Control | Range | HID property | Effect |
|---|---|---|---|
| `--disable-acceleration` / `--enable-acceleration` | bool | `HIDUseLinearScalingMouseAcceleration` | toggles the curve |
| `--tracking-speed` (alias `--acceleration`) | 0.0–40.0 | `HIDMouseAcceleration` (or the device's `HIDPointerAccelerationType`), IOFixed | **Linear mode: this is the cursor speed.** Accelerated mode: curve steepness. Stored as `tracking_speed` in the config (`acceleration` is still read). |
| `--speed` | 0.0–1.0 | `HIDPointerResolution` = `1/(1/1200 + s·(1/40 − 1/1200))` | Base sensitivity. **No effect in linear mode** on macOS 14+. |
| `--dpi` | counts/inch | (none; stored in config) | Effective sensor resolution: measured by `calibrate`, read by `dpi`, or typed in. Only used by `match`, which falls back to a factory default when it's missing. |

Properties are written in that order; the final acceleration write doubles as the refresh trigger that makes resolution take effect (the same hack LinearMouse uses).

To replicate a LinearMouse config: its "Disable pointer acceleration" checkbox is `--disable-acceleration`, its "Tracking speed" slider is `--tracking-speed`, and its "Pointer speed" slider is `--speed`.

### Verifying

`get` shows what the daemon will enforce (from config). `get --live` reads the HID properties directly from the device — use that to confirm a change actually landed.

## Files

- Socket: `~/Library/Application Support/mouse-sensitivity/daemon.sock`
- Config: `~/Library/Application Support/mouse-sensitivity/config.json`
- Log: `~/Library/Application Support/mouse-sensitivity/daemon.log` (when run via launchd)
- LaunchAgent: `~/Library/LaunchAgents/com.wcharczuk.mouse-sensitivity.plist`

The config is JSON, keyed by `(vendor_id, product_id)`. You can edit it by hand and run `mouse-sensitivity set --reload` is not needed — the daemon picks up edits via the `set` command, or send `{"cmd":"reload"}` over the socket.

## IPC protocol

Newline-delimited JSON over the Unix socket. Requests:

```json
{"cmd":"ping"}
{"cmd":"list"}
{"cmd":"get","device":0}
{"cmd":"set","device":0,"disable_acceleration":true,"tracking_speed":0.6875,"speed":0.36,"dpi":1000}
{"cmd":"reload"}
{"cmd":"shutdown"}
```

Responses are `{"status":"ok"}`, `{"status":"pong","version":"…"}`, `{"status":"devices","devices":[…]}`, or `{"status":"error","message":"…"}`.

## Comparison to LinearMouse

| | mouse-sensitivity | LinearMouse |
|---|---|---|
| Disable acceleration | ✅ | ✅ |
| Tracking speed slider | ✅ | ✅ |
| Speed slider (0–1 → resolution) | ✅ | ✅ |
| Persistent daemon | ✅ | ✅ |
| Re-apply on hotplug | ✅ | ✅ |
| Per-app profiles | ❌ | ✅ |
| Scroll customization | ❌ | ✅ |
| GUI / menu bar | ❌ | ✅ |

## License

MIT
