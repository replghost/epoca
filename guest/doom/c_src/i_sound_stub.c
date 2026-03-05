/*
 * i_sound_stub.c — No-op sound/music implementation for PolkaVM.
 *
 * Replaces i_sound.c, i_sdlsound.c, i_sdlmusic.c.
 * DOOM runs fine without sound — just silent.
 */

#include "doomtype.h"

#include <stddef.h>

/* These are the symbols that s_sound.c and other modules reference. */

typedef struct { /* Opaque */ } sfxinfo_t;

static boolean I_NullSoundIsRunning(int handle) { (void)handle; return false; }

/* Sound driver interface */
int I_SDL_PrecacheSounds(sfxinfo_t *sounds, int num_sounds) {
    (void)sounds; (void)num_sounds; return 0;
}

/* ── Stubs for i_sound.c interface ── */

void I_InitSound(boolean use_sfx_prefix) { (void)use_sfx_prefix; }
void I_ShutdownSound(void) {}
int I_GetSfxLumpNum(void *sfx) { (void)sfx; return 0; }
void I_UpdateSound(void) {}
void I_UpdateSoundParams(int handle, int vol, int sep) { (void)handle; (void)vol; (void)sep; }
int I_StartSound(void *sfx, int channel, int vol, int sep) {
    (void)sfx; (void)channel; (void)vol; (void)sep; return 0;
}
void I_StopSound(int handle) { (void)handle; }
boolean I_SoundIsPlaying(int handle) { (void)handle; return false; }

/* ── Music stubs ── */

void I_InitMusic(void) {}
void I_ShutdownMusic(void) {}
void I_SetMusicVolume(int volume) { (void)volume; }
void I_PauseSong(void) {}
void I_ResumeSong(void) {}
void *I_RegisterSong(void *data, int len) { (void)data; (void)len; return NULL; }
void I_UnRegisterSong(void *handle) { (void)handle; }
void I_PlaySong(void *handle, boolean looping) { (void)handle; (void)looping; }
void I_StopSong(void) {}
boolean I_MusicIsPlaying(void) { return false; }

/* Additional symbols referenced by other modules */
void I_PrecacheSounds(void *sounds, int num_sounds) { (void)sounds; (void)num_sounds; }
void I_BindSoundVariables(void) {}

/* Global variable referenced by config */
int snd_musicdevice = 0;
