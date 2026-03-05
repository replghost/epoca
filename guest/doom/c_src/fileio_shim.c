/*
 * fileio_shim.c — File I/O backed by host_asset_read for PolkaVM sandbox.
 *
 * Provides fopen/fclose/fread/fwrite/fseek/ftell/feof and the w_file WAD
 * interface. All file access goes through the host's asset store.
 *
 * Strategy: on fopen, load the entire asset into a malloc'd buffer.
 * Then fread/fseek/ftell operate on that buffer.
 */

#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>

/* Host imports (linked via Rust #[no_mangle] wrappers) */
extern uint32_t host_asset_read_wrapper(uint32_t name_ptr, uint32_t name_len,
                                        uint32_t offset, uint32_t dst_ptr, uint32_t max_len);

/* libc shim functions we provide */
extern void *malloc(size_t);
extern void free(void *);

/* ── Concrete definition of FILE (declared as struct _FILE in stdio.h) ── */

#define MAX_OPEN_FILES 16

struct _FILE {
    int in_use;
    unsigned char *data;
    size_t size;
    size_t pos;
    int eof_flag;
    char asset_name[64]; /* cached asset name for reuse */
};

static struct _FILE file_table[MAX_OPEN_FILES];

/* Extract just the filename from a path (strip directories). */
static const char *base_name(const char *path) {
    const char *slash;
    if (!path) return "";
    slash = strrchr(path, '/');
    if (slash) return slash + 1;
    slash = strrchr(path, '\\');
    if (slash) return slash + 1;
    return path;
}

/* Check if we already have this asset loaded (closed but data still valid). */
static struct _FILE *find_cached(const char *name) {
    for (int i = 0; i < MAX_OPEN_FILES; i++) {
        if (!file_table[i].in_use && file_table[i].data != NULL
            && strcmp(file_table[i].asset_name, name) == 0) {
            return &file_table[i];
        }
    }
    return NULL;
}

FILE *fopen(const char *path, const char *mode) {
    (void)mode; /* We only support reading. */

    /* Reject NULL or empty paths immediately. */
    if (!path || path[0] == '\0') return NULL;

    /* Find the asset name — try basename. */
    const char *name = base_name(path);
    size_t name_len = strlen(name);
    if (name_len == 0) return NULL;

    /* Reuse cached data from a previous open/close of the same asset. */
    struct _FILE *cached = find_cached(name);
    if (cached) {
        cached->in_use = 1;
        cached->pos = 0;
        cached->eof_flag = 0;
        printf("fopen: %s (cached, %u bytes)\n", name, (unsigned)cached->size);
        return cached;
    }

    #define PROBE_CHUNK (64 * 1024)
    #define MAX_FILE_SIZE (32 * 1024 * 1024)

    /* Check if asset exists. */
    uint8_t probe;
    uint32_t got = host_asset_read_wrapper((uint32_t)(uintptr_t)name, name_len,
                                   0, (uint32_t)(uintptr_t)&probe, 1);
    if (got == 0) {
        printf("fopen: asset not found: %s\n", name);
        return NULL;
    }

    /* Binary search for file size. */
    size_t lo = 1, hi = MAX_FILE_SIZE;
    while (lo < hi) {
        size_t mid = lo + (hi - lo + 1) / 2;
        got = host_asset_read_wrapper((uint32_t)(uintptr_t)name, name_len,
                              (uint32_t)(mid - 1), (uint32_t)(uintptr_t)&probe, 1);
        if (got > 0) lo = mid;
        else hi = mid - 1;
    }
    size_t file_size = lo;

    /* Allocate and read the entire file. */
    unsigned char *data = (unsigned char *)malloc(file_size);
    if (!data) {
        printf("fopen: malloc failed for %u bytes\n", (unsigned)file_size);
        return NULL;
    }

    size_t offset = 0;
    while (offset < file_size) {
        size_t chunk = file_size - offset;
        if (chunk > PROBE_CHUNK) chunk = PROBE_CHUNK;
        got = host_asset_read_wrapper((uint32_t)(uintptr_t)name, name_len,
                              (uint32_t)offset,
                              (uint32_t)(uintptr_t)(data + offset),
                              (uint32_t)chunk);
        if (got == 0) break;
        offset += got;
    }

    /* Find a free file slot. */
    for (int i = 0; i < MAX_OPEN_FILES; i++) {
        if (!file_table[i].in_use && file_table[i].data == NULL) {
            file_table[i].in_use = 1;
            file_table[i].data = data;
            file_table[i].size = offset;
            file_table[i].pos = 0;
            file_table[i].eof_flag = 0;
            /* Cache the asset name for reuse after fclose. */
            strncpy(file_table[i].asset_name, name, sizeof(file_table[i].asset_name) - 1);
            file_table[i].asset_name[sizeof(file_table[i].asset_name) - 1] = '\0';
            printf("fopen: %s (%u bytes)\n", name, (unsigned)offset);
            return &file_table[i];
        }
    }

    free(data);
    printf("fopen: too many open files\n");
    return NULL;
}

int fclose(FILE *f) {
    if (!f || !f->in_use) return -1;
    f->in_use = 0;
    /* Keep f->data and f->asset_name for cache reuse on next fopen. */
    f->pos = 0;
    f->eof_flag = 0;
    return 0;
}

size_t fread(void *ptr, size_t elem_size, size_t count, FILE *f) {
    if (!f || !f->in_use) return 0;
    size_t total = elem_size * count;
    size_t avail = (f->pos < f->size) ? f->size - f->pos : 0;
    if (total > avail) {
        total = avail;
        f->eof_flag = 1;
    }
    memcpy(ptr, f->data + f->pos, total);
    f->pos += total;
    return total / elem_size;
}

size_t fwrite(const void *ptr, size_t size, size_t count, FILE *f) {
    (void)ptr; (void)size; (void)count; (void)f;
    return 0; /* Read-only filesystem. */
}

int fseek(FILE *f, long offset, int whence) {
    if (!f || !f->in_use) return -1;
    long new_pos;
    switch (whence) {
        case SEEK_SET: new_pos = offset; break;
        case SEEK_CUR: new_pos = (long)f->pos + offset; break;
        case SEEK_END: new_pos = (long)f->size + offset; break;
        default: return -1;
    }
    if (new_pos < 0) new_pos = 0;
    if ((size_t)new_pos > f->size) new_pos = (long)f->size;
    f->pos = (size_t)new_pos;
    f->eof_flag = 0;
    return 0;
}

long ftell(FILE *f) {
    if (!f || !f->in_use) return -1;
    return (long)f->pos;
}

int feof(FILE *f) {
    if (!f || !f->in_use) return 1;
    return f->eof_flag;
}

char *fgets(char *buf, int n, FILE *f) {
    if (!f || !f->in_use || n <= 0) return NULL;
    int i = 0;
    while (i < n - 1 && f->pos < f->size) {
        char c = (char)f->data[f->pos++];
        buf[i++] = c;
        if (c == '\n') break;
    }
    if (i == 0) { f->eof_flag = 1; return NULL; }
    buf[i] = '\0';
    return buf;
}

/* ── Filesystem stubs doom needs ───────────────────────────────── */

int remove(const char *path) { (void)path; return -1; }
int rename(const char *old, const char *new_name) { (void)old; (void)new_name; return -1; }
int access(const char *path, int mode) { (void)path; (void)mode; return -1; }
int mkdir(const char *path, unsigned int mode) { (void)path; (void)mode; return -1; }
int stat(const char *path, void *buf) { (void)path; (void)buf; return -1; }
int open(const char *path, int flags, ...) { (void)path; (void)flags; return -1; }
int close(int fd) { (void)fd; return -1; }
int read(int fd, void *buf, size_t count) { (void)fd; (void)buf; (void)count; return -1; }
int write(int fd, const void *buf, size_t count) { (void)fd; (void)buf; (void)count; return -1; }
long lseek(int fd, long offset, int whence) { (void)fd; (void)offset; (void)whence; return -1; }

/* ══════════════════════════════════════════════════════════════════
 * WAD file interface — implements w_file.h for doomgeneric
 *
 * This replaces w_file_stdc.c.
 * ══════════════════════════════════════════════════════════════════ */

#include "w_file.h"

typedef struct {
    wad_file_t wad;
    FILE *stream;
} stdc_wad_file_t;

/* Forward declaration */
wad_file_class_t stdc_wad_file;

static wad_file_t *W_Shim_OpenFile(char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;

    stdc_wad_file_t *result = malloc(sizeof(stdc_wad_file_t));
    if (!result) { fclose(f); return NULL; }

    result->wad.file_class = &stdc_wad_file;
    result->wad.mapped = NULL;
    result->wad.length = f->size;
    result->stream = f;

    return &result->wad;
}

static void W_Shim_CloseFile(wad_file_t *wad) {
    stdc_wad_file_t *sf = (stdc_wad_file_t *)wad;
    fclose(sf->stream);
}

static size_t W_Shim_Read(wad_file_t *wad, unsigned int offset,
                          void *buffer, size_t buffer_len) {
    stdc_wad_file_t *sf = (stdc_wad_file_t *)wad;
    fseek(sf->stream, (long)offset, SEEK_SET);
    return fread(buffer, 1, buffer_len, sf->stream);
}

wad_file_class_t stdc_wad_file = {
    W_Shim_OpenFile,
    W_Shim_CloseFile,
    W_Shim_Read,
};
