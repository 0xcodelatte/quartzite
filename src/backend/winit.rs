//! Nested backend: runs the compositor inside a window of an existing
//! Wayland or X11 session. Used for development and testing.

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::Transform,
};

use crate::{backend::Backend, render, state::State};

pub struct WinitData {
    pub backend: WinitGraphicsBackend<GlesRenderer>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
}

pub fn init(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, winit_loop) = winit::init::<GlesRenderer>()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "quartzite".into(),
            model: "nested".into(),
        },
    );
    output.create_global::<State>(&state.display_handle);
    // GL's origin is bottom-left; Flipped180 makes the nested window upright.
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);
    state.backend = Backend::Winit(WinitData {
        backend,
        output: output.clone(),
        damage_tracker,
    });
    state.add_output(output.clone());

    event_loop
        .handle()
        .insert_source(winit_loop, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let mode = Mode { size, refresh: 60_000 };
                output.change_current_state(Some(mode), None, None, None);
                output.set_preferred(mode);
                state.queue_redraw();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => render_frame(state),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })?;

    Ok(())
}

fn render_frame(state: &mut State) {
    let State { backend, layout, .. } = state;
    let Backend::Winit(data) = backend else { return };
    let output = data.output.clone();

    let age = data.backend.buffer_age().unwrap_or(0);
    let damage = {
        let (renderer, mut framebuffer) = match data.backend.bind() {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(?err, "failed to bind winit framebuffer");
                return;
            }
        };
        let elements = render::output_elements(renderer, layout, &data.output);
        match data.damage_tracker.render_output(
            renderer,
            &mut framebuffer,
            age,
            &elements,
            render::CLEAR_COLOR,
        ) {
            Ok(result) => result.damage.cloned(),
            Err(err) => {
                tracing::error!(?err, "render failed");
                None
            }
        }
    };

    // Damage tracking: only present (and only repaint) the changed regions.
    if let Some(damage) = damage {
        if let Err(err) = data.backend.submit(Some(&damage)) {
            tracing::warn!(?err, "failed to submit frame");
        }
    }

    state.redraw_needed = false;
    state.post_render(&output);
}
