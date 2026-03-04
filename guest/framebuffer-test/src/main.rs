#![no_std]
#![no_main]

extern crate alloc;

#[global_allocator]
static ALLOCATOR: polkavm_derive::LeakingAllocator = polkavm_derive::LeakingAllocator;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("unimp", options(noreturn)) }
}

#[polkavm_derive::polkavm_import]
extern "C" {
    fn host_present_frame(ptr: u32, width: u32, height: u32, stride: u32) -> u32;
    fn host_time_ms() -> u64;
    fn host_poll_input(buf_ptr: u32, buf_len: u32) -> u32;
    fn host_log(ptr: u32, len: u32);
}

const W: u32 = 320;
const H: u32 = 200;
const PIXELS: usize = (W * H) as usize;

// Static framebuffer — ARGB packed as u32
static mut FB: [u32; PIXELS] = [0u32; PIXELS];
static mut RECT_X: i32 = 0;
static mut DX: i32 = 2;
static mut RECT_Y: i32 = 80;
static mut DY: i32 = 1;

const RECT_W: i32 = 48;
const RECT_H: i32 = 48;
const BG_COLOR: u32 = 0xFF0F0F23; // deep navy (ARGB)
const FG_COLOR: u32 = 0xFF00D4AA; // teal/mint (ARGB)

fn log_msg(msg: &[u8]) {
    unsafe {
        host_log(msg.as_ptr() as u32, msg.len() as u32);
    }
}

#[polkavm_derive::polkavm_export]
extern "C" fn init() {
    log_msg(b"framebuffer-test: init");
}

#[polkavm_derive::polkavm_export]
extern "C" fn update() {
    unsafe {
        // Clear to background color
        for pixel in FB.iter_mut() {
            *pixel = BG_COLOR;
        }

        // Update position — bounce
        RECT_X += DX;
        RECT_Y += DY;

        if RECT_X <= 0 || RECT_X >= (W as i32 - RECT_W) {
            DX = -DX;
            RECT_X += DX;
        }
        if RECT_Y <= 0 || RECT_Y >= (H as i32 - RECT_H) {
            DY = -DY;
            RECT_Y += DY;
        }

        // Draw rectangle
        let x0 = RECT_X.max(0) as u32;
        let y0 = RECT_Y.max(0) as u32;
        let x1 = ((RECT_X + RECT_W) as u32).min(W);
        let y1 = ((RECT_Y + RECT_H) as u32).min(H);

        for y in y0..y1 {
            for x in x0..x1 {
                FB[(y * W + x) as usize] = FG_COLOR;
            }
        }

        // Present the frame
        host_present_frame(
            FB.as_ptr() as u32,
            W,
            H,
            W * 4, // stride in bytes
        );
    }
}
