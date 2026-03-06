use anyhow::{anyhow, Context, Result};
use polkavm::{CallError, Config, Engine, Instance, Linker, Module, ProgramBlob};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use epoca_protocol::{
    deserialize_view_tree, serialize_event, GuestEvent, ViewTree,
};

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
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            view_tree: None,
            event_queue: VecDeque::new(),
            pending_fetches: Vec::new(),
            framebuffer: None,
            input_queue: VecDeque::new(),
            assets: HashMap::new(),
            time_origin: std::time::Instant::now(),
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

/// A running PolkaVM sandbox instance.
pub struct SandboxInstance {
    instance: Instance<HostState, anyhow::Error>,
    state: Arc<Mutex<HostState>>,
    /// Gas limit applied before each `call_update()` tick.
    max_gas_per_update: u64,
}

impl SandboxInstance {
    /// Create a new sandbox from a .polkavm program blob.
    pub fn from_bytes(blob_bytes: &[u8], config: &SandboxConfig) -> Result<Self> {
        let blob = ProgramBlob::parse(blob_bytes.into())
            .context("Failed to parse PolkaVM program blob")?;

        let engine_config = Config::from_env().unwrap_or_default();
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

        let state = Arc::new(Mutex::new(HostState::default()));

        Ok(Self { instance, state, max_gas_per_update: config.max_gas_per_update })
    }

    /// Load a .polkavm file from disk.
    pub fn from_file(path: &std::path::Path, config: &SandboxConfig) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        Self::from_bytes(&bytes, config)
    }

    /// Call the guest's `init` function.
    /// Uses 10x the per-update gas budget (init may load assets, build tables, etc.).
    pub fn call_init(&mut self) -> Result<()> {
        let init_gas = self.max_gas_per_update.saturating_mul(10).min(i64::MAX as u64) as i64;
        self.instance.set_gas(init_gas);
        let mut state = self.state.lock().unwrap();
        self.instance
            .call_typed_and_get_result::<(), ()>(&mut *state, "init", ())
            .map_err(|e| match e {
                CallError::Trap => {
                    let pc = self.instance.program_counter();
                    anyhow!("Guest trapped during init (pc={:?})", pc)
                },
                CallError::NotEnoughGas => anyhow!("Guest ran out of gas during init"),
                CallError::Error(e) => e.into(),
                CallError::User(e) => e,
                _ => anyhow!("Unexpected call error during init"),
            })?;
        Ok(())
    }

    /// Call the guest's `update` function (main loop tick).
    /// Returns `Err` if the guest traps, exceeds its gas budget, or errors.
    /// A `NotEnoughGas` error should be shown to the user as "app timed out"
    /// rather than killing the browser — the guest can be restarted.
    pub fn call_update(&mut self) -> Result<()> {
        // Re-fill gas before each tick so a slow tick doesn't accumulate debt.
        let gas = if self.max_gas_per_update > i64::MAX as u64 {
            i64::MAX
        } else {
            self.max_gas_per_update as i64
        };
        self.instance.set_gas(gas);
        let mut state = self.state.lock().unwrap();
        self.instance
            .call_typed_and_get_result::<(), ()>(&mut *state, "update", ())
            .map_err(|e| match e {
                CallError::Trap => {
                    let pc = self.instance.program_counter();
                    anyhow!("Guest trapped during update (pc={:?})", pc)
                },
                CallError::NotEnoughGas => anyhow!("Guest exceeded gas limit during update — possible infinite loop"),
                CallError::Error(e) => e.into(),
                CallError::User(e) => e,
                _ => anyhow!("Unexpected call error during update"),
            })?;
        Ok(())
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
}
