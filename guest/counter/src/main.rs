//! Counter guest app — a minimal PolkaVM guest demonstrating the view protocol.
//!
//! Build:
//!   TARGET=$(polkatool get-target-json-path --bitness 32)
//!   cargo +nightly build -Z build-std=core,alloc --target "$TARGET" --release
//!   polkatool link -s target/riscv32emac-unknown-none-polkavm/release/counter-guest -o counter.polkavm

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use zhost_guest_ui::*;
use zhost_protocol::*;

#[global_allocator]
static ALLOCATOR: polkavm_derive::LeakingAllocator = polkavm_derive::LeakingAllocator;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("unimp", options(noreturn)) }
}

// Host function imports
#[polkavm_derive::polkavm_import]
extern "C" {
    fn host_set_view(ptr: u32, len: u32) -> u32;
    fn host_poll_event(buf_ptr: u32, buf_len: u32) -> u32;
    fn host_log(ptr: u32, len: u32);
}

static mut COUNTER: i64 = 0;
const INCR: CallbackId = 100;
const DECR: CallbackId = 101;

#[polkavm_derive::polkavm_export]
extern "C" fn init() {
    log_msg("counter: init");
    emit_view();
}

#[polkavm_derive::polkavm_export]
extern "C" fn update() {
    let mut buf = [0u8; 256];
    let len = unsafe { host_poll_event(buf.as_mut_ptr() as u32, buf.len() as u32) };

    if len > 0 {
        if let Ok(event) = deserialize_event(&buf[..len as usize]) {
            match event.callback_id {
                INCR => unsafe { COUNTER += 1 },
                DECR => unsafe { COUNTER -= 1 },
                _ => return,
            }
            emit_view();
        }
    }
}

fn emit_view() {
    let count = unsafe { COUNTER };
    reset_ids();

    let tree = vstack(12, alloc::vec![
        text(&format!("Count: {count}")).heading(),
        hstack(8, alloc::vec![
            button("+").primary().on_click_id(INCR),
            button("-").on_click_id(DECR),
        ]),
    ])
    .into_tree();

    if let Ok(bytes) = serialize_view_tree(&tree) {
        unsafe {
            host_set_view(bytes.as_ptr() as u32, bytes.len() as u32);
        }
    }
}

fn log_msg(msg: &str) {
    unsafe {
        host_log(msg.as_ptr() as u32, msg.len() as u32);
    }
}
