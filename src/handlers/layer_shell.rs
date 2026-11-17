//! wlr-layer-shell: panels, docks and — most importantly here — a global
//! menu bar client anchored to the top edge of the screen.

use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState},
    },
};

use crate::state::State;

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, output: Option<WlOutput>, _layer: Layer, namespace: String) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.layout.primary_output().cloned());
        let Some(output) = output else { return };
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            tracing::warn!(?err, "failed to map layer surface");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        for output in self.layout.all_outputs() {
            let mut map = layer_map_for_output(output);
            if let Some(layer) = map.layer_for_surface(surface.wl_surface(), smithay::desktop::WindowSurfaceType::TOPLEVEL).cloned() {
                map.unmap_layer(&layer);
            }
        }
        self.queue_redraw();
    }
}
delegate_layer_shell!(State);

impl State {
    /// Re-arrange the layer map and send the initial configure.
    pub fn handle_layer_commit(&mut self, surface: &WlSurface) {
        for output in self.layout.all_outputs() {
            let mut map = layer_map_for_output(output);
            let Some(layer) = map.layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL).cloned() else {
                continue;
            };
            map.arrange();
            let initial_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .map(|d| d.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });
            if !initial_sent {
                layer.layer_surface().send_configure();
            }
        }
    }
}
