# quartzite

A Wayland compositor in Rust (Smithay 0.7) that behaves like macOS's
WindowServer: every window is composited from its own buffer, with soft
shadows, rounded corners, spring animations, 2x-then-downscale HiDPI,
Spaces, Exposé, background blur and a reserved menu bar. Runs on Linux and
FreeBSD.

Each feature is its own git commit, in the order requested:

| Commit | Feature |
|---|---|
| Phase 1  | Minimal compositor: winit backend, xdg-shell, input, move/resize |
| Phase 1b | DRM/KMS backend: libseat, libinput, GBM, dmabuf, cursor |
| Phase 2  | Shadows and rounded corners (custom GLES shaders) |
| Phase 3  | Spring animations: open, close, minimize, zoom, focus |
| Phase 4  | HiDPI: render at 2x, downscale to fractional scales |
| Phase 5  | Spaces with a slide transition (keys + 3-finger swipe) |
| Phase 6  | Exposé |
| Phase 7  | Wallpaper + background blur |
| Phase 8  | Layer-shell + reserved menu bar area |

`git log --oneline` shows them; `git diff <phase>~1 <phase>` shows what
each added.

## Architecture

```
            input events                         client buffers
 libinput/winit ──► input/ ──► layout/ ◄── handlers/ ◄── Wayland clients
                     (keys,     (Spaces,      (xdg-shell, layer-shell,
                      focus,     windows,      dmabuf, seat)
                      grabs)     Exposé)
                                    │
                    animation/ ───► render/ ──► list of render elements
                    (springs)       (shadows, corners, blur, HiDPI)
                                    │
                                 backend/ ──► winit window or DRM/KMS
                                 (damage-tracked, one frame per vblank)
```

- **Every window is offscreen.** Clients draw into their own buffers
  (shm or dmabuf). The compositor samples them as textures and builds the
  final frame; nothing draws directly to the screen.
- **Render elements, front to back.** `render::output_elements` returns a
  list: cursor, panels, menu bar, then per window its rounded surfaces,
  blur and shadow, then the wallpaper. Smithay's damage tracker compares
  this list with the last frame and repaints only what changed.
- **Shaders.** `render/shaders/`: `shadow.frag` (analytic blurred
  rounded-rect), `rounded.frag` (clips client textures; corners are
  removed from opaque regions so damage stays correct), `blur.frag`,
  `wallpaper.frag`.
- **Animations.** `animation::Animated` is a damped spring sampled once per
  frame. While any spring is moving the backend asks for another frame at
  the next vblank, so motion is vsync-paced with no timers.
- **HiDPI like macOS.** With `QUARTZITE_SCALE=1.5`, clients are told
  "scale 2"; the scene is composed into a 2x offscreen texture and drawn
  downscaled. Integer scales skip the extra pass.
- **Blur.** The wallpaper is rendered once into a quarter-size texture;
  blurred regions sample it with a Gaussian. Cheap, but it blurs only the
  wallpaper, not windows underneath (same trade-off as niri's "xray").
- **Portability.** Sessions via libseat (seatd or logind), input via
  libinput, display via DRM/KMS. The only OS-specific code is in
  `src/platform.rs`, behind `cfg(target_os)`.

### Unsafe code

There are three `unsafe` blocks, each with a `SAFETY:` comment:
1. `state.rs`: `Display::get_mut` to dispatch clients. It's unsafe because
   dropping the display while calloop polls its fd would be unsound; we
   never drop it.
2. `backend/udev.rs`: `EGLDisplay::new`. EGL shares displays per native
   handle, so foreign code could terminate ours; no other EGL code runs.
3. `backend/udev.rs`: `GlesRenderer::new`. The renderer must own the GL
   context exclusively; the context is moved into it.

## System packages

**Debian / Ubuntu**
```
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev \
  libudev-dev libinput-dev libgbm-dev libseat-dev libegl-dev libgles-dev libdrm-dev
```
**Fedora**
```
sudo dnf install gcc pkgconf-pkg-config wayland-devel libxkbcommon-devel \
  systemd-devel libinput-devel mesa-libgbm-devel libseat-devel mesa-libEGL-devel libdrm-devel
```
**Arch**
```
sudo pacman -S base-devel wayland libxkbcommon libinput mesa seatd
```
**FreeBSD**
```
pkg install rust pkgconf wayland libxkbcommon libinput seatd libudev-devd \
  mesa-libs mesa-dri drm-kmod
```

Rust 1.85 or newer.

## Build

```
cargo build --release                         # full build (DRM backend included)
cargo build --release --no-default-features   # nested-only build, no libseat/udev needed
cargo test                                    # spring physics tests
```

## Run

**Nested (any OS, for testing).** Start it from inside an existing
Wayland or X11 session; it opens as a window:
```
./target/release/quartzite -c foot
```
In nested mode ⌘ is **Alt** (the host usually grabs Super).

**On a TTY, Linux.** Log in on a text console, make sure seatd (or
logind) is running, then:
```
./target/release/quartzite --tty -c foot
```

**On a TTY, FreeBSD.**
```
sysrc kld_list+=i915kms        # or amdgpu; then reboot or kldload it
sysrc seatd_enable=YES && service seatd start
pw groupmod video -m $USER
sysctl kern.evdev.rcpt_mask=12 # let libinput see mice and keyboards
export XDG_RUNTIME_DIR=/var/run/user/$(id -u)   # create it (mode 700) if missing
./target/release/quartzite --tty -c foot
```

### Settings (environment variables)

| Variable | Effect |
|---|---|
| `QUARTZITE_SCALE=1.5` | Output scale; fractional values use 2x + downscale |
| `QUARTZITE_BLUR=1` | Blur behind translucent windows (the menu bar is always blurred) |
| `QUARTZITE_CMD=alt\|super` | Which key acts as ⌘ |
| `QUARTZITE_DRM_DEVICE=/dev/dri/card1` | Pick the GPU |
| `TERMINAL=foot` | Terminal for ⌘+Return |
| `XCURSOR_THEME`, `XCURSOR_SIZE` | Cursor (TTY only) |

### Key bindings (⌘ = Super on a TTY, Alt nested)

| Keys | Action |
|---|---|
| Click | Focus and raise (focused window gets the stronger shadow) |
| ⌘ Q / ⌘ W | Close window |
| ⌘ M / ⌘ Shift M | Minimize / restore last minimized |
| ⌘ F | Zoom (maximize) toggle, animated |
| ⌘ Return | Terminal |
| Ctrl ← / → , Ctrl 1–4 | Switch Space |
| Ctrl Shift ← / → | Move window to the next Space |
| 3-finger swipe | Drag between Spaces (TTY, touchpad) |
| Ctrl ↑ or F3 | Exposé; click a window to pick it, Esc to leave |
| Ctrl Alt Backspace | Quit |
| Ctrl Alt F1–F12 | Switch VT (TTY) |

A menu bar client is any wlr-layer-shell panel anchored to the top with an
exclusive zone (e.g. `waybar`). Without one, a translucent bar is drawn and
its 28 px stay reserved.

## What was tested

- Compiles with Rust 1.91 against Smithay 0.7.0, with and without the
  `udev` feature. `cargo test` passes.
- Nested mode was run under Xvfb (software GL) with `weston-terminal` and
  `weston-simple-shm`. Screenshots confirmed corners, shadows, open,
  minimize, Exposé, Space sliding, 1.5x scaling, blur and the menu bar.
- The DRM/KMS backend compiles, but has not been run on real hardware
  yet, on Linux or FreeBSD. Treat it as the first thing to test.

## Known limitations

- Single GPU; no multi-GPU or DRM leasing. Multiple monitors are placed
  left to right, and the menu bar, placement and Exposé use the first one.
- Blur samples only the wallpaper, not windows behind.
- No real global-menu protocol (KDE appmenu / DBusMenu) yet; the bar area
  is reserved for a layer-shell client.
- No popup grabs, no XWayland, no screencopy. Rotated outputs are untested.
- OpenBSD: no udev, so only the nested build (`--no-default-features`) is
  expected to work.
- Under Xvfb/llvmpipe, Smithay logs a harmless `BAD_SURFACE` error each
  frame (no buffer-age support); real drivers don't do this.
