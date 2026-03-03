pub mod input;
pub mod layout;
pub mod renderer;
pub mod text;
pub mod theme;

use input::InputHandler;
use layout::LayoutNode;
use renderer::Renderer;
use text::TextEngine;
use theme::Theme;

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};
use epoca_dsl::{
    eval_app, exec_actions, handle_bind, init_state, parse, CallbackEntry, EvalResult, StateStore,
    ZmlApp,
};
use epoca_protocol::*;

/// GPU state created on resume, dropped on suspend.
struct GpuState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
}

/// Main application state.
pub struct App {
    zml_app: ZmlApp,
    state: StateStore,
    callbacks: Vec<CallbackEntry>,
    event_queue: Vec<GuestEvent>,
    text_engine: TextEngine,
    theme: Theme,
    input_handler: InputHandler,
    current_layout: Option<LayoutNode>,
    current_tree: Option<ViewTree>,
    needs_render: bool,
    gpu: Option<GpuState>,
    /// Last cursor position for desktop mouse→touch mapping.
    cursor_pos: (f32, f32),
    #[allow(dead_code)]
    zml_source: String,
}

impl App {
    pub fn new(zml_source: String) -> anyhow::Result<Self> {
        let zml_app = parse(&zml_source).map_err(|e| anyhow::anyhow!("ZML parse error: {e}"))?;
        let mut state = StateStore::new();
        init_state(&zml_app.state_block, &mut state);

        let text_engine = TextEngine::new();
        let theme = Theme::default();

        Ok(Self {
            zml_app,
            state,
            callbacks: Vec::new(),
            event_queue: Vec::new(),
            text_engine,
            theme,
            input_handler: InputHandler::new(),
            current_layout: None,
            current_tree: None,
            needs_render: true,
            gpu: None,
            cursor_pos: (0.0, 0.0),
            zml_source,
        })
    }

    /// Evaluate ZML → ViewTree → Layout → mark for render.
    fn evaluate_and_layout(&mut self, width: f32, height: f32) {
        let result: EvalResult = eval_app(&self.zml_app, &self.state);
        self.callbacks = result.callbacks;

        let layout_root =
            layout::layout(&result.tree, (width, height), &mut self.text_engine, &self.theme);
        self.input_handler.set_layout(layout_root.clone());
        self.current_layout = Some(layout_root);
        self.current_tree = Some(result.tree);
        self.needs_render = true;
    }

    /// Process queued guest events.
    fn pump_events(&mut self) {
        if self.event_queue.is_empty() {
            return;
        }

        let events: Vec<GuestEvent> = self.event_queue.drain(..).collect();
        let mut state_changed = false;

        for event in &events {
            // Find the callback entry.
            let entry = self
                .callbacks
                .iter()
                .find(|c| c.callback_id == event.callback_id);

            if let Some(entry) = entry {
                if entry.actions.is_empty() {
                    // Bind callback — update bound state variable.
                    if let Some(tree) = &self.current_tree {
                        if let Some(node) = find_node(&tree.root, event.callback_id) {
                            handle_bind(&node.props, &mut self.state, &event.data);
                            state_changed = true;
                        }
                    }
                } else {
                    // Execute handler actions.
                    if let Err(e) = exec_actions(&entry.actions.clone(), &mut self.state, &event.data)
                    {
                        log::error!("Action error: {e}");
                    }
                    state_changed = true;
                }
            }
        }

        if state_changed {
            if let Some(gpu) = &self.gpu {
                let size = gpu.window.inner_size();
                self.evaluate_and_layout(size.width as f32, size.height as f32);
            }
        }
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(WindowAttributes::default().with_title("Epoca"))
                .expect("failed to create window"),
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");

        let (adapter, device, queue) = pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .expect("no suitable GPU adapter");

            let (device, queue) = adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("epoca"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                            .using_resolution(adapter.limits()),
                        ..Default::default()
                    },
                    None,
                )
                .await
                .expect("failed to create device");

            (adapter, device, queue)
        });

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let renderer = Renderer::new(&device, &queue, format);
        renderer.resize(&queue, size.width as f32, size.height as f32);

        self.gpu = Some(GpuState {
            window,
            surface,
            config,
            device,
            queue,
            renderer,
        });

        // Initial evaluate + layout.
        self.evaluate_and_layout(size.width as f32, size.height as f32);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            self.init_gpu(event_loop);
        }
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Drop GPU resources — Android can reclaim the window.
        self.gpu = None;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.config.width = new_size.width;
                        gpu.config.height = new_size.height;
                        gpu.surface.configure(&gpu.device, &gpu.config);
                        gpu.renderer
                            .resize(&gpu.queue, new_size.width as f32, new_size.height as f32);
                    }
                    self.evaluate_and_layout(new_size.width as f32, new_size.height as f32);
                    if let Some(gpu) = &self.gpu {
                        gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::Touch(touch) => {
                let x = touch.location.x as f32;
                let y = touch.location.y as f32;
                match touch.phase {
                    TouchPhase::Started => {
                        self.input_handler.touch_down(x, y);
                    }
                    TouchPhase::Ended => {
                        if let Some(event) = self.input_handler.touch_up(x, y) {
                            self.event_queue.push(event);
                        }
                    }
                    _ => {}
                }
                self.pump_events();
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            // Desktop: map mouse to touch events.
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, .. } => {
                let (x, y) = self.cursor_pos;
                match state {
                    ElementState::Pressed => {
                        self.input_handler.touch_down(x, y);
                    }
                    ElementState::Released => {
                        if let Some(event) = self.input_handler.touch_up(x, y) {
                            self.event_queue.push(event);
                        }
                        self.pump_events();
                        if let Some(gpu) = &self.gpu {
                            gpu.window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.pump_events();

                if let Some(gpu) = &mut self.gpu {
                    if let Some(layout) = &self.current_layout {
                        let output = match gpu.surface.get_current_texture() {
                            Ok(t) => t,
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                gpu.surface.configure(&gpu.device, &gpu.config);
                                return;
                            }
                            Err(e) => {
                                log::error!("Surface error: {e}");
                                return;
                            }
                        };

                        let view = output
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default());

                        gpu.renderer.render_frame(
                            &gpu.device,
                            &gpu.queue,
                            &view,
                            layout,
                            &mut self.text_engine,
                            &self.theme,
                        );

                        output.present();
                        self.needs_render = false;
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Process any remaining events.
        if !self.event_queue.is_empty() {
            self.pump_events();
            if let Some(gpu) = &self.gpu {
                gpu.window.request_redraw();
            }
        }
    }
}

/// Find a ViewNode by callback_id (for bind lookups).
fn find_node(node: &ViewNode, callback_id: CallbackId) -> Option<&ViewNode> {
    for cb in &node.callbacks {
        if cb.id == callback_id {
            return Some(node);
        }
    }
    for child in &node.children {
        if let Some(found) = find_node(child, callback_id) {
            return Some(found);
        }
    }
    None
}

/// Desktop entry point for testing the Android renderer pipeline.
#[cfg(not(target_os = "android"))]
pub fn desktop_main(zml_path: &str) -> anyhow::Result<()> {
    env_logger::init();

    let source = std::fs::read_to_string(zml_path)
        .map_err(|e| anyhow::anyhow!("Failed to read {zml_path}: {e}"))?;

    let mut app = App::new(source)?;
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Android entry point.
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(android_logger::Config::default().with_min_level(log::Level::Info));

    // Load ZML from bundled assets — default to a built-in counter.
    let source = include_str!("../../../examples/counter.zml").to_string();

    let mut app = match App::new(source) {
        Ok(app) => app,
        Err(e) => {
            log::error!("Failed to init app: {e}");
            return;
        }
    };

    let event_loop = EventLoop::builder()
        .with_android_app(android_app)
        .build()
        .expect("failed to build event loop");

    event_loop.run_app(&mut app).expect("event loop error");
}
