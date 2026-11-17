//! quartzite — a macOS WindowServer-style Wayland compositor built on Smithay.
//!
//! Module map:
//! - `backend`   – where frames go (winit window for testing, DRM/KMS on a TTY)
//! - `handlers`  – Wayland protocol handlers (compositor, xdg-shell, seat, ...)
//! - `input`     – keyboard/pointer processing, key bindings, grabs
//! - `layout`    – workspaces ("Spaces") and window placement
//! - `render`    – turns the scene into a list of render elements each frame
//! - `animation` – spring physics used by every animation

mod backend;
mod handlers;
mod input;
mod layout;
mod platform;
mod render;
mod state;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

use crate::state::State;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;

    let mut state = State::new(&mut event_loop, display)?;

    // Nested when started from inside another session, unless --tty is given.
    let force_tty = std::env::args().any(|a| a == "--tty");
    let nested = !force_tty
        && (std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some());
    state.cmd_key = input::CmdKey::from_env(nested);
    if nested {
        backend::winit::init(&mut event_loop, &mut state)?;
    } else {
        #[cfg(feature = "udev")]
        backend::udev::init(&mut event_loop, &mut state)?;
        #[cfg(not(feature = "udev"))]
        return Err("built without the `udev` feature; run nested inside a session".into());
    }
    // Clients we spawn must talk to us, not to the host compositor.
    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    // Optional startup client: `quartzite -c foot`.
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-c" || arg == "--command" {
            if let Some(cmd) = args.next() {
                state.spawn(&cmd);
            }
        }
    }

    tracing::info!(socket = ?state.socket_name, "compositor running");
    event_loop.run(None, &mut state, |state| state.on_idle())?;
    Ok(())
}
