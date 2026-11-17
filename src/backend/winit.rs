//! Nested backend: runs the compositor inside a window of an existing
//! Wayland or X11 session. Used for development and testing.

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma, Renderer, TextureFilter},
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
    let (mut backend, winit_loop) = winit::init::<GlesRenderer>()?;

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
    output.change_current_state(Some(mode), Some(Transform::Flipped180), Some(render::hidpi::scale_from_env()), Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);
    // Linear filtering for the 2x → fractional downscale.
    backend.renderer().downscale_filter(TextureFilter::Linear)?;
    let formats = backend.renderer().dmabuf_formats();
    state.init_dmabuf(formats);
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
    let State { backend, layout, render_state, .. } = state;
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
        let elements = render::output_elements(renderer, render_state, layout, &data.output, None);
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

    // Keep drawing while animations run; each redraw waits for vsync.
    state.redraw_needed = state.render_state.animating;
    state.post_render(&output);
}
