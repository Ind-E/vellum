# Vellum

Vellum is a small native Wayland overlay for drawing directly over the live desktop.

https://github.com/user-attachments/assets/f8171063-16a8-497f-ba20-9e11bc50727e

Vellum began as a fork of [Chameleos](https://github.com/Treeniks/chameleos) by Thomas Lindae.

## Compatibility

Vellum requires a Wayland compositor that implements [`wlr-layer-shell`](https://wayland.app/protocols/wlr-layer-shell-unstable-v1). This includes niri, Sway, Hyprland, river, Wayfire, labwc, KDE Plasma, and COSMIC, though niri is the only compositor currently tested.

## Installation

### Home Manager

Add Vellum to your flake inputs:

```nix
inputs.vellum.url = "github:greyxp1/vellum";
```

Then import the module in your Home Manager configuration:

```nix
{inputs, ...}: {
  imports = [inputs.vellum.homeModules.default];
  services.vellum.enable = true;
}
```

### Building and installing from source

Install the dependencies for your distribution:

<details>
<summary>Arch dependencies</summary>

```sh
sudo pacman -S --needed base-devel git rust wayland libxkbcommon fontconfig vulkan-icd-loader
```

For a PKGBUILD:

```bash
depends=('wayland' 'libxkbcommon' 'fontconfig' 'vulkan-icd-loader')
makedepends=('cargo')
```

</details>

<details>
<summary>Debian 13+ dependencies</summary>

```sh
sudo apt install build-essential git pkg-config rustup libwayland-dev libxkbcommon-dev libfontconfig-dev libvulkan1
rustup default stable
```

</details>

<details>
<summary>Fedora dependencies</summary>

```sh
sudo dnf install cargo gcc git pkgconf-pkg-config wayland-devel libxkbcommon-devel fontconfig-devel vulkan-loader
```

</details>

Then build and install Vellum:

```sh
git clone https://github.com/greyxp1/vellum
cd vellum
cargo xtask install
```

## Usage

Start Vellum manually with `vellum &` or run it as a service:

<details>
<summary>systemd</summary>

Save this as `~/.config/systemd/user/vellum.service`:

```ini
[Unit]
Description=Vellum screen annotation overlay
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=exec
ExecStart=%h/.local/bin/vellum
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

Enable the service and start it now:

```sh
systemctl --user enable --now vellum.service
```

</details>

Then bind `vellum toggle` to a compositor shortcut. For example, in niri:

```kdl
Mod+A { spawn "vellum" "toggle"; }
```

## Controls

### Mouse, trackpad, and pen

| Action | Mouse | Trackpad | Pen |
| --- | --- | --- | --- |
| Draw or manipulate the selection | Left drag | One-finger drag | Drag with the pen tip |
| Open the radial picker | Right-click | Two-finger click | Briefly press the barrel button while hovering |
| Choose a tool or color | Hold the right mouse button, move, then release | Open the picker, move, then one-finger click | Hold the barrel button while hovering, move, then release |
| Erase | Middle drag, or middle-click then left drag | Three-finger tap to toggle, then one-finger drag | Hold the barrel button while drawing or drag with the eraser tip |
| Change size | Mouse wheel | Two-finger scroll | Mouse wheel or trackpad scroll |

### Shortcuts and modifiers

| Input | Action |
| --- | --- |
| <kbd>Ctrl+Z</kbd> | Undo |
| <kbd>Ctrl+Shift+Z</kbd> or <kbd>Ctrl+Y</kbd> | Redo |
| Mouse back button | Undo |
| Mouse forward button | Redo |
| <kbd>Ctrl+A</kbd> | Select all annotations |
| <kbd>Backspace</kbd> or <kbd>Delete</kbd> | Delete selected annotations |
| <kbd>Escape</kbd> | Cancel, clear the selection, or leave drawing mode |
| <kbd>Ctrl</kbd> + click in selection mode | Add or remove an annotation from the selection |
| Double-click selected text | Edit it |
| <kbd>Shift</kbd> while drawing | Constrain the shape |
| <kbd>Alt</kbd> while drawing | Draw triangles, rectangles, and ellipses from their center |
| <kbd>F</kbd> | Toggle shape fill or text background |
| <kbd>Ctrl</kbd> + scroll | Change opacity |
| <kbd>Shift</kbd> + scroll | Change roundness |
| Drag a selection handle | Reshape the selection or stretch text |
| <kbd>Shift</kbd> + drag a text handle | Resize text without stretching |

While editing text:

| Input | Action |
| --- | --- |
| Arrow keys | Move the caret; <kbd>Ctrl+Left/Right</kbd> moves by word |
| <kbd>Home</kbd> / <kbd>End</kbd> | Move to the line's start / end; add <kbd>Ctrl</kbd> for the whole annotation |
| <kbd>Shift</kbd> + navigation key | Extend the text selection |
| <kbd>Ctrl+A</kbd> | Select all text in the annotation |
| Click / drag | Position the caret / select text; <kbd>Shift</kbd> + click extends the selection |
| Double-click / triple-click | Select a word / line |
| Typing, <kbd>Backspace</kbd>, or <kbd>Delete</kbd> | Replace or delete selected text |
| <kbd>Shift+Enter</kbd> | Insert a newline |
| <kbd>Enter</kbd> / <kbd>Escape</kbd> | Finish / cancel editing |

## Configuration

Vellum looks for `vellum/config.toml` in `$XDG_CONFIG_HOME` (default `~/.config`), then `$XDG_CONFIG_DIRS` (default `/etc/xdg`). Use `--config PATH` to load a specific file or `--no-config` to skip configuration.

### Options

#### Global

| Option | Type | Description |
| --- | --- | --- |
| `draw_on` | string | Where drawing can start: `all` monitors or only the `current` monitor |
| `default_tool` | string | Startup tool: `pen`, `line`, `arrow`, `triangle`, `rectangle`, `ellipse`, `text`, `eraser`, or `select` |
| `remember_last_tool` | boolean | Keep the selected tool when drawing mode is reopened |
| `stroke_size` | number | Initial size shared by pen, line, arrow, and shape tools |
| `size_range` | table | Optional `min`, `max`, `step`, and `stops` for scrolling through sizes and pausing at each stop |
| `default_color` | string | Initial [CSS color](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Values/color_value#syntax). It must be present in `palette` |
| `palette` | array of strings | Between 2 and 12 [CSS colors](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Values/color_value#syntax) |
| `feedback_duration_ms` | integer | How long property feedback remains visible, from `0` to `60000` milliseconds |
| `clear_on_escape` | boolean | Clear annotations when Escape deactivates drawing mode |
| `default_fill_shapes` | boolean | Initially fill triangles, rectangles, and ellipses |

#### Per-tool

Set these properties under `[tools.<tool>]`.

| Property | Type | Supported tools | Description |
| --- | --- | --- | --- |
| `size` | number | All except `select` | Initial logical-pixel size. Pen, line, arrow, and shapes inherit `stroke_size` |
| `size_range` | table | All except `select` | Overrides the matching global fields. `stops = []` removes inherited stops |
| `opacity` | number | All except `eraser` and `select` | Initial opacity from `0.05` to `1.0`. Overrides `default_color` alpha |
| `roundness` | number | `pen`, `line`, `arrow`, `triangle`, `rectangle`, `text` | Initial roundness from `0.0` to `1.0` |
| `filled` | boolean | `triangle`, `rectangle`, `ellipse` | Initial fill state |
| `background` | boolean | `text` | Whether text starts with an automatic black or white background |
| `font.family` | string | `text` | Ordered [CSS font-family list](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/font-family#values) |
| `font.weight` | number | `text` | [Weight](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/font-weight#common_weight_name_mapping) from `1` to `1000` |
| `font.style` | string | `text` | Font style: `"normal"`, `"italic"`, or `"oblique"` |
| `font.features` | table | `text` | [OpenType feature tags](https://sparanoid.com/lab/opentype-features/) (`0` disables, `1` enables). For example: `zero = 1`, `tnum = 1`, `liga = 0` |

### Defaults

See the [complete default configuration](default-config.toml). The source installer also places a copy under `~/.local/share/doc/vellum`.
