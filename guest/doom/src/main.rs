#![no_std]
#![no_main]

extern crate alloc;

#[global_allocator]
static ALLOCATOR: polkavm_derive::LeakingAllocator = polkavm_derive::LeakingAllocator;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Try to log the panic message via host_log
    let mut buf = [0u8; 256];
    let msg = b"PANIC: doom guest panicked";
    let len = msg.len().min(buf.len());
    buf[..len].copy_from_slice(&msg[..len]);
    unsafe { host_log(buf.as_ptr() as u32, len as u32) };
    let _ = info;
    unsafe { core::arch::asm!("unimp", options(noreturn)) }
}

// ── Host function imports ──────────────────────────────────────────

#[polkavm_derive::polkavm_import]
extern "C" {
    fn host_present_frame(ptr: u32, width: u32, height: u32, stride: u32) -> u32;
    fn host_poll_input(buf_ptr: u32, buf_len: u32) -> u32;
    fn host_time_ms() -> u64;
    fn host_asset_read(name_ptr: u32, name_len: u32, offset: u32, dst_ptr: u32, max_len: u32) -> u32;
    fn host_log(ptr: u32, len: u32);
}

// ── C function imports from our compiled doom library ──────────────

extern "C" {
    fn doomgeneric_Create(argc: i32, argv: *const *const u8);
    fn doomgeneric_Tick();
}

// ── Exported entry points for PolkaVM ──────────────────────────────

/// Fake argv for doomgeneric: ["doom", "-iwad", "doom1.wad"]
static ARG0: &[u8] = b"doom\0";
static ARG1: &[u8] = b"-iwad\0";
static ARG2: &[u8] = b"doom1.wad\0";

#[polkavm_derive::polkavm_export]
extern "C" fn init() {
    let argv: [*const u8; 3] = [
        ARG0.as_ptr(),
        ARG1.as_ptr(),
        ARG2.as_ptr(),
    ];
    unsafe {
        doomgeneric_Create(3, argv.as_ptr());
    }
}

#[polkavm_derive::polkavm_export]
extern "C" fn update() {
    unsafe {
        doomgeneric_Tick();
    }
}
