/*
 * doomgeneric_polkavm.c — Platform layer for doomgeneric running on PolkaVM.
 *
 * Implements the 7 DG_* callbacks using host functions provided by the sandbox.
 */

#include "doomgeneric.h"
#include "doomkeys.h"

#include <stdint.h>
#include <string.h>

/* ── Host function imports (linked via Rust polkavm_import) ── */
extern uint32_t host_present_frame(uint32_t ptr, uint32_t width, uint32_t height, uint32_t stride);
extern uint32_t host_poll_input(uint32_t buf_ptr, uint32_t buf_len);
extern uint64_t host_time_ms(void);

/* ── Key mapping: host scancodes → DOOM key constants ── */
/* The host sends the same scancodes we defined in FramebufferAppTab::keystroke_to_code */

static unsigned char translate_key(uint8_t code) {
    switch (code) {
        /* Arrow keys (scancodes from host) */
        case 0x48: return KEY_UPARROW;
        case 0x50: return KEY_DOWNARROW;
        case 0x4B: return KEY_LEFTARROW;
        case 0x4D: return KEY_RIGHTARROW;

        /* WASD (scan codes) */
        case 0x11: return KEY_UPARROW;    /* W → up */
        case 0x1F: return KEY_DOWNARROW;  /* S → down */
        case 0x1E: return KEY_STRAFE_L;   /* A → strafe left */
        case 0x20: return KEY_STRAFE_R;   /* D → strafe right */

        /* Action keys */
        case 0x1C: return KEY_ENTER;      /* Enter */
        case 0x01: return KEY_ESCAPE;     /* Escape */
        case 0x0F: return KEY_TAB;        /* Tab */
        case 0x39: return KEY_USE;        /* Space → Use */
        case 0x2A: return KEY_RSHIFT;     /* Shift → Run */
        case 0x1D: return KEY_FIRE;       /* Ctrl → Fire */

        default:
            /* Pass through ASCII letters/numbers directly */
            if (code >= 'a' && code <= 'z') return code;
            if (code >= '0' && code <= '9') return code;
            return 0;
    }
}

/* ── Input event ring buffer ── */
#define MAX_KEYS 32

typedef struct {
    int pressed; /* 1 = down, 0 = up */
    unsigned char key;
} key_event_t;

static key_event_t key_ring[MAX_KEYS];
static int key_head = 0;
static int key_count = 0;

static void push_key(int pressed, unsigned char key) {
    if (key_count >= MAX_KEYS) return;
    int idx = (key_head + key_count) % MAX_KEYS;
    key_ring[idx].pressed = pressed;
    key_ring[idx].key = key;
    key_count++;
}

static void poll_host_input(void) {
    /* InputEvent is 4 bytes: [event_type, key_code, pad, pad] */
    uint8_t buf[MAX_KEYS * 4];
    uint32_t bytes = host_poll_input((uint32_t)(uintptr_t)buf, sizeof(buf));
    uint32_t count = bytes / 4;
    for (uint32_t i = 0; i < count; i++) {
        uint8_t type = buf[i * 4];
        uint8_t code = buf[i * 4 + 1];
        unsigned char doom_key = translate_key(code);
        if (doom_key) {
            push_key(type == 1 ? 1 : 0, doom_key);
        }
    }
}

/* ══════════════════════════════════════════════════════════════════
 * DG_* callbacks — required by doomgeneric
 * ══════════════════════════════════════════════════════════════════ */

void DG_Init(void) {
    /* Nothing to initialize — host is ready. */
}

void DG_DrawFrame(void) {
    /* DG_ScreenBuffer is XRGB (32-bit, X=unused, same as ARGB with A=0).
     * Our host expects ARGB with A=0xFF. Set alpha on each pixel. */
    uint32_t *pixels = (uint32_t *)DG_ScreenBuffer;
    int count = DOOMGENERIC_RESX * DOOMGENERIC_RESY;
    for (int i = 0; i < count; i++) {
        pixels[i] |= 0xFF000000;
    }
    host_present_frame(
        (uint32_t)(uintptr_t)DG_ScreenBuffer,
        DOOMGENERIC_RESX,
        DOOMGENERIC_RESY,
        DOOMGENERIC_RESX * 4
    );
}

void DG_SleepMs(uint32_t ms) {
    /* PolkaVM is single-threaded — we can't really sleep.
     * Just return immediately; the host controls frame pacing. */
    (void)ms;
}

uint32_t DG_GetTicksMs(void) {
    return (uint32_t)host_time_ms();
}

int DG_GetKey(int *pressed, unsigned char *doom_key) {
    /* First drain any new input from the host. */
    poll_host_input();

    if (key_count == 0) return 0;

    *pressed = key_ring[key_head].pressed;
    *doom_key = key_ring[key_head].key;
    key_head = (key_head + 1) % MAX_KEYS;
    key_count--;
    return 1;
}

void DG_SetWindowTitle(const char *title) {
    (void)title; /* Host manages the tab title. */
}

void DG_Close(void) {
    /* Nothing to clean up. */
}
