#![no_std]
#![no_main]
#![allow(static_mut_refs)]
#![allow(dead_code)]

extern crate alloc;

// Provide memset/memcpy/memmove for no_std (used by array copies)
#[no_mangle]
pub unsafe extern "C" fn memset(dest: *mut u8, val: i32, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        *dest.add(i) = val as u8;
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        *dest.add(i) = *src.add(i);
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dest as usize) < (src as usize) {
        let mut i = 0;
        while i < n {
            *dest.add(i) = *src.add(i);
            i += 1;
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            *dest.add(i) = *src.add(i);
        }
    }
    dest
}

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

// ── Constants ────────────────────────────────────────────────────────
const W: u32 = 320;
const H: u32 = 200;
const PIXELS: usize = (W * H) as usize;

const SKY_TOP: u32 = 0;
const SKY_BOTTOM: u32 = 40;
const SURFACE_TOP: u32 = 40;
const SURFACE_BOTTOM: u32 = 60;
const WATER_TOP: u32 = 60;
const WATER_BOTTOM: u32 = 200;

// Scancodes (from keystroke_to_code)
const KEY_SPACE: u8 = 0x39;
const KEY_ESCAPE: u8 = 0x01;
const KEY_1: u8 = b'1';
const KEY_2: u8 = b'2';
const KEY_3: u8 = b'3';
const KEY_4: u8 = b'4';

// Colors (ARGB)
const COL_SKY_TOP: u32 = 0xFF050510;
const COL_SKY_BOT: u32 = 0xFF0A0A2A;
const COL_WATER_TOP: u32 = 0xFF0A1228;
const COL_WATER_BOT: u32 = 0xFF020208;
const COL_SURFACE: u32 = 0xFF102040;
const COL_MOON: u32 = 0xFFE8E0C8;
const COL_DOCK: u32 = 0xFF5A3A1A;
const COL_DOCK_DARK: u32 = 0xFF3A2510;
const COL_PLAYER_SKIN: u32 = 0xFFD4A574;
const COL_PLAYER_SHIRT: u32 = 0xFF2244AA;
const COL_PLAYER_PANTS: u32 = 0xFF1A1A3A;
const COL_PLAYER_HAT: u32 = 0xFF883322;
const COL_LINE: u32 = 0xFFAABBCC;
const COL_LINE_TENSE: u32 = 0xFFFF4444;
const COL_WHITE: u32 = 0xFFFFFFFF;
const COL_BLACK: u32 = 0xFF000000;
const COL_GRAY: u32 = 0xFF888888;
const COL_DARK_GRAY: u32 = 0xFF444444;
const COL_RED: u32 = 0xFFFF2222;
const COL_GREEN_UI: u32 = 0xFF22DD44;
const COL_YELLOW: u32 = 0xFFFFDD22;

// Lure / fish glow colors
const COL_CYAN: u32 = 0xFF00DDFF;
const COL_ORANGE: u32 = 0xFFFF8800;
const COL_GREEN: u32 = 0xFF00FF66;
const COL_PURPLE: u32 = 0xFFAA44FF;
const COL_GOLD: u32 = 0xFFFFDD00;
const COL_GHOST_WHITE: u32 = 0xFFCCDDFF;

// Fish species indices
const FISH_MOONMINNOW: u8 = 0;
const FISH_EMBEREEL: u8 = 1;
const FISH_GHOSTFIN: u8 = 2;
const FISH_VENOMJAW: u8 = 3;
const FISH_ABYSSAL: u8 = 4;
const FISH_SHIMMER: u8 = 5;
const NUM_SPECIES: usize = 6;

// Lure indices
const LURE_CYAN: u8 = 0;
const LURE_ORANGE: u8 = 1;
const LURE_GREEN: u8 = 2;
const LURE_PURPLE: u8 = 3;

const MAX_FISH: usize = 12;
const MAX_PARTICLES: usize = 40;
const MAX_STARS: usize = 60;

// ── Game States ──────────────────────────────────────────────────────
const STATE_IDLE: u8 = 0;
const STATE_CASTING: u8 = 1;
const STATE_WAITING: u8 = 2;
const STATE_BITING: u8 = 3;
const STATE_REELING: u8 = 4;
const STATE_CAUGHT: u8 = 5;
const STATE_LOST: u8 = 6;
const STATE_WIN: u8 = 7;

// ── Fish Definition (compile-time) ──────────────────────────────────
#[derive(Clone, Copy)]
struct FishDef {
    glow_color: u32,
    body_color: u32,
    depth_min: u32,
    depth_max: u32,
    speed: i32,           // base movement speed (256 = 1 pixel/frame)
    bite_window_ms: u32,
    green_zone: u8,       // percentage of reel bar that is "safe"
    pull_freq: u16,       // frames between pulls
    pull_strength: u8,    // tension added per pull (percentage points)
    rarity_weight: u8,
    attracted_by: u8,     // lure index, 0xFF = any (ghostfin), 0xFE = ignore (shimmer)
    weight_min: u16,      // in tenths of oz
    weight_max: u16,
    body_len: u8,         // sprite length in pixels
    body_height: u8,      // sprite height
}

const FISH_DEFS: [FishDef; NUM_SPECIES] = [
    // Moonminnow — cyan, easy, common
    FishDef {
        glow_color: COL_CYAN, body_color: 0xFF0088AA,
        depth_min: 65, depth_max: 120, speed: 180,
        bite_window_ms: 1500, green_zone: 40, pull_freq: 90, pull_strength: 5,
        rarity_weight: 40, attracted_by: LURE_CYAN,
        weight_min: 20, weight_max: 80, body_len: 8, body_height: 4,
    },
    // Embereel — orange, medium
    FishDef {
        glow_color: COL_ORANGE, body_color: 0xFFAA4400,
        depth_min: 80, depth_max: 150, speed: 200,
        bite_window_ms: 1200, green_zone: 35, pull_freq: 60, pull_strength: 10,
        rarity_weight: 25, attracted_by: LURE_ORANGE,
        weight_min: 40, weight_max: 160, body_len: 12, body_height: 3,
    },
    // Ghostfin — white, wildcard
    FishDef {
        glow_color: COL_GHOST_WHITE, body_color: 0xFF8899BB,
        depth_min: 70, depth_max: 140, speed: 100,
        bite_window_ms: 1200, green_zone: 30, pull_freq: 70, pull_strength: 8,
        rarity_weight: 20, attracted_by: 0xFF, // any lure
        weight_min: 30, weight_max: 120, body_len: 10, body_height: 5,
    },
    // Venomjaw — green, hard
    FishDef {
        glow_color: COL_GREEN, body_color: 0xFF006622,
        depth_min: 100, depth_max: 180, speed: 280,
        bite_window_ms: 1000, green_zone: 25, pull_freq: 40, pull_strength: 18,
        rarity_weight: 10, attracted_by: LURE_GREEN,
        weight_min: 80, weight_max: 300, body_len: 11, body_height: 5,
    },
    // Abyssal Lantern — purple, very hard
    FishDef {
        glow_color: COL_PURPLE, body_color: 0xFF552288,
        depth_min: 140, depth_max: 195, speed: 120,
        bite_window_ms: 800, green_zone: 20, pull_freq: 30, pull_strength: 22,
        rarity_weight: 4, attracted_by: LURE_PURPLE,
        weight_min: 120, weight_max: 500, body_len: 10, body_height: 7,
    },
    // Shimmer Ray — gold, legendary
    FishDef {
        glow_color: COL_GOLD, body_color: 0xFFBB9900,
        depth_min: 80, depth_max: 190, speed: 160,
        bite_window_ms: 600, green_zone: 15, pull_freq: 35, pull_strength: 15,
        rarity_weight: 1, attracted_by: 0xFE, // ignores lure
        weight_min: 200, weight_max: 800, body_len: 14, body_height: 6,
    },
];

// Species names as byte arrays for bitmap font
const FISH_NAMES: [&[u8]; NUM_SPECIES] = [
    b"MOONMINNOW",
    b"EMBEREEL",
    b"GHOSTFIN",
    b"VENOMJAW",
    b"ABYSSAL LANTERN",
    b"SHIMMER RAY",
];

// ── Bitmap Font (3x5, 15 bits per glyph) ────────────────────────────
// Each u16 encodes a 3×5 glyph: bit 14 = top-left, bit 0 = bottom-right
// Row-major: bits 14-12 = row0, 11-9 = row1, 8-6 = row2, 5-3 = row3, 2-0 = row4
const FONT_GLYPHS: [u16; 40] = [
    // A-Z
    0b_010_111_101_111_101, // A
    0b_110_101_110_101_110, // B
    0b_011_100_100_100_011, // C
    0b_110_101_101_101_110, // D
    0b_111_100_110_100_111, // E
    0b_111_100_110_100_100, // F
    0b_011_100_101_101_011, // G
    0b_101_101_111_101_101, // H
    0b_111_010_010_010_111, // I
    0b_001_001_001_101_010, // J
    0b_101_110_100_110_101, // K
    0b_100_100_100_100_111, // L
    0b_101_111_111_101_101, // M
    0b_101_111_111_111_101, // N
    0b_010_101_101_101_010, // O
    0b_110_101_110_100_100, // P
    0b_010_101_101_110_011, // Q
    0b_110_101_110_101_101, // R
    0b_011_100_010_001_110, // S
    0b_111_010_010_010_010, // T
    0b_101_101_101_101_010, // U
    0b_101_101_101_010_010, // V – same visual
    0b_101_101_111_111_101, // W
    0b_101_101_010_101_101, // X
    0b_101_101_010_010_010, // Y
    0b_111_001_010_100_111, // Z
    // 0-9
    0b_010_101_101_101_010, // 0
    0b_010_110_010_010_111, // 1
    0b_110_001_010_100_111, // 2
    0b_110_001_010_001_110, // 3
    0b_101_101_111_001_001, // 4
    0b_111_100_110_001_110, // 5
    0b_011_100_111_101_010, // 6 — slightly adjusted
    0b_111_001_010_010_010, // 7
    0b_010_101_010_101_010, // 8
    0b_010_101_011_001_110, // 9
    // Special: space, !, :, .
    0b_000_000_000_000_000, // space (index 36)
    0b_010_010_010_000_010, // !     (index 37)
    0b_000_010_000_010_000, // :     (index 38)
    0b_000_000_000_000_010, // .     (index 39)
];

fn glyph_index(ch: u8) -> Option<usize> {
    match ch {
        b'A'..=b'Z' => Some((ch - b'A') as usize),
        b'a'..=b'z' => Some((ch - b'a') as usize),
        b'0'..=b'9' => Some((ch - b'0') as usize + 26),
        b' ' => Some(36),
        b'!' => Some(37),
        b':' => Some(38),
        b'.' => Some(39),
        _ => None,
    }
}

// ── Data Structures ──────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Fish {
    active: bool,
    species: u8,
    x: i32,           // fixed-point 8.8
    y: i32,
    dir: i8,           // -1 or 1
    anim_frame: u16,
    state: u8,         // 0=swimming, 1=attracted, 2=circling, 3=inspecting, 4=fleeing
    circle_angle: u16, // for circling behavior
    flee_timer: u16,
}

impl Fish {
    const fn empty() -> Self {
        Fish {
            active: false, species: 0, x: 0, y: 0, dir: 1,
            anim_frame: 0, state: 0, circle_angle: 0, flee_timer: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Particle {
    active: bool,
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    color: u32,
    life: u16,
    max_life: u16,
}

impl Particle {
    const fn empty() -> Self {
        Particle {
            active: false, x: 0, y: 0, vx: 0, vy: 0,
            color: 0, life: 0, max_life: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Star {
    x: u16,
    y: u16,
    brightness: u8,
    twinkle_phase: u16,
}

struct Game {
    // Core
    state: u8,
    tick: u32,
    rng_state: u32,
    last_time_ms: u64,
    keys: [bool; 256],
    keys_prev: [bool; 256],

    // Lure selection
    current_lure: u8,
    lures_unlocked: [bool; 4],

    // Casting
    cast_power: u16,       // 0..255
    cast_charging: bool,
    lure_x: i32,           // where lure landed (pixel coords)
    lure_y: i32,           // depth

    // Bobber
    bobber_y_base: i32,
    bobber_phase: u16,
    twitch_count: u8,
    twitch_timer: u16,
    next_twitch_at: u16,

    // Biting
    bite_timer: u16,       // countdown frames in BITING state
    bite_fish_idx: u8,     // which fish is biting

    // Reeling
    tension: u16,          // 0..255
    reel_progress: u16,    // 0..255 (100% = caught)
    fish_reel_x: i32,      // visual position of hooked fish
    fish_reel_y: i32,
    fish_pull_timer: u16,
    fish_pull_dir: i8,

    // Caught display
    caught_species: u8,
    caught_weight: u16,    // tenths of oz
    caught_timer: u16,
    show_silhouettes: bool,

    // Lost display
    lost_timer: u16,

    // Screen shake
    shake_timer: u8,
    shake_offset_x: i8,
    shake_offset_y: i8,

    // Collection
    fish_caught_count: [u16; NUM_SPECIES],
    best_weight: [u16; NUM_SPECIES],
    total_caught: u16,

    // Entities
    fish: [Fish; MAX_FISH],
    particles: [Particle; MAX_PARTICLES],
    stars: [Star; MAX_STARS],

    // Splash animation
    splash_x: i32,
    splash_timer: u8,

    // Wait timer
    wait_timer: u16,

    // Cast line endpoint for animation
    cast_anim_timer: u8,

    // Win state
    win_timer: u16,
}

// ── Static Game State ────────────────────────────────────────────────
static mut FB: [u32; PIXELS] = [0u32; PIXELS];
static mut GAME: Game = Game {
    state: STATE_IDLE,
    tick: 0,
    rng_state: 0xDEAD_BEEF,
    last_time_ms: 0,
    keys: [false; 256],
    keys_prev: [false; 256],
    current_lure: LURE_CYAN,
    lures_unlocked: [true, false, false, false],
    cast_power: 0,
    cast_charging: false,
    lure_x: 0,
    lure_y: 0,
    bobber_y_base: 0,
    bobber_phase: 0,
    twitch_count: 0,
    twitch_timer: 0,
    next_twitch_at: 0,
    bite_timer: 0,
    bite_fish_idx: 0,
    tension: 0,
    reel_progress: 0,
    fish_reel_x: 0,
    fish_reel_y: 0,
    fish_pull_timer: 0,
    fish_pull_dir: 1,
    caught_species: 0,
    caught_weight: 0,
    caught_timer: 0,
    show_silhouettes: false,
    lost_timer: 0,
    shake_timer: 0,
    shake_offset_x: 0,
    shake_offset_y: 0,
    fish_caught_count: [0; NUM_SPECIES],
    best_weight: [0; NUM_SPECIES],
    total_caught: 0,
    fish: [Fish::empty(); MAX_FISH],
    particles: [Particle::empty(); MAX_PARTICLES],
    stars: [Star { x: 0, y: 0, brightness: 0, twinkle_phase: 0 }; MAX_STARS],
    splash_x: 0,
    splash_timer: 0,
    wait_timer: 0,
    cast_anim_timer: 0,
    win_timer: 0,
};

// ── Utility Functions ────────────────────────────────────────────────
fn log_msg(msg: &[u8]) {
    unsafe { host_log(msg.as_ptr() as u32, msg.len() as u32); }
}

fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Integer sine approximation: phase 0..1023 maps to -127..127
/// Uses triangle wave for simplicity
fn isin(phase: u32) -> i32 {
    let p = (phase & 1023) as i32;
    if p < 256 {
        (p * 127) / 256
    } else if p < 512 {
        ((512 - p) * 127) / 256
    } else if p < 768 {
        -((p - 512) * 127) / 256
    } else {
        -((1024 - p) * 127) / 256
    }
}

fn icos(phase: u32) -> i32 {
    isin(phase + 256)
}

fn isqrt(n: u32) -> u32 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn abs_i32(x: i32) -> i32 {
    if x < 0 { -x } else { x }
}

fn lerp_u8(a: u8, b: u8, t: u8) -> u8 {
    let a = a as u32;
    let b = b as u32;
    let t = t as u32;
    ((a * (255 - t) + b * t) / 255) as u8
}

fn lerp_color(a: u32, b: u32, t: u8) -> u32 {
    let ra = ((a >> 16) & 0xFF) as u8;
    let ga = ((a >> 8) & 0xFF) as u8;
    let ba = (a & 0xFF) as u8;
    let rb = ((b >> 16) & 0xFF) as u8;
    let gb = ((b >> 8) & 0xFF) as u8;
    let bb = (b & 0xFF) as u8;
    let r = lerp_u8(ra, rb, t);
    let g = lerp_u8(ga, gb, t);
    let bl = lerp_u8(ba, bb, t);
    0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (bl as u32)
}

fn color_alpha(base: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let r = (((base >> 16) & 0xFF) * a / 255) as u32;
    let g = (((base >> 8) & 0xFF) * a / 255) as u32;
    let b = ((base & 0xFF) * a / 255) as u32;
    0xFF000000 | (r << 16) | (g << 8) | b
}

fn lure_color(lure: u8) -> u32 {
    match lure {
        LURE_CYAN => COL_CYAN,
        LURE_ORANGE => COL_ORANGE,
        LURE_GREEN => COL_GREEN,
        LURE_PURPLE => COL_PURPLE,
        _ => COL_WHITE,
    }
}

// ── Drawing Primitives ───────────────────────────────────────────────
fn put_pixel(x: i32, y: i32, color: u32) {
    if x >= 0 && x < W as i32 && y >= 0 && y < H as i32 {
        unsafe { FB[(y as u32 * W + x as u32) as usize] = color; }
    }
}

fn put_pixel_alpha(x: i32, y: i32, fg: u32, alpha: u8) {
    if x < 0 || x >= W as i32 || y < 0 || y >= H as i32 { return; }
    let idx = (y as u32 * W + x as u32) as usize;
    unsafe {
        let bg = FB[idx];
        FB[idx] = lerp_color(bg, fg, alpha);
    }
}

fn fill_rect(x0: i32, y0: i32, w: i32, h: i32, color: u32) {
    let x_start = x0.max(0);
    let y_start = y0.max(0);
    let x_end = (x0 + w).min(W as i32);
    let y_end = (y0 + h).min(H as i32);
    for y in y_start..y_end {
        for x in x_start..x_end {
            unsafe { FB[(y as u32 * W + x as u32) as usize] = color; }
        }
    }
}

fn hline(x0: i32, x1: i32, y: i32, color: u32) {
    if y < 0 || y >= H as i32 { return; }
    let xs = x0.max(0);
    let xe = x1.min(W as i32 - 1);
    for x in xs..=xe {
        unsafe { FB[(y as u32 * W + x as u32) as usize] = color; }
    }
}

fn vline(x: i32, y0: i32, y1: i32, color: u32) {
    if x < 0 || x >= W as i32 { return; }
    let ys = y0.max(0);
    let ye = y1.min(H as i32 - 1);
    for y in ys..=ye {
        unsafe { FB[(y as u32 * W + x as u32) as usize] = color; }
    }
}

fn draw_circle_filled(cx: i32, cy: i32, r: i32, color: u32) {
    for dy in -r..=r {
        let dx_max = isqrt((r * r - dy * dy) as u32) as i32;
        hline(cx - dx_max, cx + dx_max, cy + dy, color);
    }
}

fn draw_char(x: i32, y: i32, ch: u8, color: u32) {
    if let Some(idx) = glyph_index(ch) {
        let bits = FONT_GLYPHS[idx];
        for row in 0..5 {
            for col in 0..3 {
                let bit = 14 - (row * 3 + col);
                if (bits >> bit) & 1 != 0 {
                    put_pixel(x + col as i32, y + row as i32, color);
                }
            }
        }
    }
}

fn draw_text(x: i32, y: i32, text: &[u8], color: u32) {
    let mut cx = x;
    for &ch in text {
        draw_char(cx, y, ch, color);
        cx += 4;
    }
}

fn draw_text_centered(y: i32, text: &[u8], color: u32) {
    let w = text.len() as i32 * 4 - 1;
    draw_text((W as i32 - w) / 2, y, text, color);
}

fn text_width(text: &[u8]) -> i32 {
    if text.is_empty() { 0 } else { text.len() as i32 * 4 - 1 }
}

// ── Number to text helper ────────────────────────────────────────────
fn draw_number(x: i32, y: i32, mut val: u32, color: u32) {
    if val == 0 {
        draw_char(x, y, b'0', color);
        return;
    }
    let mut digits = [0u8; 10];
    let mut count = 0;
    while val > 0 {
        digits[count] = (val % 10) as u8 + b'0';
        val /= 10;
        count += 1;
    }
    let mut cx = x;
    for i in (0..count).rev() {
        draw_char(cx, y, digits[i], color);
        cx += 4;
    }
}

fn draw_weight(x: i32, y: i32, weight_tenths: u16, color: u32) {
    let whole = weight_tenths / 10;
    let frac = weight_tenths % 10;
    let mut cx = x;
    // Draw whole part
    if whole == 0 {
        draw_char(cx, y, b'0', color);
        cx += 4;
    } else {
        let mut digits = [0u8; 5];
        let mut count = 0;
        let mut v = whole;
        while v > 0 {
            digits[count] = (v % 10) as u8 + b'0';
            v /= 10;
            count += 1;
        }
        for i in (0..count).rev() {
            draw_char(cx, y, digits[i], color);
            cx += 4;
        }
    }
    draw_char(cx, y, b'.', color);
    cx += 4;
    draw_char(cx, y, (frac as u8) + b'0', color);
    cx += 4;
    draw_text(cx + 4, y, b"OZ", color);
}

// ── Environment Rendering ────────────────────────────────────────────
fn render_sky(tick: u32) {
    // Gradient sky
    for y in SKY_TOP..SKY_BOTTOM {
        let t = ((y - SKY_TOP) * 255 / (SKY_BOTTOM - SKY_TOP)) as u8;
        let color = lerp_color(COL_SKY_TOP, COL_SKY_BOT, t);
        for x in 0..W {
            unsafe { FB[(y * W + x) as usize] = color; }
        }
    }

    // Stars
    unsafe {
        for s in &GAME.stars {
            if s.x < W as u16 && s.y < SKY_BOTTOM as u16 {
                let twinkle = isin((tick as u32 * 3 + s.twinkle_phase as u32) % 1024);
                let b = (s.brightness as i32 + twinkle / 2).max(0).min(255) as u8;
                let col = 0xFF000000 | ((b as u32) << 16) | ((b as u32) << 8) | (b as u32);
                put_pixel(s.x as i32, s.y as i32, col);
            }
        }
    }

    // Moon (top-right area)
    let moon_x: i32 = 260;
    let moon_y: i32 = 15;
    let moon_r: i32 = 8;

    // Glow ring
    for dy in -(moon_r + 4)..=(moon_r + 4) {
        for dx in -(moon_r + 4)..=(moon_r + 4) {
            let dist_sq = dx * dx + dy * dy;
            let outer = (moon_r + 4) * (moon_r + 4);
            let inner = moon_r * moon_r;
            if dist_sq > inner && dist_sq < outer {
                let alpha = (255 - (dist_sq - inner) * 255 / (outer - inner)).max(0).min(255) as u8;
                put_pixel_alpha(moon_x + dx, moon_y + dy, 0xFFDDCCAA, alpha / 3);
            }
        }
    }

    // Moon body
    draw_circle_filled(moon_x, moon_y, moon_r, COL_MOON);
    // Crater hints
    put_pixel(moon_x - 2, moon_y - 1, 0xFFD0D0B0);
    put_pixel(moon_x + 3, moon_y + 2, 0xFFD0D0B0);
    put_pixel(moon_x + 1, moon_y - 3, 0xFFD8D0B8);
}

fn render_water_surface(tick: u32) {
    for y in SURFACE_TOP..SURFACE_BOTTOM {
        let depth_t = ((y - SURFACE_TOP) * 255 / (SURFACE_BOTTOM - SURFACE_TOP)) as u8;
        let base = lerp_color(COL_SKY_BOT, COL_WATER_TOP, depth_t);
        for x in 0..W {
            // Wave displacement
            let wave = isin((x as u32 * 12 + tick as u32 * 8) % 1024);
            let bright = ((wave as i32 + 127) * 20 / 255) as u8;
            let col = lerp_color(base, 0xFF1A2A50, bright);
            unsafe { FB[(y * W + x) as usize] = col; }
        }
    }

    // Moon reflection column (shimmer)
    let moon_x: i32 = 260;
    for y in SURFACE_TOP..SURFACE_BOTTOM {
        let shimmer = isin((y as u32 * 40 + tick as u32 * 12) % 1024);
        let offset = shimmer / 32;
        let px = moon_x + offset;
        if px >= 0 && px < W as i32 {
            let alpha = (80 - ((y - SURFACE_TOP) as i32 * 2)).max(0).min(80) as u8;
            put_pixel_alpha(px, y as i32, COL_MOON, alpha);
            put_pixel_alpha(px - 1, y as i32, COL_MOON, alpha / 3);
            put_pixel_alpha(px + 1, y as i32, COL_MOON, alpha / 3);
        }
    }
}

fn render_underwater() {
    for y in WATER_TOP..WATER_BOTTOM {
        let t = ((y - WATER_TOP) * 255 / (WATER_BOTTOM - WATER_TOP)) as u8;
        let color = lerp_color(COL_WATER_TOP, COL_WATER_BOT, t);
        for x in 0..W {
            unsafe { FB[(y * W + x) as usize] = color; }
        }
    }

    // Moon reflection extending into water (fading)
    let moon_x: i32 = 260;
    unsafe {
        let tick = GAME.tick;
        for y in WATER_TOP..(WATER_TOP + 30) {
            let shimmer = isin((y as u32 * 40 + tick as u32 * 12) % 1024);
            let offset = shimmer / 24;
            let px = moon_x + offset;
            let alpha = (40 - ((y - WATER_TOP) as i32 * 40 / 30)).max(0) as u8;
            put_pixel_alpha(px, y as i32, COL_MOON, alpha);
        }
    }
}

fn render_particles(tick: u32) {
    unsafe {
        for p in &GAME.particles {
            if !p.active { continue; }
            let px = p.x / 256;
            let py = p.y / 256;
            let life_ratio = if p.max_life > 0 { (p.life as u32 * 255 / p.max_life as u32) as u8 } else { 0 };
            let alpha = life_ratio;
            // Pulsing
            let pulse = isin((tick as u32 * 8 + px as u32 * 7) % 1024);
            let a = (alpha as i32 + pulse / 4).max(0).min(255) as u8;
            put_pixel_alpha(px as i32, py as i32, p.color, a);
        }
    }
}

fn render_fish_entity(f: &Fish, tick: u32, is_silhouette: bool) {
    let def = &FISH_DEFS[f.species as usize];
    let px = f.x / 256;
    let py = f.y / 256;
    let len = def.body_len as i32;
    let h = def.body_height as i32;

    // Glow aura (larger, dimmer)
    if !is_silhouette {
        let glow_r = len + 3;
        for dy in -glow_r..=glow_r {
            for dx in -glow_r..=glow_r {
                let dist_sq = dx * dx + dy * dy;
                if dist_sq < glow_r * glow_r {
                    let alpha = (60 - dist_sq * 60 / (glow_r * glow_r)).max(0) as u8;
                    // Pulsing glow
                    let pulse = isin((tick as u32 * 4 + f.anim_frame as u32 * 20) % 1024);
                    let a = (alpha as i32 + pulse * alpha as i32 / 512).max(0).min(255) as u8;
                    put_pixel_alpha(px as i32 + dx, py as i32 + dy, def.glow_color, a);
                }
            }
        }
    }

    // Body
    let body_color = if is_silhouette { 0xFF111122 } else { def.body_color };
    let flip = f.dir < 0;

    // Simple fish shape: elliptical body + tail
    for dy in (-h / 2)..=(h / 2) {
        // Width varies by row (elliptical)
        let row_frac = abs_i32(dy) * 256 / (h / 2 + 1);
        let row_width = len - (row_frac * len / 3 / 256);
        for dx in 0..row_width {
            let fx = if flip { px as i32 + dx - len / 2 } else { px as i32 - dx + len / 2 };
            put_pixel(fx, py as i32 + dy, body_color);
        }
    }

    // Tail (triangle)
    let tail_x = if flip { px as i32 + len / 2 + 1 } else { px as i32 - len / 2 - 1 };
    let tail_w: i32 = 3;
    for i in 0..tail_w {
        let th = (i + 1).min(h / 2 + 1);
        for dy in -th..=th {
            let tx = if flip { tail_x + i } else { tail_x - i };
            put_pixel(tx, py as i32 + dy, body_color);
        }
    }

    // Eye
    if !is_silhouette {
        let eye_x = if flip { px as i32 - len / 3 } else { px as i32 + len / 3 };
        let eye_y = py as i32 - 1;
        put_pixel(eye_x, eye_y, COL_WHITE);

        // Species-specific details
        match f.species {
            FISH_EMBEREEL => {
                // Sine wave body undulation
                let wave = isin((f.anim_frame as u32 * 6) % 1024) / 64;
                put_pixel(px as i32, py as i32 + wave as i32, def.glow_color);
            }
            FISH_GHOSTFIN => {
                // Semi-transparent fins
                let fin_alpha = ((isin((tick as u32 * 3) % 1024) + 127) / 2) as u8;
                put_pixel_alpha(px as i32, py as i32 - h / 2 - 1, def.glow_color, fin_alpha as u8);
                put_pixel_alpha(px as i32 + 1, py as i32 - h / 2 - 1, def.glow_color, fin_alpha / 2);
            }
            FISH_ABYSSAL => {
                // Lantern (bright dot above head)
                let lantern_y = py as i32 - h / 2 - 2;
                let lant_bright = ((isin((tick as u32 * 6) % 1024) + 127) * 200 / 255 + 55) as u8;
                let lant_col = color_alpha(def.glow_color, lant_bright);
                put_pixel(px as i32 + if flip { -2 } else { 2 }, lantern_y, lant_col);
                put_pixel_alpha(px as i32 + if flip { -2 } else { 2 }, lantern_y - 1, def.glow_color, lant_bright / 2);
            }
            FISH_SHIMMER => {
                // Gold trail particles
                let trail_x = if flip { px as i32 + len / 2 + 3 } else { px as i32 - len / 2 - 3 };
                for i in 0..3 {
                    let ta = (80 - i * 25) as u8;
                    put_pixel_alpha(trail_x + if flip { i } else { -i }, py as i32, COL_GOLD, ta);
                }
            }
            _ => {}
        }
    }
}

fn render_fish(tick: u32) {
    unsafe {
        for f in &GAME.fish {
            if !f.active { continue; }
            render_fish_entity(f, tick, false);
        }
    }
}

fn render_dock() {
    // Dock: left side, straddling waterline (y ~35 to ~65)
    let dock_x = 10;
    let dock_w = 30;
    let dock_y = 36;
    let dock_h = 28;

    // Planks
    fill_rect(dock_x, dock_y, dock_w, 3, COL_DOCK);
    fill_rect(dock_x, dock_y + 3, dock_w, 1, COL_DOCK_DARK);
    fill_rect(dock_x, dock_y + 4, dock_w, 3, COL_DOCK);
    fill_rect(dock_x, dock_y + 7, dock_w, 1, COL_DOCK_DARK);

    // Support posts (going down into water)
    fill_rect(dock_x + 2, dock_y + 8, 2, dock_h - 8, COL_DOCK_DARK);
    fill_rect(dock_x + dock_w - 4, dock_y + 8, 2, dock_h - 8, COL_DOCK_DARK);
    fill_rect(dock_x + dock_w / 2 - 1, dock_y + 8, 2, dock_h - 8, COL_DOCK_DARK);
}

fn render_player(tick: u32) {
    // Player on right edge of dock
    let px: i32 = 38;
    let py: i32 = 30; // feet at dock surface

    // Hat
    fill_rect(px - 2, py - 10, 5, 2, COL_PLAYER_HAT);
    // Head
    fill_rect(px - 1, py - 8, 3, 3, COL_PLAYER_SKIN);
    // Body/shirt
    fill_rect(px - 2, py - 5, 5, 4, COL_PLAYER_SHIRT);
    // Pants
    fill_rect(px - 2, py - 1, 5, 3, COL_PLAYER_PANTS);
    // Feet
    fill_rect(px - 2, py + 2, 2, 1, COL_PLAYER_PANTS);
    fill_rect(px + 1, py + 2, 2, 1, COL_PLAYER_PANTS);

    // Arm holding rod (extends right)
    put_pixel(px + 2, py - 4, COL_PLAYER_SKIN);
    put_pixel(px + 3, py - 5, COL_PLAYER_SKIN);

    // Rod
    let rod_base_x = px + 3;
    let rod_base_y = py - 5;
    let rod_tip_x = px + 14;
    let rod_tip_y = py - 12;

    // Draw rod as line
    let dx = rod_tip_x - rod_base_x;
    let dy = rod_tip_y - rod_base_y;
    let steps = if abs_i32(dx) > abs_i32(dy) { abs_i32(dx) } else { abs_i32(dy) };
    if steps > 0 {
        for i in 0..=steps {
            let lx = rod_base_x + dx * i / steps;
            let ly = rod_base_y + dy * i / steps;
            put_pixel(lx, ly, COL_DOCK);
        }
    }

    // Fishing line from rod tip
    unsafe {
        match GAME.state {
            STATE_WAITING | STATE_BITING | STATE_REELING => {
                let line_end_x = GAME.lure_x;
                let bobber_bob = isin((GAME.bobber_phase as u32) % 1024) / 128; // ±1

                let line_end_y = if GAME.state == STATE_BITING {
                    GAME.bobber_y_base + 4 // dipped
                } else if GAME.state == STATE_REELING {
                    GAME.fish_reel_y
                } else {
                    GAME.bobber_y_base + bobber_bob as i32
                };

                // Line curvature based on tension
                let sag = if GAME.state == STATE_REELING {
                    let t = GAME.tension as i32;
                    if t > 200 { 0 } else { (200 - t) / 20 }
                } else {
                    6 // gentle sag while waiting
                };

                draw_fishing_line(rod_tip_x, rod_tip_y, line_end_x, line_end_y, sag, tick);

                // Bobber (only in WAITING/BITING)
                if GAME.state == STATE_WAITING || GAME.state == STATE_BITING {
                    let by = line_end_y;
                    let bx = line_end_x;
                    // Red top, white bottom
                    put_pixel(bx, by - 1, COL_RED);
                    put_pixel(bx, by, COL_WHITE);
                    put_pixel(bx - 1, by, COL_WHITE);
                    put_pixel(bx + 1, by, COL_WHITE);

                    // "!" indicator when biting
                    if GAME.state == STATE_BITING {
                        let flash = (tick / 4) % 2 == 0;
                        if flash {
                            draw_char(bx - 1, by - 10, b'!', COL_YELLOW);
                        }
                    }

                    // Twitch indicator
                    if GAME.twitch_timer > 0 && GAME.state == STATE_WAITING {
                        put_pixel(bx, by + 2, COL_WHITE);
                    }
                }

                // Draw lure underwater
                if GAME.state == STATE_WAITING || GAME.state == STATE_BITING {
                    let lure_col = lure_color(GAME.current_lure);
                    let ly = GAME.lure_y;
                    let lx = GAME.lure_x;
                    // Small glowing dot
                    put_pixel(lx, ly, lure_col);
                    put_pixel_alpha(lx - 1, ly, lure_col, 100);
                    put_pixel_alpha(lx + 1, ly, lure_col, 100);
                    put_pixel_alpha(lx, ly - 1, lure_col, 100);
                    put_pixel_alpha(lx, ly + 1, lure_col, 100);
                    // Glow aura
                    for dy in -3i32..=3 {
                        for dx in -3i32..=3 {
                            let d = dx * dx + dy * dy;
                            if d > 1 && d < 10 {
                                put_pixel_alpha(lx + dx, ly + dy, lure_col, (40 - d * 4).max(0) as u8);
                            }
                        }
                    }
                }
            }
            STATE_CASTING => {
                // Animated cast line
                if GAME.cast_anim_timer > 0 {
                    let progress = (20 - GAME.cast_anim_timer as i32) * 256 / 20;
                    let target_x = GAME.lure_x;
                    let target_y = GAME.bobber_y_base;
                    let cur_x = rod_tip_x + (target_x - rod_tip_x) * progress / 256;
                    let cur_y = rod_tip_y + (target_y - rod_tip_y) * progress / 256;
                    // Arc upward
                    let arc = -(progress * (256 - progress)) / 256 * 20 / 256;
                    draw_fishing_line(rod_tip_x, rod_tip_y, cur_x, cur_y + arc, 2, tick);
                }
            }
            _ => {
                // Just a dangling line from rod tip
                vline(rod_tip_x, rod_tip_y, rod_tip_y + 8, COL_LINE);
            }
        }
    }
}

fn draw_fishing_line(x0: i32, y0: i32, x1: i32, y1: i32, sag: i32, _tick: u32) {
    let steps = 20;
    let mut prev_x = x0;
    let mut prev_y = y0;

    unsafe {
        let tension = GAME.tension;
        let line_col = if GAME.state == STATE_REELING && tension > 220 {
            COL_LINE_TENSE
        } else if GAME.state == STATE_REELING && tension > 180 {
            lerp_color(COL_LINE, COL_LINE_TENSE, ((tension as u32 - 180) * 255 / 40) as u8)
        } else {
            COL_LINE
        };

        for i in 1..=steps {
            let t = i * 256 / steps;
            let lx = x0 + (x1 - x0) * t / 256;
            let base_y = y0 + (y1 - y0) * t / 256;
            // Catenary sag: parabolic, max at midpoint
            let mid_factor = t * (256 - t) / 256;
            let ly = base_y + sag * mid_factor / 64;

            // Draw line segment
            let sdx = lx - prev_x;
            let sdy = ly - prev_y;
            let seg_steps = (abs_i32(sdx).max(abs_i32(sdy))).max(1);
            for j in 0..=seg_steps {
                let px = prev_x + sdx * j / seg_steps;
                let py = prev_y + sdy * j / seg_steps;
                put_pixel(px, py, line_col);
            }
            prev_x = lx;
            prev_y = ly;
        }

        // Vibration at high tension
        if GAME.state == STATE_REELING && tension > 160 {
            let vib = ((GAME.tick % 3) as i32) - 1; // -1, 0, or 1
            let mid_x = (x0 + x1) / 2 + vib;
            let mid_y = (y0 + y1) / 2;
            put_pixel(mid_x, mid_y, line_col);
        }
    }
}

fn render_splash(_tick: u32) {
    unsafe {
        if GAME.splash_timer > 0 {
            let sx = GAME.splash_x;
            let sy = SURFACE_BOTTOM as i32 - 2;
            let spread = (12 - GAME.splash_timer as i32) * 2;
            let alpha = (GAME.splash_timer as u8 * 20).min(200);

            // Ripple rings
            for dx in -spread..=spread {
                let a = alpha / (abs_i32(dx) as u8 / 2 + 1);
                put_pixel_alpha(sx + dx, sy, COL_WHITE, a);
                if spread > 3 {
                    put_pixel_alpha(sx + dx, sy - 1, COL_WHITE, a / 2);
                }
            }
        }
    }
}

fn render_depth_marks() {
    // Right edge depth indicator
    let x = W as i32 - 4;
    let marks = [(70, b"S"), (110, b"M"), (150, b"D"), (185, b"A")];
    for (y, _label) in &marks {
        hline(x, x + 2, *y as i32, COL_DARK_GRAY);
    }
}

// ── HUD Rendering ────────────────────────────────────────────────────
fn render_hud(tick: u32) {
    unsafe {
        // Lure selection (top-left)
        draw_text(4, 4, b"LURE:", COL_GRAY);
        for i in 0u8..4 {
            let lx = 24 + i as i32 * 12;
            let col = lure_color(i);
            if GAME.lures_unlocked[i as usize] {
                let num = [b'1' + i];
                if i == GAME.current_lure {
                    // Selected: bright, with bracket
                    fill_rect(lx - 1, 2, 10, 9, 0xFF222244);
                    draw_char(lx, 4, num[0], col);
                    // Underline
                    hline(lx, lx + 3, 10, col);
                } else {
                    draw_char(lx, 4, num[0], color_alpha(col, 150));
                }
            } else {
                // Locked: dimmed
                let num = [b'1' + i];
                draw_char(lx, 4, num[0], COL_DARK_GRAY);
            }
        }

        // Fish count (top-right)
        draw_text(W as i32 - 40, 4, b"FISH:", COL_GRAY);
        draw_number(W as i32 - 16, 4, GAME.total_caught as u32, COL_WHITE);

        // State-specific UI
        match GAME.state {
            STATE_IDLE => {
                // Prompt
                let flash = (tick / 30) % 2 == 0;
                if flash {
                    draw_text_centered(H as i32 - 14, b"HOLD SPACE TO CAST", COL_WHITE);
                }
            }
            STATE_CASTING => {
                // Power bar
                let bar_x = W as i32 / 2 - 30;
                let bar_y = H as i32 - 20;
                let bar_w = 60;
                let bar_h = 6;
                fill_rect(bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, COL_DARK_GRAY);
                let fill_w = GAME.cast_power as i32 * bar_w / 255;
                let fill_col = if GAME.cast_power > 200 { COL_RED } else if GAME.cast_power > 128 { COL_YELLOW } else { COL_GREEN_UI };
                fill_rect(bar_x, bar_y, fill_w, bar_h, fill_col);
                draw_text_centered(bar_y - 10, b"POWER", COL_GRAY);
            }
            STATE_WAITING => {
                // Timer
                let _seconds = GAME.wait_timer / 60;
                // Just show a subtle "..." animation
                let dots = (tick / 20) % 4;
                let mut buf = [b'.'; 3];
                for i in dots as usize..3 { buf[i] = b' '; }
                draw_text(W as i32 / 2 - 6, H as i32 - 10, &buf[..3], COL_DARK_GRAY);
            }
            STATE_BITING => {
                // Flashing prompt
                let flash = (tick / 3) % 2 == 0;
                if flash {
                    draw_text_centered(H as i32 - 14, b"SPACE!", COL_YELLOW);
                }
            }
            STATE_REELING => {
                // Tension meter
                let bar_x = W as i32 / 2 - 40;
                let bar_y = H as i32 - 28;
                let bar_w = 80;
                let bar_h = 6;

                // Background
                fill_rect(bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, COL_DARK_GRAY);

                // Green zone — bite_fish_idx is a fish SLOT index, not a species index
                let def = &FISH_DEFS[GAME.fish[GAME.bite_fish_idx as usize].species as usize];
                let green_w = def.green_zone as i32 * bar_w / 100;
                fill_rect(bar_x, bar_y, green_w, bar_h, 0xFF113311);

                // Danger zone
                let danger_start = bar_w * 80 / 100;
                fill_rect(bar_x + danger_start, bar_y, bar_w - danger_start, bar_h, 0xFF331111);

                // Tension fill
                let tension_w = GAME.tension as i32 * bar_w / 255;
                let t_col = if GAME.tension > 220 { COL_RED }
                    else if GAME.tension > 180 { COL_YELLOW }
                    else { COL_GREEN_UI };
                fill_rect(bar_x, bar_y, tension_w, bar_h, t_col);

                draw_text(bar_x, bar_y - 8, b"TENSION", COL_GRAY);

                // Progress bar (below tension)
                let prog_y = bar_y + 12;
                fill_rect(bar_x - 1, prog_y - 1, bar_w + 2, 4 + 2, COL_DARK_GRAY);
                let prog_w = GAME.reel_progress as i32 * bar_w / 255;
                fill_rect(bar_x, prog_y, prog_w, 4, COL_CYAN);
                draw_text(bar_x, prog_y + 6, b"REEL", COL_GRAY);
            }
            STATE_CAUGHT => {
                // Species reveal panel
                let panel_x = W as i32 / 2 - 60;
                let panel_y = 50;
                let panel_w = 120;
                let panel_h = 60;

                // Panel background
                fill_rect(panel_x - 1, panel_y - 1, panel_w + 2, panel_h + 2, COL_WHITE);
                fill_rect(panel_x, panel_y, panel_w, panel_h, 0xFF0A0A1A);

                // Species name
                let name = FISH_NAMES[GAME.caught_species as usize];
                let name_col = FISH_DEFS[GAME.caught_species as usize].glow_color;
                let nx = panel_x + (panel_w - text_width(name)) / 2;
                draw_text(nx, panel_y + 6, name, name_col);

                // Draw a centered fish
                let fake_fish = Fish {
                    active: true, species: GAME.caught_species,
                    x: (W as i32 / 2) * 256, y: (panel_y + 30) * 256,
                    dir: 1, anim_frame: (tick as u16) % 1000,
                    state: 0, circle_angle: 0, flee_timer: 0,
                };
                render_fish_entity(&fake_fish, tick, false);

                // Weight
                draw_weight(panel_x + 10, panel_y + 44, GAME.caught_weight, COL_WHITE);

                // "One more cast" prompt
                if GAME.caught_timer < 120 {
                    let flash = (tick / 20) % 2 == 0;
                    if flash {
                        draw_text_centered(panel_y + panel_h + 8, b"SPACE TO CONTINUE", COL_GRAY);
                    }
                }

                // Show fish silhouettes briefly
                if GAME.show_silhouettes {
                    for f in &GAME.fish {
                        if !f.active { continue; }
                        render_fish_entity(f, tick, true);
                    }
                }
            }
            STATE_LOST => {
                let flash = (tick / 15) % 2 == 0;
                if flash {
                    draw_text_centered(H as i32 / 2, b"LINE SNAPPED!", COL_RED);
                }
                if GAME.lost_timer < 60 {
                    draw_text_centered(H as i32 / 2 + 12, b"SPACE TO CONTINUE", COL_GRAY);
                }
            }
            STATE_WIN => {
                // Full-screen collection display
                fill_rect(0, 0, W as i32, H as i32, 0xFF050510);

                // Title
                let title_y = 12;
                let fade = ((GAME.win_timer as u32 * 4).min(255)) as u8;
                let title_col = color_alpha(COL_GOLD, fade);
                draw_text_centered(title_y, b"MOONPOOL COMPLETE!", title_col);

                // Subtitle
                draw_text_centered(title_y + 10, b"ALL SPECIES CAUGHT", color_alpha(COL_WHITE, fade));

                // Collection grid — 2 columns, 3 rows
                let grid_x = 30;
                let grid_y = 36;
                let col_w = 130;
                let row_h = 24;

                for s in 0..NUM_SPECIES {
                    let col = s % 2;
                    let row = s / 2;
                    let cx = grid_x + col as i32 * col_w;
                    let cy = grid_y + row as i32 * row_h;

                    // Fade in each species sequentially
                    let delay = s as u16 * 15;
                    if GAME.win_timer < delay { continue; }
                    let entry_fade = (((GAME.win_timer - delay) as u32 * 8).min(255)) as u8;

                    let def = &FISH_DEFS[s];
                    let name = FISH_NAMES[s];

                    // Mini fish sprite
                    let fish_x = cx + 6;
                    let fish_y = cy + 6;
                    let mini_fish = Fish {
                        active: true, species: s as u8,
                        x: fish_x * 256, y: fish_y * 256,
                        dir: 1, anim_frame: (tick as u16).wrapping_add(s as u16 * 100),
                        state: 0, circle_angle: 0, flee_timer: 0,
                    };
                    render_fish_entity(&mini_fish, tick, false);

                    // Name
                    let name_col = color_alpha(def.glow_color, entry_fade);
                    draw_text(cx + 22, cy + 2, name, name_col);

                    // Count + best weight
                    let count = GAME.fish_caught_count[s];
                    let best = GAME.best_weight[s];
                    let info_y = cy + 10;
                    draw_text(cx + 22, info_y, b"x", color_alpha(COL_GRAY, entry_fade));
                    draw_number(cx + 26, info_y, count as u32, color_alpha(COL_WHITE, entry_fade));
                    draw_text(cx + 42, info_y, b"BEST:", color_alpha(COL_GRAY, entry_fade));
                    draw_weight(cx + 64, info_y, best, color_alpha(COL_WHITE, entry_fade));
                }

                // Total count
                let total_y = grid_y + 3 * row_h + 8;
                if GAME.win_timer > 100 {
                    draw_text(grid_x, total_y, b"TOTAL CAUGHT:", COL_GRAY);
                    draw_number(grid_x + 56, total_y, GAME.total_caught as u32, COL_WHITE);
                }
            }
            _ => {}
        }
    }
}

// ── Screen Shake ─────────────────────────────────────────────────────
fn apply_shake() {
    unsafe {
        if GAME.shake_timer > 0 {
            GAME.shake_timer -= 1;
            let r = xorshift32(&mut GAME.rng_state);
            GAME.shake_offset_x = ((r % 5) as i8) - 2;
            GAME.shake_offset_y = (((r >> 8) % 5) as i8) - 2;
        } else {
            GAME.shake_offset_x = 0;
            GAME.shake_offset_y = 0;
        }
    }
}

// ── Fish Spawning & AI ───────────────────────────────────────────────
fn spawn_fish() {
    unsafe {
        // Count active fish
        let active_count = GAME.fish.iter().filter(|f| f.active).count();
        if active_count >= MAX_FISH - 2 { return; }

        // Weighted random species selection
        let total_weight: u32 = FISH_DEFS.iter().map(|d| d.rarity_weight as u32).sum();
        let mut roll = (xorshift32(&mut GAME.rng_state) % total_weight) as u32;
        let mut species = 0u8;
        for (i, def) in FISH_DEFS.iter().enumerate() {
            if roll < def.rarity_weight as u32 {
                species = i as u8;
                break;
            }
            roll -= def.rarity_weight as u32;
        }

        let def = &FISH_DEFS[species as usize];

        // Find empty slot
        for f in &mut GAME.fish {
            if f.active { continue; }

            let r = xorshift32(&mut GAME.rng_state);
            let depth = def.depth_min + (r % (def.depth_max - def.depth_min + 1));
            let dir: i8 = if r & 0x100 != 0 { 1 } else { -1 };
            let start_x = if dir > 0 { -20 } else { W as i32 + 20 };

            *f = Fish {
                active: true,
                species,
                x: start_x * 256,
                y: depth as i32 * 256,
                dir,
                anim_frame: (r & 0xFFFF) as u16,
                state: 0, // swimming
                circle_angle: 0,
                flee_timer: 0,
            };
            break;
        }
    }
}

fn update_fish() {
    unsafe {
        for i in 0..MAX_FISH {
            if !GAME.fish[i].active { continue; }
            let species = GAME.fish[i].species;
            let def = &FISH_DEFS[species as usize];

            GAME.fish[i].anim_frame = GAME.fish[i].anim_frame.wrapping_add(1);

            match GAME.fish[i].state {
                0 => {
                    // Swimming — move horizontally with species-specific behavior
                    let speed = def.speed;
                    let dir = GAME.fish[i].dir as i32;

                    match species {
                        FISH_EMBEREEL => {
                            // Sine wave motion
                            let wave = isin((GAME.fish[i].anim_frame as u32 * 8) % 1024);
                            GAME.fish[i].x += dir * speed / 2;
                            GAME.fish[i].y += wave / 4;
                        }
                        FISH_GHOSTFIN => {
                            // Slow drift
                            GAME.fish[i].x += dir * speed / 3;
                            // Slight vertical bob
                            let bob = isin((GAME.fish[i].anim_frame as u32 * 2) % 1024);
                            GAME.fish[i].y += bob / 16;
                        }
                        FISH_VENOMJAW => {
                            // Fast, direct
                            GAME.fish[i].x += dir * speed;
                        }
                        FISH_ABYSSAL => {
                            // Slow, pulsing movement
                            let pulse = isin((GAME.fish[i].anim_frame as u32 * 3) % 1024);
                            let s = speed / 3 + (pulse.max(0) as i32 * speed / 512);
                            GAME.fish[i].x += dir * s;
                        }
                        FISH_SHIMMER => {
                            // Smooth glide
                            GAME.fish[i].x += dir * speed;
                            let wave = isin((GAME.fish[i].anim_frame as u32 * 2) % 1024);
                            GAME.fish[i].y += wave / 8;
                        }
                        _ => {
                            // Moonminnow: basic swim
                            GAME.fish[i].x += dir * speed / 2;
                        }
                    }

                    // Check if attracted to lure (only during WAITING)
                    if GAME.state == STATE_WAITING {
                        let lure_attracted = match def.attracted_by {
                            0xFF => true, // Ghostfin: any lure
                            0xFE => {     // Shimmer Ray: random chance regardless
                                xorshift32(&mut GAME.rng_state) % 2000 == 0
                            }
                            lure => {
                                if lure == GAME.current_lure {
                                    true
                                } else {
                                    // Small chance with wrong lure
                                    xorshift32(&mut GAME.rng_state) % 500 == 0
                                }
                            }
                        };

                        if lure_attracted {
                            let dx = GAME.lure_x * 256 - GAME.fish[i].x;
                            let dy = GAME.lure_y * 256 - GAME.fish[i].y;
                            let dist = isqrt((dx / 256 * dx / 256 + dy / 256 * dy / 256) as u32);

                            let attract_range: u32 = if def.attracted_by == GAME.current_lure || def.attracted_by == 0xFF {
                                80
                            } else {
                                30
                            };

                            if dist < attract_range {
                                GAME.fish[i].state = 1; // attracted
                            }
                        }
                    }

                    // Despawn if off-screen
                    let px = GAME.fish[i].x / 256;
                    if px < -40 || px > W as i32 + 40 {
                        GAME.fish[i].active = false;
                    }
                }
                1 => {
                    // Attracted — swim toward lure
                    let dx = GAME.lure_x * 256 - GAME.fish[i].x;
                    let dy = GAME.lure_y * 256 - GAME.fish[i].y;
                    let dist = isqrt(((dx / 256) * (dx / 256) + (dy / 256) * (dy / 256)) as u32) as i32;

                    if dist > 0 {
                        let speed = def.speed / 2;
                        GAME.fish[i].x += dx * speed / (dist * 256);
                        GAME.fish[i].y += dy * speed / (dist * 256);
                        GAME.fish[i].dir = if dx > 0 { 1 } else { -1 };
                    }

                    // Switch to circling when close
                    if dist < 15 {
                        GAME.fish[i].state = 2; // circling
                        GAME.fish[i].circle_angle = 0;
                    }

                    // Cancel attraction if lure gone
                    if GAME.state != STATE_WAITING {
                        GAME.fish[i].state = 0;
                    }
                }
                2 => {
                    // Circling — orbit around lure
                    GAME.fish[i].circle_angle = GAME.fish[i].circle_angle.wrapping_add(6);
                    let angle = GAME.fish[i].circle_angle as u32;
                    let radius: i32 = 12;
                    let target_x = GAME.lure_x * 256 + icos(angle) * radius * 2;
                    let target_y = GAME.lure_y * 256 + isin(angle) * radius * 2;

                    GAME.fish[i].x += (target_x - GAME.fish[i].x) / 8;
                    GAME.fish[i].y += (target_y - GAME.fish[i].y) / 8;
                    GAME.fish[i].dir = if icos(angle) < 0 { -1 } else { 1 };

                    // After enough circling, inspect (then bite)
                    if GAME.fish[i].circle_angle > 400 {
                        GAME.fish[i].state = 3; // inspecting
                        GAME.fish[i].circle_angle = 0;
                    }

                    if GAME.state != STATE_WAITING {
                        GAME.fish[i].state = 0;
                    }
                }
                3 => {
                    // Inspecting — move close, then trigger bite
                    let dx = GAME.lure_x * 256 - GAME.fish[i].x;
                    let dy = GAME.lure_y * 256 - GAME.fish[i].y;
                    GAME.fish[i].x += dx / 16;
                    GAME.fish[i].y += dy / 16;
                    GAME.fish[i].dir = if dx > 0 { 1 } else { -1 };

                    GAME.fish[i].circle_angle += 1;

                    // After inspection time, trigger bite
                    let inspect_time: u16 = match species {
                        FISH_MOONMINNOW => 30,
                        FISH_EMBEREEL => 60,
                        FISH_GHOSTFIN => 45,
                        FISH_VENOMJAW => 20,
                        FISH_ABYSSAL => 50,
                        FISH_SHIMMER => 25,
                        _ => 40,
                    };

                    if GAME.fish[i].circle_angle > inspect_time && GAME.state == STATE_WAITING {
                        // Check if we already have enough twitches
                        if GAME.twitch_count >= 1 + (xorshift32(&mut GAME.rng_state) % 2) as u8 {
                            // Trigger bite!
                            trigger_bite(i as u8);
                        } else if GAME.twitch_timer == 0 {
                            // Trigger a twitch (false signal)
                            GAME.twitch_timer = 8;
                            GAME.twitch_count += 1;
                            GAME.fish[i].circle_angle = inspect_time / 2; // Reset to circle more
                        }
                    }

                    if GAME.state != STATE_WAITING {
                        GAME.fish[i].state = 0;
                    }
                }
                4 => {
                    // Fleeing — fast escape
                    let speed = def.speed * 3;
                    GAME.fish[i].x += GAME.fish[i].dir as i32 * speed;
                    GAME.fish[i].flee_timer += 1;
                    if GAME.fish[i].flee_timer > 60 {
                        GAME.fish[i].active = false;
                    }
                }
                _ => {}
            }

            // Keep fish in depth bounds
            let py = GAME.fish[i].y / 256;
            if py < WATER_TOP as i32 + 5 {
                GAME.fish[i].y = (WATER_TOP as i32 + 5) * 256;
            }
            if py > WATER_BOTTOM as i32 - 5 {
                GAME.fish[i].y = (WATER_BOTTOM as i32 - 5) * 256;
            }
        }
    }
}

fn trigger_bite(fish_idx: u8) {
    unsafe {
        GAME.state = STATE_BITING;
        GAME.bite_fish_idx = fish_idx;
        let def = &FISH_DEFS[GAME.fish[fish_idx as usize].species as usize];
        GAME.bite_timer = (def.bite_window_ms / 16) as u16; // convert ms to frames at ~60fps
    }
}

fn scare_fish_near(cx: i32, cy: i32, radius: i32) {
    unsafe {
        for f in &mut GAME.fish {
            if !f.active { continue; }
            let dx = f.x / 256 - cx;
            let dy = f.y / 256 - cy;
            let dist = isqrt((dx * dx + dy * dy) as u32) as i32;
            if dist < radius {
                f.state = 4; // flee
                f.dir = if dx > 0 { 1 } else { -1 };
                f.flee_timer = 0;
            }
        }
    }
}

// ── Particle System ──────────────────────────────────────────────────
fn spawn_ambient_particle() {
    unsafe {
        let r = xorshift32(&mut GAME.rng_state);
        let x = (r % W) as i32;
        let y = (WATER_TOP + (r >> 8) % (WATER_BOTTOM - WATER_TOP)) as i32;
        let colors = [COL_CYAN, COL_GREEN, COL_PURPLE, COL_GHOST_WHITE];
        let col = colors[(r >> 16) as usize % 4];

        for p in &mut GAME.particles {
            if p.active { continue; }
            *p = Particle {
                active: true,
                x: x * 256,
                y: y * 256,
                vx: ((r >> 4) % 3) as i32 - 1,
                vy: -(((r >> 6) % 2) as i32) - 1,
                color: col,
                life: 120 + (r % 60) as u16,
                max_life: 120 + (r % 60) as u16,
            };
            break;
        }
    }
}

fn spawn_catch_burst(x: i32, y: i32, color: u32) {
    unsafe {
        for _ in 0..12 {
            let r = xorshift32(&mut GAME.rng_state);
            let angle = r % 1024;
            let speed = 128 + (r >> 10) % 256;
            let vx = icos(angle) * speed as i32 / 127;
            let vy = isin(angle) * speed as i32 / 127;

            for p in &mut GAME.particles {
                if p.active { continue; }
                *p = Particle {
                    active: true,
                    x: x * 256,
                    y: y * 256,
                    vx,
                    vy,
                    color,
                    life: 40 + (r >> 16) as u16 % 20,
                    max_life: 40 + (r >> 16) as u16 % 20,
                };
                break;
            }
        }
    }
}

fn update_particles() {
    unsafe {
        for p in &mut GAME.particles {
            if !p.active { continue; }
            p.x += p.vx;
            p.y += p.vy;
            if p.life > 0 {
                p.life -= 1;
            } else {
                p.active = false;
            }
        }
    }
}

// ── Game State Machine ───────────────────────────────────────────────
fn key_pressed(code: u8) -> bool {
    unsafe { GAME.keys[code as usize] && !GAME.keys_prev[code as usize] }
}

fn key_held(code: u8) -> bool {
    unsafe { GAME.keys[code as usize] }
}

fn key_released(code: u8) -> bool {
    unsafe { !GAME.keys[code as usize] && GAME.keys_prev[code as usize] }
}

fn update_game_state() {
    unsafe {
        match GAME.state {
            STATE_IDLE => {
                // Lure selection
                if key_pressed(KEY_1) && GAME.lures_unlocked[0] { GAME.current_lure = LURE_CYAN; }
                if key_pressed(KEY_2) && GAME.lures_unlocked[1] { GAME.current_lure = LURE_ORANGE; }
                if key_pressed(KEY_3) && GAME.lures_unlocked[2] { GAME.current_lure = LURE_GREEN; }
                if key_pressed(KEY_4) && GAME.lures_unlocked[3] { GAME.current_lure = LURE_PURPLE; }

                // Start casting
                if key_pressed(KEY_SPACE) {
                    GAME.state = STATE_CASTING;
                    GAME.cast_power = 0;
                    GAME.cast_charging = true;
                }
            }
            STATE_CASTING => {
                if key_held(KEY_SPACE) && GAME.cast_charging {
                    GAME.cast_power = (GAME.cast_power + 3).min(255);
                }

                if key_released(KEY_SPACE) {
                    // Calculate lure landing point
                    let power = GAME.cast_power as i32;
                    let dock_edge_x = 52; // right edge of dock area
                    let cast_range = 50 + power * 200 / 255; // 50..250 pixels from dock
                    GAME.lure_x = dock_edge_x + cast_range * (W as i32 - dock_edge_x - 10) / 300;
                    GAME.lure_x = GAME.lure_x.min(W as i32 - 15);

                    // Depth based on distance
                    let depth_frac = (GAME.lure_x - dock_edge_x) * 256 / (W as i32 - dock_edge_x - 10);
                    GAME.lure_y = WATER_TOP as i32 + 10 + depth_frac * (WATER_BOTTOM as i32 - WATER_TOP as i32 - 20) / 256;

                    GAME.bobber_y_base = SURFACE_BOTTOM as i32 - 2;
                    GAME.bobber_phase = 0;
                    GAME.twitch_count = 0;
                    GAME.twitch_timer = 0;
                    GAME.wait_timer = 0;

                    // Splash
                    GAME.splash_x = GAME.lure_x;
                    GAME.splash_timer = 12;

                    // Scare nearby fish
                    scare_fish_near(GAME.lure_x, GAME.lure_y, 30);

                    // Cast animation
                    GAME.cast_anim_timer = 20;

                    GAME.state = STATE_WAITING;
                    GAME.cast_charging = false;
                }

                if key_pressed(KEY_ESCAPE) {
                    GAME.state = STATE_IDLE;
                    GAME.cast_charging = false;
                }
            }
            STATE_WAITING => {
                GAME.wait_timer += 1;
                GAME.bobber_phase = GAME.bobber_phase.wrapping_add(3); // ~2s period at 60fps

                // Twitch timer
                if GAME.twitch_timer > 0 {
                    GAME.twitch_timer -= 1;
                }

                // Max wait timeout
                let max_wait = 15 * 60; // 15 seconds
                if GAME.wait_timer > max_wait {
                    // No fish bit, return to idle
                    GAME.state = STATE_IDLE;
                }

                if key_pressed(KEY_ESCAPE) {
                    GAME.state = STATE_IDLE;
                }
            }
            STATE_BITING => {
                if GAME.bite_timer > 0 {
                    GAME.bite_timer -= 1;
                }

                if key_pressed(KEY_SPACE) {
                    // Hooked! Transition to reeling
                    GAME.state = STATE_REELING;
                    GAME.tension = 80;
                    GAME.reel_progress = 0;
                    GAME.fish_pull_timer = 0;
                    GAME.fish_pull_dir = 1;

                    let fi = GAME.bite_fish_idx as usize;
                    GAME.fish_reel_x = GAME.fish[fi].x / 256;
                    GAME.fish_reel_y = GAME.fish[fi].y / 256;

                    // Screen shake on hook set
                    GAME.shake_timer = 4;

                    // Deactivate the fish from the swimming pool (it's now "hooked")
                    GAME.fish[fi].active = false;
                }

                if GAME.bite_timer == 0 {
                    // Missed! Fish escapes
                    let fi = GAME.bite_fish_idx as usize;
                    if GAME.fish[fi].active {
                        GAME.fish[fi].state = 4; // flee
                        GAME.fish[fi].flee_timer = 0;
                    }
                    GAME.state = STATE_LOST;
                    GAME.lost_timer = 120;
                }
            }
            STATE_REELING => {
                let fi = GAME.bite_fish_idx as usize;
                let def = &FISH_DEFS[GAME.fish[fi].species as usize];

                // Reeling
                if key_held(KEY_SPACE) {
                    // Reel in: progress increases, tension increases
                    GAME.reel_progress = (GAME.reel_progress + 2).min(255);
                    GAME.tension = (GAME.tension + 2).min(255);
                } else {
                    // Rest: tension drops, progress slowly regresses
                    if GAME.tension > 3 {
                        GAME.tension -= 3;
                    }
                    if GAME.reel_progress > 0 {
                        GAME.reel_progress -= 1;
                    }
                }

                // Fish pulls
                GAME.fish_pull_timer += 1;
                if GAME.fish_pull_timer >= def.pull_freq {
                    GAME.fish_pull_timer = 0;
                    GAME.tension = (GAME.tension as u32 + def.pull_strength as u32).min(255) as u16;
                    // Jerk the fish visually
                    let r = xorshift32(&mut GAME.rng_state);
                    GAME.fish_pull_dir = if r & 1 != 0 { 1 } else { -1 };
                    GAME.fish_reel_x += GAME.fish_pull_dir as i32 * 4;
                }

                // Fish visual — slowly moves toward player as progress increases
                let target_x = 52; // near dock
                let target_y = SURFACE_BOTTOM as i32;
                let base_x = GAME.lure_x;
                let base_y = GAME.lure_y;
                let prog = GAME.reel_progress as i32;
                GAME.fish_reel_x = base_x + (target_x - base_x) * prog / 255 + GAME.fish_pull_dir as i32 * 2;
                GAME.fish_reel_y = base_y + (target_y - base_y) * prog / 255;

                // Tension break
                if GAME.tension >= 255 {
                    GAME.state = STATE_LOST;
                    GAME.lost_timer = 120;
                }

                // Caught!
                if GAME.reel_progress >= 255 {
                    let species = GAME.fish[fi].species;
                    let def = &FISH_DEFS[species as usize];

                    // Random weight
                    let r = xorshift32(&mut GAME.rng_state);
                    let weight_range = def.weight_max - def.weight_min;
                    let weight = def.weight_min + (r as u16 % (weight_range + 1));

                    GAME.caught_species = species;
                    GAME.caught_weight = weight;
                    GAME.caught_timer = 180; // 3 seconds display
                    GAME.show_silhouettes = true;
                    GAME.total_caught += 1;
                    GAME.fish_caught_count[species as usize] += 1;

                    // Best weight
                    if weight > GAME.best_weight[species as usize] {
                        GAME.best_weight[species as usize] = weight;
                    }

                    // Lure unlock checks
                    check_unlocks();

                    // Particle burst
                    spawn_catch_burst(W as i32 / 2, 80, def.glow_color);

                    GAME.state = STATE_CAUGHT;
                }
            }
            STATE_CAUGHT => {
                if GAME.caught_timer > 0 {
                    GAME.caught_timer -= 1;
                }
                if GAME.caught_timer < 60 {
                    GAME.show_silhouettes = false;
                }
                if key_pressed(KEY_SPACE) && GAME.caught_timer < 120 {
                    // Check if all 6 species caught
                    if all_species_caught() {
                        GAME.state = STATE_WIN;
                        GAME.win_timer = 0;
                    } else {
                        GAME.state = STATE_IDLE;
                    }
                }
                // Auto-transition after timer
                if GAME.caught_timer == 0 {
                    if all_species_caught() {
                        GAME.state = STATE_WIN;
                        GAME.win_timer = 0;
                    } else {
                        GAME.state = STATE_IDLE;
                    }
                }
            }
            STATE_WIN => {
                GAME.win_timer += 1;
                // Spawn celebration particles
                if GAME.win_timer % 10 == 0 {
                    let r = xorshift32(&mut GAME.rng_state);
                    let x = 40 + (r % 240) as i32;
                    let colors = [COL_CYAN, COL_ORANGE, COL_GREEN, COL_PURPLE, COL_GOLD, COL_GHOST_WHITE];
                    let col = colors[(r >> 8) as usize % 6];
                    spawn_catch_burst(x, 60 + (r >> 16) as i32 % 80, col);
                }
            }
            STATE_LOST => {
                if GAME.lost_timer > 0 {
                    GAME.lost_timer -= 1;
                }
                if key_pressed(KEY_SPACE) && GAME.lost_timer < 60 {
                    GAME.state = STATE_IDLE;
                }
                if GAME.lost_timer == 0 {
                    GAME.state = STATE_IDLE;
                }
            }
            _ => { GAME.state = STATE_IDLE; }
        }
    }
}

fn check_unlocks() {
    unsafe {
        // Moonminnow caught → unlock Orange
        if GAME.fish_caught_count[FISH_MOONMINNOW as usize] > 0 && !GAME.lures_unlocked[LURE_ORANGE as usize] {
            GAME.lures_unlocked[LURE_ORANGE as usize] = true;
            log_msg(b"Unlocked: Orange lure!");
        }
        // Embereel caught → unlock Green
        if GAME.fish_caught_count[FISH_EMBEREEL as usize] > 0 && !GAME.lures_unlocked[LURE_GREEN as usize] {
            GAME.lures_unlocked[LURE_GREEN as usize] = true;
            log_msg(b"Unlocked: Green lure!");
        }
        // Venomjaw caught → unlock Purple
        if GAME.fish_caught_count[FISH_VENOMJAW as usize] > 0 && !GAME.lures_unlocked[LURE_PURPLE as usize] {
            GAME.lures_unlocked[LURE_PURPLE as usize] = true;
            log_msg(b"Unlocked: Purple lure!");
        }
    }
}

fn all_species_caught() -> bool {
    unsafe {
        let mut i = 0;
        while i < NUM_SPECIES {
            if GAME.fish_caught_count[i] == 0 {
                return false;
            }
            i += 1;
        }
        true
    }
}

// ── Input Processing ─────────────────────────────────────────────────
fn process_input() {
    let mut buf = [0u8; 64];
    let bytes_read = unsafe {
        host_poll_input(buf.as_mut_ptr() as u32, buf.len() as u32)
    };

    unsafe {
        // Save previous key state
        GAME.keys_prev = GAME.keys;
    }

    let events = bytes_read as usize / 4;
    for i in 0..events {
        let base = i * 4;
        let event_type = buf[base];
        let key_code = buf[base + 1];

        unsafe {
            match event_type {
                1 => GAME.keys[key_code as usize] = true,  // key down
                2 => GAME.keys[key_code as usize] = false,  // key up
                _ => {}
            }
        }
    }
}

// ── Main Update Loop ─────────────────────────────────────────────────
#[polkavm_derive::polkavm_export]
extern "C" fn init() {
    log_msg(b"moonpool: init");

    // Initialize stars
    unsafe {
        for i in 0..MAX_STARS {
            let r = xorshift32(&mut GAME.rng_state);
            GAME.stars[i] = Star {
                x: (r % W) as u16,
                y: ((r >> 8) % SKY_BOTTOM) as u16,
                brightness: (100 + (r >> 16) % 156) as u8,
                twinkle_phase: (r >> 4) as u16 % 1024,
            };
        }

        // Spawn initial fish
        for _ in 0..6 {
            spawn_fish();
            // Spread them out by placing in random positions
            for f in &mut GAME.fish {
                if f.active {
                    let r = xorshift32(&mut GAME.rng_state);
                    f.x = (r % W) as i32 * 256;
                }
            }
        }

        GAME.last_time_ms = host_time_ms();
    }
}

#[polkavm_derive::polkavm_export]
extern "C" fn update() {
    let now = unsafe { host_time_ms() };

    // Process input
    process_input();

    unsafe {
        GAME.tick = GAME.tick.wrapping_add(1);
        let tick = GAME.tick;

        // Update game state
        update_game_state();

        // Spawn new fish periodically
        if tick % 90 == 0 {
            spawn_fish();
        }

        // Spawn ambient particles
        if tick % 30 == 0 {
            spawn_ambient_particle();
        }

        // Update fish AI
        update_fish();

        // Update particles
        update_particles();

        // Update splash
        if GAME.splash_timer > 0 {
            GAME.splash_timer -= 1;
        }

        // Update cast animation
        if GAME.cast_anim_timer > 0 {
            GAME.cast_anim_timer -= 1;
        }

        // Screen shake
        apply_shake();

        // ── Render ───────────────────────────────────────────────
        render_sky(tick);
        render_water_surface(tick);
        render_underwater();
        render_particles(tick);
        render_fish(tick);
        render_dock();
        render_player(tick);
        render_splash(tick);
        render_depth_marks();
        render_hud(tick);

        // Apply screen shake by shifting framebuffer
        if GAME.shake_offset_x != 0 || GAME.shake_offset_y != 0 {
            // Simple: we just offset the present call won't work, so we copy with offset
            // For simplicity, just present — the visual jitter from per-pixel offset
            // is expensive. Instead we apply shake to UI elements in future refinement.
            // Actually let's do a fast row-copy approach:
            let ox = GAME.shake_offset_x as i32;
            let oy = GAME.shake_offset_y as i32;
            if ox != 0 || oy != 0 {
                // Just shift a few rows to create the effect
                // Simple approach: shift render of the HUD only (already rendered)
                // For full shake, we'd need a second buffer. Skip for perf.
            }
        }

        // Present frame
        host_present_frame(
            FB.as_ptr() as u32,
            W,
            H,
            W * 4,
        );

        GAME.last_time_ms = now;
    }
}
