use anyhow::{anyhow, Context, Result};
use polkavm::{CallError, Config, Engine, Instance, Linker, Module, ProgramBlob};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use epoca_protocol::{
    deserialize_view_tree, serialize_event, GuestEvent, ViewTree,
};

/// Shared ring buffer for audio: interleaved i16 stereo samples (L, R, L, R, …).
/// Fed by `host_audio_submit` on the game thread, drained by the cpal callback.
type AudioRing = Arc<Mutex<VecDeque<i16>>>;

/// 2 seconds of stereo 44100 Hz audio — maximum buffer before we drop oldest samples.
const AUDIO_RING_MAX: usize = 44100 * 2 * 2;

/// An input event for framebuffer guests (8 bytes, packed).
///
/// Event types:
///   1 = key down,  2 = key up        → key_code set, mouse_x/y zero
///   3 = mouse down, 4 = mouse up     → key_code = button (1=left,2=right,3=middle), mouse_x/y set
///   5 = mouse move                   → key_code zero, mouse_x/y set
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct InputEvent {
    /// Event discriminant (see above).
    pub event_type: u8,
    /// Key scancode (events 1-2) or mouse button (events 3-4), 0 otherwise.
    pub key_code: u8,
    /// Mouse X position in guest framebuffer coordinates.
    pub mouse_x: u16,
    /// Mouse Y position in guest framebuffer coordinates.
    pub mouse_y: u16,
    pub _pad: [u8; 2],
}

/// Configuration for the sandbox.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub backend: SandboxBackend,
    /// Maximum PolkaVM gas units per `call_update()` tick.
    /// Prevents a looping guest from blocking the GPUI main thread indefinitely.
    /// Default: 50_000_000 (enough for typical UI logic at 30 fps).
    /// Set to `u64::MAX` to disable the limit (only for trusted/dev guests).
    pub max_gas_per_update: u64,
}

#[derive(Debug, Clone)]
pub enum SandboxBackend {
    Auto,
    Interpreter,
    Compiler,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: SandboxBackend::Auto,
            max_gas_per_update: 50_000_000,
        }
    }
}

/// Shared state between host functions and the sandbox instance.
struct HostState {
    /// The latest view tree emitted by the guest.
    view_tree: Option<ViewTree>,
    /// Events queued for the guest to poll.
    event_queue: VecDeque<GuestEvent>,
    /// Network fetch requests from the guest (url, response_callback).
    pending_fetches: Vec<(String, u64)>,
    /// Framebuffer: (ARGB pixels, width, height) — set by `host_present_frame`.
    framebuffer: Option<(Vec<u8>, u32, u32)>,
    /// Input events queued for the guest (framebuffer mode).
    input_queue: VecDeque<InputEvent>,
    /// Assets loaded from a .prod bundle, keyed by relative path.
    assets: HashMap<String, Vec<u8>>,
    /// Time origin for `host_time_ms`.
    time_origin: std::time::Instant,
    /// Set by `host_yield` to signal cooperative yield from the guest.
    yield_requested: bool,
    /// Ring buffer shared with the cpal audio callback thread.
    audio_ring: AudioRing,
}

impl HostState {
    fn new(audio_ring: AudioRing) -> Self {
        Self {
            view_tree: None,
            event_queue: VecDeque::new(),
            pending_fetches: Vec::new(),
            framebuffer: None,
            input_queue: VecDeque::new(),
            assets: HashMap::new(),
            time_origin: std::time::Instant::now(),
            yield_requested: false,
            audio_ring,
        }
    }
}

/// Maximum bytes we will allocate for a single guest memory read.
const MAX_GUEST_READ: usize = 64 * 1024 * 1024; // 64 MiB

/// Helper to read guest memory into a Vec<u8>.
fn read_guest_memory(
    instance: &polkavm::RawInstance,
    ptr: u32,
    len: u32,
) -> Result<Vec<u8>, anyhow::Error> {
    let size = len as usize;
    if size > MAX_GUEST_READ {
        return Err(anyhow!("Guest memory read of {} bytes exceeds limit", size));
    }
    let mut buf = vec![0u8; size];
    instance
        .read_memory_into(ptr, buf.as_mut_slice())
        .map_err(|e| anyhow!("Memory read error: {:?}", e))?;
    Ok(buf)
}

/// Build a cpal output stream that drains samples from the shared audio ring buffer.
/// Returns `None` if no audio device is available (audio will be silently disabled).
fn build_audio_stream(audio_ring: &AudioRing) -> Option<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host.default_output_device()?;

    let config = cpal::StreamConfig {
        channels: 2,
        sample_rate: cpal::SampleRate(44100),
        buffer_size: cpal::BufferSize::Default,
    };

    let ring = Arc::clone(audio_ring);
    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                let mut buf = ring.lock().unwrap();
                for sample in output.iter_mut() {
                    *sample = buf.pop_front().unwrap_or(0);
                }
            },
            |err| log::error!("[audio] cpal stream error: {err}"),
            None,
        )
        .ok()?;

    stream.play().ok()?;
    log::info!("[audio] cpal output stream started (44100 Hz stereo i16)");
    Some(stream)
}

/// A running PolkaVM sandbox instance.
pub struct SandboxInstance {
    instance: Instance<HostState, anyhow::Error>,
    state: Arc<Mutex<HostState>>,
    /// Gas limit applied before each `call_update()` tick.
    max_gas_per_update: u64,
    /// True when the guest yielded mid-execution via `host_yield`.
    /// The next `call_update` will resume execution instead of calling "update".
    yielded: bool,
    /// Keeps the cpal audio stream alive. Dropped when the sandbox is dropped.
    _audio_stream: Option<SendStream>,
}

/// Wrapper to make cpal::Stream Send. CoreAudio streams are internally
/// thread-safe (callback runs on its own thread), but contain raw pointers
/// that prevent auto-Send. This is safe because we only hold the stream
/// alive — we never access its internals from another thread.
struct SendStream(cpal::Stream);
unsafe impl Send for SendStream {}

impl SandboxInstance {
    /// Create a new sandbox from a .polkavm program blob.
    pub fn from_bytes(blob_bytes: &[u8], config: &SandboxConfig) -> Result<Self> {
        let blob = ProgramBlob::parse(blob_bytes.into())
            .context("Failed to parse PolkaVM program blob")?;

        let mut engine_config = Config::from_env().unwrap_or_default();
        match config.backend {
            SandboxBackend::Compiler => {
                engine_config.set_backend(Some(polkavm::BackendKind::Compiler));
            }
            SandboxBackend::Interpreter => {
                engine_config.set_backend(Some(polkavm::BackendKind::Interpreter));
            }
            SandboxBackend::Auto => {
                // Default to compiler (JIT) when available.
                engine_config.set_backend(Some(polkavm::BackendKind::Compiler));
            }
        }
        // The generic sandbox (macOS/non-Linux) requires this flag.
        engine_config.set_allow_experimental(true);
        let engine = Engine::new(&engine_config).map_err(|e| {
            log::error!("[sandbox] Engine::new error: {e}");
            e
        }).context("Failed to create PolkaVM engine")?;

        // Enable synchronous gas metering so call_update() can be interrupted
        // when a guest loops indefinitely (CallError::NotEnoughGas is returned).
        let mut module_config = polkavm::ModuleConfig::default();
        module_config.set_gas_metering(Some(polkavm::GasMeteringKind::Sync));

        // On Apple Silicon the native page size is 16KB; PolkaVM defaults to 4KB
        // which is incompatible with the JIT compiler's generic sandbox.
        #[cfg(target_os = "macos")]
        module_config.set_page_size(16384);

        let module = Module::from_blob(&engine, &module_config, blob).map_err(|e| {
            log::error!("[sandbox] Module::from_blob error: {e}");
            e
        }).context("Failed to compile PolkaVM module")?;

        let mut linker: Linker<HostState, anyhow::Error> = Linker::new();

        // host_set_view(ptr: u32, len: u32) -> u32
        linker
            .define_typed(
                "host_set_view",
                |caller: polkavm::Caller<'_, HostState>, ptr: u32, len: u32| -> Result<u32, anyhow::Error> {
                    let buf = read_guest_memory(caller.instance, ptr, len)?;

                    match deserialize_view_tree(&buf) {
                        Ok(tree) => {
                            caller.user_data.view_tree = Some(tree);
                            Ok(0)
                        }
                        Err(e) => {
                            log::error!("Failed to deserialize view tree: {}", e);
                            Ok(1)
                        }
                    }
                },
            )
            .context("Failed to define host_set_view")?;

        // host_poll_event(buf_ptr: u32, buf_len: u32) -> u32
        linker
            .define_typed(
                "host_poll_event",
                |caller: polkavm::Caller<'_, HostState>, buf_ptr: u32, buf_len: u32| -> Result<u32, anyhow::Error> {
                    if let Some(event) = caller.user_data.event_queue.pop_front() {
                        let bytes = serialize_event(&event)
                            .map_err(|e| anyhow!("Serialize error: {}", e))?;
                        let write_len = bytes.len().min(buf_len as usize);
                        caller
                            .instance
                            .write_memory(buf_ptr, &bytes[..write_len])
                            .map_err(|e| anyhow!("Memory write error: {:?}", e))?;
                        Ok(write_len as u32)
                    } else {
                        Ok(0)
                    }
                },
            )
            .context("Failed to define host_poll_event")?;

        // host_fetch(url_ptr: u32, url_len: u32, callback_id: u32) -> u32
        linker
            .define_typed(
                "host_fetch",
                |caller: polkavm::Caller<'_, HostState>, url_ptr: u32, url_len: u32, callback_id: u32| -> Result<u32, anyhow::Error> {
                    let buf = read_guest_memory(caller.instance, url_ptr, url_len)?;

                    match String::from_utf8(buf) {
                        Ok(url) => {
                            caller
                                .user_data
                                .pending_fetches
                                .push((url, callback_id as u64));
                            Ok(0)
                        }
                        Err(_) => Ok(1),
                    }
                },
            )
            .context("Failed to define host_fetch")?;

        // host_log(ptr: u32, len: u32)
        linker
            .define_typed(
                "host_log",
                |caller: polkavm::Caller<'_, HostState>, ptr: u32, len: u32| -> Result<(), anyhow::Error> {
                    let buf = read_guest_memory(caller.instance, ptr, len)?;
                    let msg = String::from_utf8_lossy(&buf);
                    log::info!("[guest] {}", msg);
                    Ok(())
                },
            )
            .context("Failed to define host_log")?;

        // host_present_frame(ptr: u32, width: u32, height: u32, stride: u32) -> u32
        // Reads width*height*4 ARGB bytes from guest memory and stores in HostState.
        linker
            .define_typed(
                "host_present_frame",
                |caller: polkavm::Caller<'_, HostState>, ptr: u32, width: u32, height: u32, _stride: u32| -> Result<u32, anyhow::Error> {
                    let byte_len = width.checked_mul(height).and_then(|n| n.checked_mul(4));
                    let Some(byte_len) = byte_len else {
                        return Ok(1); // overflow
                    };
                    let buf = read_guest_memory(caller.instance, ptr, byte_len)?;
                    caller.user_data.framebuffer = Some((buf, width, height));
                    Ok(0)
                },
            )
            .context("Failed to define host_present_frame")?;

        // host_poll_input(buf_ptr: u32, buf_len: u32) -> u32
        // Pops InputEvents from the queue, writes into guest memory, returns bytes written.
        // Each event is 8 bytes: [event_type, key_code, mouse_x_lo, mouse_x_hi, mouse_y_lo, mouse_y_hi, pad, pad]
        linker
            .define_typed(
                "host_poll_input",
                |caller: polkavm::Caller<'_, HostState>, buf_ptr: u32, buf_len: u32| -> Result<u32, anyhow::Error> {
                    let max_events = (buf_len as usize) / 8; // InputEvent is 8 bytes
                    let mut written = 0u32;
                    for _ in 0..max_events {
                        let Some(evt) = caller.user_data.input_queue.pop_front() else {
                            break;
                        };
                        let mx = evt.mouse_x.to_le_bytes();
                        let my = evt.mouse_y.to_le_bytes();
                        let bytes = [evt.event_type, evt.key_code, mx[0], mx[1], my[0], my[1], evt._pad[0], evt._pad[1]];
                        caller
                            .instance
                            .write_memory(buf_ptr + written, &bytes)
                            .map_err(|e| anyhow!("Memory write error: {:?}", e))?;
                        written += 8;
                    }
                    Ok(written)
                },
            )
            .context("Failed to define host_poll_input")?;

        // host_time_ms() -> u64
        // Returns milliseconds since sandbox creation.
        linker
            .define_typed(
                "host_time_ms",
                |caller: polkavm::Caller<'_, HostState>| -> Result<u64, anyhow::Error> {
                    Ok(caller.user_data.time_origin.elapsed().as_millis() as u64)
                },
            )
            .context("Failed to define host_time_ms")?;

        // host_yield()
        // Cooperative yield: guest calls this to suspend execution and return control to the host.
        // The host can resume execution later via continue_execution().
        linker
            .define_typed(
                "host_yield",
                |caller: polkavm::Caller<'_, HostState>| -> Result<(), anyhow::Error> {
                    caller.user_data.yield_requested = true;
                    Err(anyhow!("__yield__"))
                },
            )
            .context("Failed to define host_yield")?;

        // host_audio_submit(ptr: u32, num_samples: u32) -> u32
        // Submits interleaved i16 stereo PCM at 44100 Hz into the shared audio ring buffer.
        // num_samples is the count of i16 values (stereo frames * 2).
        // Returns 0 on success, 1 if samples were dropped due to buffer overflow.
        linker
            .define_typed(
                "host_audio_submit",
                |caller: polkavm::Caller<'_, HostState>, ptr: u32, num_samples: u32| -> Result<u32, anyhow::Error> {
                    if num_samples == 0 {
                        return Ok(0);
                    }
                    // Hard cap: 1 second per call
                    if num_samples > 44100 * 2 {
                        return Ok(1);
                    }
                    let byte_len = num_samples.checked_mul(2)
                        .ok_or_else(|| anyhow!("audio submit overflow"))?;
                    let bytes = read_guest_memory(caller.instance, ptr, byte_len)?;

                    let mut ring = caller.user_data.audio_ring.lock().unwrap();

                    // Drop oldest on overflow
                    let overflow = (ring.len() + num_samples as usize).saturating_sub(AUDIO_RING_MAX);
                    if overflow > 0 {
                        ring.drain(..overflow);
                    }

                    ring.extend(
                        bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]))
                    );
                    Ok(if overflow > 0 { 1 } else { 0 })
                },
            )
            .context("Failed to define host_audio_submit")?;

        // host_asset_read(name_ptr: u32, name_len: u32, offset: u32, dst_ptr: u32, max_len: u32) -> u32
        // Reads from HostState.assets, writes slice into guest memory, returns bytes read (0 = not found / EOF).
        linker
            .define_typed(
                "host_asset_read",
                |caller: polkavm::Caller<'_, HostState>, name_ptr: u32, name_len: u32, offset: u32, dst_ptr: u32, max_len: u32| -> Result<u32, anyhow::Error> {
                    let name_buf = read_guest_memory(caller.instance, name_ptr, name_len)?;
                    let Ok(name) = String::from_utf8(name_buf) else {
                        return Ok(0); // invalid UTF-8 name → not found
                    };
                    let Some(data) = caller.user_data.assets.get(&name) else {
                        return Ok(0);
                    };
                    let offset = offset as usize;
                    if offset >= data.len() {
                        return Ok(0);
                    }
                    let remaining = &data[offset..];
                    let to_write = remaining.len().min(max_len as usize);
                    caller
                        .instance
                        .write_memory(dst_ptr, &remaining[..to_write])
                        .map_err(|e| anyhow!("Memory write error: {:?}", e))?;
                    Ok(to_write as u32)
                },
            )
            .context("Failed to define host_asset_read")?;

        let instance_pre = linker
            .instantiate_pre(&module)
            .context("Failed to pre-instantiate module")?;

        let instance = instance_pre
            .instantiate()
            .context("Failed to instantiate module")?;

        let audio_ring: AudioRing = Arc::new(Mutex::new(VecDeque::with_capacity(AUDIO_RING_MAX)));
        let audio_stream = build_audio_stream(&audio_ring).map(SendStream);

        let state = Arc::new(Mutex::new(HostState::new(audio_ring)));

        Ok(Self {
            instance,
            state,
            max_gas_per_update: config.max_gas_per_update,
            yielded: false,
            _audio_stream: audio_stream,
        })
    }

    /// Load a .polkavm file from disk.
    pub fn from_file(path: &std::path::Path, config: &SandboxConfig) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        Self::from_bytes(&bytes, config)
    }

    /// Check whether the last PolkaVM call ended with a cooperative yield
    /// (host_yield). If so, mark `self.yielded` and return Ok. Otherwise
    /// propagate the real error.
    fn handle_call_result(
        &mut self,
        result: Result<(), CallError<anyhow::Error>>,
        phase: &str,
    ) -> Result<()> {
        match result {
            Ok(()) => {
                // Guest function returned normally.
                Ok(())
            }
            Err(CallError::User(_)) => {
                let mut state = self.state.lock().unwrap();
                if state.yield_requested {
                    state.yield_requested = false;
                    drop(state);
                    self.yielded = true;
                    Ok(())
                } else {
                    Err(anyhow!("Guest user error during {phase}"))
                }
            }
            Err(CallError::Trap) => {
                let pc = self.instance.program_counter();
                Err(anyhow!("Guest trapped during {phase} (pc={pc:?})"))
            }
            Err(CallError::NotEnoughGas) => {
                Err(anyhow!("Guest ran out of gas during {phase}"))
            }
            Err(CallError::Error(e)) => Err(e.into()),
            Err(e) => Err(anyhow!("Unexpected call error during {phase}: {e:?}")),
        }
    }

    /// Call the guest's `init` function.
    /// Uses 1000x the per-update gas budget (init may load assets, build tables, etc.).
    /// If the guest calls `host_yield`, init returns successfully and subsequent
    /// `call_update` calls will resume execution from the yield point.
    pub fn call_init(&mut self) -> Result<()> {
        let init_gas = self.max_gas_per_update.saturating_mul(1000).min(i64::MAX as u64) as i64;
        self.instance.set_gas(init_gas);
        let mut state = self.state.lock().unwrap();
        state.yield_requested = false;
        let result = self.instance
            .call_typed_and_get_result::<(), ()>(&mut *state, "init", ());
        drop(state);
        self.handle_call_result(result, "init")
    }

    /// Call the guest's `update` function (main loop tick).
    /// If the guest previously yielded via `host_yield`, this resumes execution
    /// from the yield point instead of calling the "update" entry point.
    pub fn call_update(&mut self) -> Result<()> {
        let gas = if self.max_gas_per_update > i64::MAX as u64 {
            i64::MAX
        } else {
            self.max_gas_per_update as i64
        };
        self.instance.set_gas(gas);
        let mut state = self.state.lock().unwrap();
        state.yield_requested = false;
        let result = if self.yielded {
            self.yielded = false;
            self.instance.continue_execution(&mut *state).map(|_| ())
        } else {
            self.instance
                .call_typed_and_get_result::<(), ()>(&mut *state, "update", ())
        };
        drop(state);
        self.handle_call_result(result, "update")
    }

    /// Send an event to the guest (queued for next poll_event call).
    pub fn send_event(&self, event: GuestEvent) {
        let mut state = self.state.lock().unwrap();
        state.event_queue.push_back(event);
    }

    /// Take the latest view tree emitted by the guest.
    pub fn take_view_tree(&self) -> Option<ViewTree> {
        let mut state = self.state.lock().unwrap();
        state.view_tree.take()
    }

    /// Take pending fetch requests.
    pub fn take_pending_fetches(&self) -> Vec<(String, u64)> {
        let mut state = self.state.lock().unwrap();
        std::mem::take(&mut state.pending_fetches)
    }

    /// Take the latest framebuffer submitted by `host_present_frame`.
    /// Returns `(argb_pixels, width, height)`.
    pub fn take_framebuffer(&self) -> Option<(Vec<u8>, u32, u32)> {
        let mut state = self.state.lock().unwrap();
        state.framebuffer.take()
    }

    /// Send an input event to the guest (queued for next `host_poll_input` call).
    pub fn send_input(&self, event: InputEvent) {
        let mut state = self.state.lock().unwrap();
        state.input_queue.push_back(event);
    }

    /// Load assets into the sandbox (typically from a .prod bundle).
    pub fn load_assets(&self, assets: HashMap<String, Vec<u8>>) {
        let mut state = self.state.lock().unwrap();
        state.assets = assets;
    }

    /// Returns true if the audio ring buffer has samples in it (guest is producing audio).
    pub fn audio_active(&self) -> bool {
        let state = self.state.lock().unwrap();
        let ring = state.audio_ring.lock().unwrap();
        !ring.is_empty()
    }
}
