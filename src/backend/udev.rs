//! TTY backend: DRM/KMS for display, GBM for buffers, libseat for the
//! session and libinput for input. Works on Linux and FreeBSD (via
//! drm-kmod, seatd and libudev-devd).
//!
//! Frame pacing: we render at most one frame per output per vblank. After
//! queueing a frame we wait for the page-flip (VBlank) event before
//! rendering again, so animations are naturally vsync-paced.

use std::{collections::HashMap, path::Path};

use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{gles::GlesRenderer, ImportDma, Renderer, TextureFilter},
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{self, UdevBackend, UdevEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, RegistrationToken},
        drm::control::{connector, crtc, Device as ControlDevice, ModeTypeFlags},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Transform},
};

use crate::{backend::Backend, platform, render, state::State};

type GbmDrmCompositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

pub struct UdevData {
    pub session: LibSeatSession,
    gpu: Option<Gpu>,
}

struct Gpu {
    node: DrmNode,
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlesRenderer,
    surfaces: HashMap<crtc::Handle, OutputSurface>,
    _token: RegistrationToken,
}

struct OutputSurface {
    output: Output,
    compositor: GbmDrmCompositor,
    /// A frame is queued and we are waiting for its vblank.
    frame_pending: bool,
}

impl UdevData {
    pub fn renderer(&mut self) -> Option<&mut GlesRenderer> {
        self.gpu.as_mut().map(|g| &mut g.renderer)
    }
}

pub fn init(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::check_environment();
    tracing::info!("starting DRM backend on {}", platform::OS_NAME);

    let (session, notifier) = LibSeatSession::new()?;
    let seat_name = session.seat();

    // Pick the GPU that drives the boot display.
    let primary = match std::env::var("QUARTZITE_DRM_DEVICE") {
        Ok(path) => Some(path.into()),
        Err(_) => udev::primary_gpu(&seat_name)
            .ok()
            .flatten()
            .or_else(|| platform::fallback_drm_cards().into_iter().next()),
    }
    .ok_or("no GPU found")?;

    state.backend = Backend::Udev(UdevData { session: session.clone(), gpu: None });
    state.draw_cursor = true;

    // --- input ---------------------------------------------------------
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat_name)
        .map_err(|_| "failed to assign libinput seat")?;
    let input_backend = LibinputInputBackend::new(libinput.clone());
    event_loop
        .handle()
        .insert_source(input_backend, |event, _, state| state.process_input_event(event))?;

    // --- VT switches ---------------------------------------------------
    event_loop.handle().insert_source(notifier, move |event, _, state| {
        let Backend::Udev(data) = &mut state.backend else { return };
        match event {
            SessionEvent::PauseSession => {
                libinput.suspend();
                if let Some(gpu) = &mut data.gpu {
                    gpu.drm.pause();
                }
            }
            SessionEvent::ActivateSession => {
                if let Err(err) = libinput.resume() {
                    tracing::error!(?err, "failed to resume libinput");
                }
                if let Some(gpu) = &mut data.gpu {
                    if let Err(err) = gpu.drm.activate(false) {
                        tracing::error!(?err, "failed to activate DRM device");
                    }
                    for surface in gpu.surfaces.values_mut() {
                        let _ = surface.compositor.reset_state();
                        surface.frame_pending = false;
                    }
                }
                state.queue_redraw();
            }
        }
    })?;

    // --- display -------------------------------------------------------
    let udev_backend = UdevBackend::new(&seat_name)?;
    for (dev_id, path) in udev_backend.device_list() {
        if path == primary.as_path() {
            if let Ok(node) = DrmNode::from_dev_id(dev_id) {
                add_gpu(event_loop, state, node, path)?;
            }
        }
    }
    if matches!(&state.backend, Backend::Udev(d) if d.gpu.is_none()) {
        // udev did not list it (e.g. a minimal FreeBSD setup): open directly.
        let node = DrmNode::from_path(&primary)?;
        add_gpu(event_loop, state, node, &primary)?;
    }

    // Monitor hotplug: rescan connectors when the GPU reports a change.
    event_loop.handle().insert_source(udev_backend, |event, _, state| {
        if let UdevEvent::Changed { .. } = event {
            scan_connectors(state);
        }
    })?;

    Ok(())
}

fn add_gpu(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
    node: DrmNode,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let Backend::Udev(data) = &mut state.backend else { unreachable!() };
    let fd = data
        .session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;

    // SAFETY: `EGLDisplay::new` is unsafe because EGL hands out the same
    // display for the same native handle, so foreign code calling
    // `eglTerminate` on it would invalidate ours. No other code in this
    // process creates EGL displays, so that cannot happen here.
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let context = EGLContext::new(&egl_display)?;
    // SAFETY: `GlesRenderer::new` requires that nothing else makes this
    // context current or changes its GL state behind the renderer's back.
    // The context is moved into the renderer and never shared.
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    renderer.downscale_filter(TextureFilter::Linear)?;

    let token = event_loop.handle().insert_source(notifier, move |event, _, state| match event {
        DrmEvent::VBlank(crtc) => state.on_vblank(crtc),
        DrmEvent::Error(err) => tracing::error!(?err, "DRM error"),
    })?;

    data.gpu = Some(Gpu {
        node,
        drm,
        gbm,
        renderer,
        surfaces: HashMap::new(),
        _token: token,
    });

    let formats = data.renderer().unwrap().dmabuf_formats();
    state.init_dmabuf(formats);
    scan_connectors(state);
    Ok(())
}

/// Light up every connected monitor that does not have an output yet.
fn scan_connectors(state: &mut State) {
    let Backend::Udev(data) = &mut state.backend else { return };
    let Some(gpu) = &mut data.gpu else { return };
    let Ok(resources) = gpu.drm.resource_handles() else { return };

    let mut new_outputs = Vec::new();
    for conn in resources.connectors() {
        let Ok(info) = gpu.drm.get_connector(*conn, true) else { continue };
        if info.state() != connector::State::Connected {
            continue;
        }
        let already = gpu
            .surfaces
            .values()
            .any(|s| s.compositor.surface().current_connectors().into_iter().any(|c| c == *conn));
        if already {
            continue;
        }
        let Some(mode) = info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
            .copied()
        else {
            continue;
        };

        // Find a free CRTC this connector can drive.
        let crtc = info
            .encoders()
            .iter()
            .filter_map(|e| gpu.drm.get_encoder(*e).ok())
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .find(|c| !gpu.surfaces.contains_key(c));
        let Some(crtc) = crtc else { continue };

        let surface = match gpu.drm.create_surface(crtc, mode, &[*conn]) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(?err, "failed to create DRM surface");
                continue;
            }
        };

        let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let (w_mm, h_mm) = info.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (w_mm as i32, h_mm as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "unknown".into(),
                model: name,
            },
        );
        let output_mode = OutputMode::from(mode);
        output.change_current_state(Some(output_mode), Some(Transform::Normal), Some(render::hidpi::scale_from_env()), None);
        output.set_preferred(output_mode);

        let allocator = GbmAllocator::new(gpu.gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        let exporter = GbmFramebufferExporter::new(gpu.gbm.clone(), Some(gpu.node).into());
        let render_formats = gpu.renderer.egl_context().dmabuf_render_formats().clone();
        let compositor = match DrmCompositor::new(
            &output,
            surface,
            None,
            allocator,
            exporter,
            [Fourcc::Argb8888, Fourcc::Xrgb8888],
            render_formats,
            gpu.drm.cursor_size(),
            Some(gpu.gbm.clone()),
        ) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(?err, "failed to create DRM compositor");
                continue;
            }
        };

        gpu.surfaces.insert(crtc, OutputSurface { output: output.clone(), compositor, frame_pending: false });
        new_outputs.push(output);
    }

    for output in new_outputs {
        tracing::info!(output = output.name(), "output connected");
        output.create_global::<State>(&state.display_handle);
        state.add_output(output);
    }
}

impl State {
    /// Render every output that is not already waiting for a vblank.
    pub fn render_udev(&mut self) {
        let crtcs: Vec<crtc::Handle> = match &self.backend {
            Backend::Udev(UdevData { gpu: Some(gpu), .. }) => gpu
                .surfaces
                .iter()
                .filter(|(_, s)| !s.frame_pending)
                .map(|(c, _)| *c)
                .collect(),
            _ => return,
        };
        for crtc in crtcs {
            self.render_surface(crtc);
        }
    }

    fn render_surface(&mut self, crtc: crtc::Handle) {
        let pointer = self.seat.get_pointer().map(|p| p.current_location());
        let draw_cursor = self.draw_cursor;
        let State { backend, layout, render_state, .. } = self;
        let Backend::Udev(UdevData { gpu: Some(gpu), session }) = backend else { return };
        if !session.is_active() {
            return;
        }
        let Some(surface) = gpu.surfaces.get_mut(&crtc) else { return };

        let elements = render::output_elements(&mut gpu.renderer, render_state, layout, &surface.output, pointer.filter(|_| draw_cursor));
        let output = surface.output.clone();
        match surface
            .compositor
            .render_frame(&mut gpu.renderer, &elements, render::CLEAR_COLOR, FrameFlags::DEFAULT)
        {
            Ok(result) if !result.is_empty => match surface.compositor.queue_frame(()) {
                Ok(()) => surface.frame_pending = true,
                Err(err) => tracing::warn!(?err, "failed to queue frame"),
            },
            Ok(_) => {} // nothing changed: damage tracking skipped the frame
            Err(err) => tracing::warn!(?err, "failed to render frame"),
        }

        // Keep rendering on every vblank while animations are running.
        self.redraw_needed = self.render_state.animating;
        self.post_render(&output);
    }

    fn on_vblank(&mut self, crtc: crtc::Handle) {
        if let Backend::Udev(UdevData { gpu: Some(gpu), .. }) = &mut self.backend {
            if let Some(surface) = gpu.surfaces.get_mut(&crtc) {
                if let Err(err) = surface.compositor.frame_submitted() {
                    tracing::warn!(?err, "frame_submitted failed");
                }
                surface.frame_pending = false;
            }
        }
        if self.redraw_needed {
            self.render_surface(crtc);
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Backend::Udev(data) = &mut self.backend {
            if let Err(err) = data.session.change_vt(vt) {
                tracing::warn!(?err, vt, "failed to switch VT");
            }
        }
    }
}
