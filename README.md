# Falling Sand Screensaver

A screensaver that simulates sand falling over obstacles.

## Requirements

- A Rust toolchain (stable, edition 2021): <https://rustup.rs>
- A GPU with a Vulkan, DirectX 12 or OpenGL driver (anything wgpu supports)

## Build

```powershell
cargo build --release
```

The executable will end up at `target\release\screensaver_sand.exe`. On my machine the release takes about a minute to build, but the debug build is much slower while running so the minute is worth it.


## Run

Just run the executable. Add /c to get the config menu

The screensaver quits on any key press, mouse click, mouse wheel, or a mouse
move of more than 12 pixels. Input is ignored for the first 1.5 seconds so the
launch itself does not close it.

## Install as a Windows screensaver

1. Build in release mode.
2. Copy the executable and rename it to `.scr`
3. Right-click `screensaver_sand.scr` and choose **Install**. Windows opens the
   screensaver settings with it selected. (Alternatively, copy the file to
   `C:\Windows\System32`, which needs administrator rights.)
4. In **Screen Saver Settings**, choose "screensaver_sand", set the wait time,
   and press **Settings...** to open the settings window described below.

To remove it, delete the `.scr` file (from `System32` if you put it there) and
pick another screensaver. Your saved settings stay in `%APPDATA%` (see below).

## Settings window

`screensaver_sand.exe /c` (or **Settings...** in Windows) opens a normal window
with the simulation running behind a menu. Every change applies immediately so
you can watch the effect. Settings are saved when you exit.

### Controls

| Key | Action |
| --- | --- |
| Up / Down | Move through a list; on an adjustment screen, change the value |
| Right or Enter | Open a folder or setting |
| Left or Esc | Go back (Esc at the top level saves and exits) |
| Shift | Adjust 10 times faster |
| Ctrl | Adjust 100 times faster |
| Shift + Ctrl | Adjust 1000 times faster |
| Backspace | Reset the setting being adjusted to its default |
| Tab | Hide the menu (any key shows it again) |
| R | Restart the simulation with the current settings |

Closing the window also saves.

### Folders

| Folder | Contents |
| --- | --- |
| Presets | Load, save and manage presets |
| Obstacles | Count, scale, thickness, lifetimes, spacing, holes |
| Obstacle kinds | How often each kind is built (slopes, diamonds, circles, funnels, slots, round holes) |
| Streams | Count, density, width, duration |
| Influence | How streams and obstacles attract each other, chaining |
| Weirdness | Chances of unusual kinds, placement, shapes, overlaps, redirects, loops |
| Colors | Lightness, chroma, hue start, hue range, colour speed |

Most settings have a matching "variation" setting: the amount each new item
may differ from the average. Count settings of 0 mean "auto" (chosen from the
screen size).

### Presets

- Built-in presets: Default, Slow, Cascade, Boulders, Hourglass, Chaos, Rain
  and Magma.
- **Enter** on a preset loads it. **Right** opens its menu: Load, and Copy to
  custom for built-ins; Load, Overwrite with current, Rename and Delete (asks
  to confirm) for your own presets.
- **+ Save current as new** stores the current settings as a custom preset.
  Names are up to 20 characters: letters, digits, space, `-`, `_` and `.`.
- The line "Preset: ..." at the top shows the preset that matches the current
  settings, "(modified)" once you change it, or "Unsaved" if none matches.

## Where settings are stored

`%APPDATA%\screensaver_sand\`

- `settings.txt`: the settings used by the screensaver (`key=value` lines)
- `presets.txt`: your custom presets

Both are plain text. Unknown keys and invalid values are ignored, and values
are clamped to their allowed range. Deleting the folder restores the defaults.

## AI

This was made almost entirely with AI