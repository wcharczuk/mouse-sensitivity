# mouse-sensitivity

A command-line utility to set mouse sensitivity on macOS using HID APIs, similar to [LinearMouse](https://github.com/linearmouse/linearmouse).

## Features

- **List connected mouse/pointer devices** with vendor and product IDs
- **Disable/enable mouse acceleration** - uses linear scaling on macOS 14+
- **Set pointer speed** (0.0-1.0 scale) when acceleration is disabled
- **Set tracking speed** (1.0-20.0 scale) when acceleration is enabled

No sudo or special permissions required.

## Installation

### Build from source

```bash
# Clone the repository
git clone <this-repo>
cd mouse-sensitivity

# Build release version
cargo build --release

# Binary is at ./target/release/mouse-sensitivity
```

### Install globally (optional)

```bash
sudo cp ./target/release/mouse-sensitivity /usr/local/bin/
```

## Usage

```bash
# List all connected mouse/pointer devices
mouse-sensitivity list

# Get current settings for all devices
mouse-sensitivity get

# Get settings for a specific device (by index from list)
mouse-sensitivity get -d 0

# Disable acceleration and set speed (0.0-1.0)
mouse-sensitivity set --no-acceleration --speed 0.5

# Enable acceleration and set tracking speed (1.0-20.0)
mouse-sensitivity set --acceleration --tracking-speed 2.0

# Apply to a specific device only
mouse-sensitivity set --no-acceleration --speed 0.5 -d 0
```

## How It Works

This tool uses private IOKit HID APIs (`IOHIDServiceClient`) to interact with pointing devices at the driver level. It can:

1. **Modify acceleration**: Sets the `HIDMouseAcceleration` property. Setting to -1 disables acceleration on pre-Sonoma macOS.

2. **Linear scaling mode** (macOS 14+/Sonoma): Uses the `HIDUseLinearScalingMouseAcceleration` property for cleaner acceleration disabling.

3. **Pointer resolution**: Modifies the `HIDPointerResolution` property in IOFixed format (16.16 fixed-point). This controls the effective sensitivity/speed.

## Limitations

- **Settings are not persistent**: Settings reset when the device is disconnected or the system restarts. Run the command again or add it to a login script.

- **Per-app settings**: Unlike LinearMouse, this tool doesn't support per-application sensitivity profiles.

## Making Settings Persistent

Add to your shell profile (e.g., `~/.zshrc`) or create a login script:

```bash
# Apply on login (disable acceleration with speed 0.5)
mouse-sensitivity set --no-acceleration --speed 0.5
```

Or create a LaunchAgent for automatic application at login.

## Comparison to LinearMouse

| Feature | mouse-sensitivity | LinearMouse |
|---------|------------------|-------------|
| Disable acceleration | ✅ | ✅ |
| Set acceleration level | ✅ | ✅ |
| Set pointer speed | ✅ | ✅ |
| Per-app profiles | ❌ | ✅ |
| GUI | ❌ | ✅ |
| Menu bar | ❌ | ✅ |
| Scroll customization | ❌ | ✅ |
| Login item | ❌ | ✅ |

## Troubleshooting

### Device not found

Make sure your device is:
1. Connected and powered on
2. Recognized by the system (check System Information > USB/Bluetooth)
3. A mouse or pointer device (keyboards with trackpoints may not appear)

### Settings don't seem to apply

Try running `get` to verify the settings were applied:
```bash
mouse-sensitivity get -d 0
```

If the device shows different values than expected, the device may be resetting its own properties.

## Technical Details

The tool uses these HID properties:

| Property | Description | Range |
|----------|-------------|-------|
| `HIDPointerResolution` | Pointer speed in IOFixed format | 10-1995 |
| `HIDMouseAcceleration` | Acceleration multiplier in IOFixed | 0-20, or -1 to disable |
| `HIDUseLinearScalingMouseAcceleration` | Linear mode (Sonoma+) | 0 or 1 |

IOFixed format uses 16.16 fixed-point representation (value × 65536).

Speed is converted to resolution using: `resolution = 1 / (min_rate + speed * (max_rate - min_rate))`
where min_rate = 1/1200 and max_rate = 1/40.

## License

MIT
