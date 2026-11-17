//! Global compositor state: every Wayland global and the scene live here.

use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{PopupManager, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        dmabuf::{DmabufGlobal, DmabufState},
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::{backend::Backend, input::CmdKey, layout::Layout, render::RenderState};

pub struct State {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State>,
    pub loop_signal: LoopSignal,

    pub backend: Backend,
    pub layout: Layout,
    pub render_state: RenderState,
    /// Draw our own cursor (DRM). Nested, the host draws it.
    pub draw_cursor: bool,

    // Smithay protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub popups: PopupManager,
    pub seat: Seat<State>,

    /// Set whenever something on screen may have changed. Backends clear it
    /// after rendering a frame.
    pub redraw_needed: bool,
    pub cmd_key: CmdKey,
    /// Finger count of the touchpad swipe in progress (0 = none).
    pub swipe_fingers: u32,
}

impl State {
    pub fn new(
        event_loop: &mut EventLoop<'static, State>,
        display: Display<State>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        seat.add_keyboard(Default::default(), 300, 30)?;
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop)?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_handle: event_loop.handle(),
            loop_signal: event_loop.get_signal(),
            backend: Backend::None,
            layout: Layout::new(),
            render_state: RenderState::new(),
            draw_cursor: false,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            popups: PopupManager::default(),
            seat,
            redraw_needed: true,
            cmd_key: CmdKey::from_env(true),
            swipe_fingers: 0,
        })
    }

    fn init_wayland_listener(
        display: Display<State>,
        event_loop: &mut EventLoop<'static, State>,
    ) -> Result<OsString, Box<dyn std::error::Error>> {
        let socket = ListeningSocketSource::new_auto()?;
        let socket_name = socket.socket_name().to_os_string();
        let handle = event_loop.handle();

        handle.insert_source(socket, |stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!(?err, "failed to add client");
            }
        })?;

        handle.insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: `get_mut` is unsafe because dropping the Display
                // while calloop still polls its fd would be unsound. We only
                // dispatch here and never drop or replace it.
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                }
                Ok(PostAction::Continue)
            },
        )?;

        Ok(socket_name)
    }

    /// Called by calloop after every batch of events.
    pub fn on_idle(&mut self) {
        self.layout.refresh();
        self.popups.cleanup();
        let _ = self.display_handle.flush_clients();
        if self.redraw_needed {
            self.render_if_needed();
        }
    }

    /// Advertise linux-dmabuf so GPU-rendering clients can share buffers
    /// with us without copies.
    pub fn init_dmabuf(&mut self, formats: impl IntoIterator<Item = smithay::backend::allocator::Format>) {
        let formats: Vec<_> = formats.into_iter().collect();
        let global = self.dmabuf_state.create_global::<State>(&self.display_handle, formats);
        self.dmabuf_global = Some(global);
    }

    pub fn queue_redraw(&mut self) {
        self.redraw_needed = true;
    }

    pub fn add_output(&mut self, output: Output) {
        self.layout.add_output(&output);
        self.queue_redraw();
    }

    /// Topmost client surface under `pos` (global logical coordinates).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let space = &self.layout.active().space;
        space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<Window> {
        self.layout
            .active()
            .space
            .element_under(pos)
            .map(|(w, _)| w.clone())
    }

    /// Send frame callbacks so clients draw their next frame in step with us.
    pub fn post_render(&mut self, output: &Output) {
        let time = self.start_time.elapsed();
        for window in self.layout.active().space.elements() {
            window.send_frame(output, time, Some(std::time::Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
    }

    pub fn spawn(&self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else { return };
        if let Err(err) = std::process::Command::new(program)
            .args(parts)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .spawn()
        {
            tracing::warn!(?err, cmd, "failed to spawn");
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
