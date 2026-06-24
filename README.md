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

## Usage

```bash
# Is the daemon up?
mouse-sensitivity status

# List connected mice
mouse-sensitivity list

# Show current settings (per device)
mouse-sensitivity get

# Disable the macOS acceleration curve and set tracking speed (0.0–20.0)
# In linear mode, --acceleration IS the cursor speed.
mouse-sensitivity set --disable-acceleration --acceleration 0.6875

# Re-enable the macOS curve; now --acceleration is curve steepness and
# --speed (0.0–1.0, maps to HIDPointerResolution) is base sensitivity.
mouse-sensitivity set --enable-acceleration --acceleration 0.875 --speed 0.36

# Verify what the device actually reports (bypasses config)
mouse-sensitivity get --live

# Target a single device
mouse-sensitivity set -d 0 --disable-acceleration --speed 0.36

# Stop the daemon (launchd will restart it if installed)
mouse-sensitivity stop
```

`set` is incremental: any flag you omit keeps its previously-saved value for that device.

## Settings model (matches LinearMouse)

| Control | Range | HID property | Effect |
|---|---|---|---|
| `--disable-acceleration` / `--enable-acceleration` | bool | `HIDUseLinearScalingMouseAcceleration` | toggles the curve |
| `--acceleration` | 0.0–40.0 | `HIDMouseAcceleration` (or the device's `HIDPointerAccelerationType`), IOFixed | **Linear mode: this is the cursor speed.** Accelerated mode: curve steepness. |
| `--speed` | 0.0–1.0 | `HIDPointerResolution` = `1/(1/1200 + s·(1/40 − 1/1200))` | Base sensitivity. **No effect in linear mode** on macOS 14+. |

Properties are written in that order; the final acceleration write doubles as the refresh trigger that makes resolution take effect (the same hack LinearMouse uses).

To replicate a LinearMouse config: its "Disable pointer acceleration" checkbox is `--disable-acceleration`, its "Tracking speed" slider is `--acceleration`, and its "Pointer speed" slider is `--speed`.

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
{"cmd":"set","device":0,"disable_acceleration":true,"acceleration":0.6875,"speed":0.36}
{"cmd":"reload"}
{"cmd":"shutdown"}
```

Responses are `{"status":"ok"}`, `{"status":"pong","version":"…"}`, `{"status":"devices","devices":[…]}`, or `{"status":"error","message":"…"}`.

## Comparison to LinearMouse

| | mouse-sensitivity | LinearMouse |
|---|---|---|
| Disable acceleration | ✅ | ✅ |
| Acceleration slider (0–20) | ✅ | ✅ |
| Speed slider (0–1 → resolution) | ✅ | ✅ |
| Persistent daemon | ✅ | ✅ |
| Re-apply on hotplug | ✅ | ✅ |
| Per-app profiles | ❌ | ✅ |
| Scroll customization | ❌ | ✅ |
| GUI / menu bar | ❌ | ✅ |

## License

MIT
