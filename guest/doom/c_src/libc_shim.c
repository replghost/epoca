/*
 * libc_shim.c — Minimal libc for doomgeneric running on PolkaVM.
 *
 * Provides: memory allocation (bump allocator), string/memory functions,
 * printf family (routed to host_log), and various stubs.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>

/* ── Host function imports (linked via Rust #[no_mangle] wrappers) ── */
extern void host_log_wrapper(const char *ptr, unsigned int len);

/* Forward declarations for functions used before definition */
void *memcpy(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
void  free(void *ptr);
size_t strlen(const char *s);

/* ══════════════════════════════════════════════════════════════════
 * Memory allocator — simple bump with free-list for reuse
 * ══════════════════════════════════════════════════════════════════ */

#define HEAP_SIZE (32 * 1024 * 1024)  /* 32 MiB arena */

static unsigned char heap[HEAP_SIZE] __attribute__((aligned(16)));
static size_t heap_offset = 0;

/* Each allocation is prefixed with its size for realloc/free tracking. */
typedef struct {
    size_t size;
    unsigned char data[];
} alloc_header_t;

#define HEADER_SIZE ((sizeof(alloc_header_t) + 15) & ~15)

void *malloc(size_t size) {
    if (size == 0) return NULL;
    size_t total = HEADER_SIZE + ((size + 15) & ~15);
    if (heap_offset + total > HEAP_SIZE) return NULL;
    alloc_header_t *hdr = (alloc_header_t *)(heap + heap_offset);
    hdr->size = size;
    heap_offset += total;
    return hdr->data;
}

void *calloc(size_t count, size_t size) {
    size_t total = count * size;
    void *p = malloc(total);
    if (p) memset(p, 0, total);
    return p;
}

void *realloc(void *ptr, size_t new_size) {
    if (!ptr) return malloc(new_size);
    if (new_size == 0) { free(ptr); return NULL; }
    alloc_header_t *hdr = (alloc_header_t *)((unsigned char *)ptr - HEADER_SIZE);
    size_t old_size = hdr->size;
    void *new_ptr = malloc(new_size);
    if (!new_ptr) return NULL;
    size_t copy_size = old_size < new_size ? old_size : new_size;
    memcpy(new_ptr, ptr, copy_size);
    return new_ptr;
}

void free(void *ptr) {
    /* Bump allocator — free is a no-op. */
    (void)ptr;
}

/* ══════════════════════════════════════════════════════════════════
 * Memory functions
 * ══════════════════════════════════════════════════════════════════ */

void *memcpy(void *dst, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    for (size_t i = 0; i < n; i++) d[i] = s[i];
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    if (d < s) {
        for (size_t i = 0; i < n; i++) d[i] = s[i];
    } else {
        for (size_t i = n; i > 0; i--) d[i - 1] = s[i - 1];
    }
    return dst;
}

void *memset(void *dst, int c, size_t n) {
    unsigned char *d = (unsigned char *)dst;
    for (size_t i = 0; i < n; i++) d[i] = (unsigned char)c;
    return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *pa = (const unsigned char *)a;
    const unsigned char *pb = (const unsigned char *)b;
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    for (size_t i = 0; i < n; i++) {
        if (pa[i] != pb[i]) return pa[i] - pb[i];
    }
    return 0;
}

/* ══════════════════════════════════════════════════════════════════
 * String functions
 * ══════════════════════════════════════════════════════════════════ */

size_t strlen(const char *s) {
    if (!s) return 0;
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

char *strcpy(char *dst, const char *src) {
    char *d = dst;
    if (!dst) return dst;
    if (!src) { *d = '\0'; return dst; }
    while ((*d++ = *src++));
    return dst;
}

char *strncpy(char *dst, const char *src, size_t n) {
    size_t i;
    if (!dst) return dst;
    if (!src) { if (n) dst[0] = '\0'; return dst; }
    for (i = 0; i < n && src[i]; i++) dst[i] = src[i];
    for (; i < n; i++) dst[i] = '\0';
    return dst;
}

char *strcat(char *dst, const char *src) {
    char *d;
    if (!dst) return dst;
    if (!src) return dst;
    d = dst + strlen(dst);
    while ((*d++ = *src++));
    return dst;
}

char *strncat(char *dst, const char *src, size_t n) {
    char *d;
    size_t i;
    if (!dst) return dst;
    if (!src) return dst;
    d = dst + strlen(dst);
    for (i = 0; i < n && src[i]; i++) d[i] = src[i];
    d[i] = '\0';
    return dst;
}

int strcmp(const char *a, const char *b) {
    if (!a) return b ? -1 : 0;
    if (!b) return 1;
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    if (!a) return b ? -1 : 0;
    if (!b) return 1;
    for (size_t i = 0; i < n; i++) {
        if (a[i] != b[i]) return (unsigned char)a[i] - (unsigned char)b[i];
        if (a[i] == '\0') return 0;
    }
    return 0;
}

static int to_lower(int c) {
    return (c >= 'A' && c <= 'Z') ? c + 32 : c;
}

int strcasecmp(const char *a, const char *b) {
    if (!a) return b ? -1 : 0;
    if (!b) return 1;
    while (*a && to_lower(*a) == to_lower(*b)) { a++; b++; }
    return to_lower((unsigned char)*a) - to_lower((unsigned char)*b);
}

int strncasecmp(const char *a, const char *b, size_t n) {
    if (!a) return b ? -1 : 0;
    if (!b) return 1;
    for (size_t i = 0; i < n; i++) {
        int la = to_lower((unsigned char)a[i]);
        int lb = to_lower((unsigned char)b[i]);
        if (la != lb) return la - lb;
        if (a[i] == '\0') return 0;
    }
    return 0;
}

char *strchr(const char *s, int c) {
    if (!s) return NULL;
    while (*s) {
        if (*s == (char)c) return (char *)s;
        s++;
    }
    return (c == 0) ? (char *)s : NULL;
}

char *strrchr(const char *s, int c) {
    const char *last = NULL;
    if (!s) return NULL;
    while (*s) {
        if (*s == (char)c) last = s;
        s++;
    }
    if (c == 0) return (char *)s;
    return (char *)last;
}

char *strstr(const char *haystack, const char *needle) {
    size_t nlen;
    if (!haystack) return NULL;
    if (!needle) return (char *)haystack;
    nlen = strlen(needle);
    if (nlen == 0) return (char *)haystack;
    while (*haystack) {
        if (strncmp(haystack, needle, nlen) == 0) return (char *)haystack;
        haystack++;
    }
    return NULL;
}

char *strdup(const char *s) {
    size_t len;
    char *d;
    if (!s) return NULL;
    len = strlen(s) + 1;
    d = malloc(len);
    if (d) memcpy(d, s, len);
    return d;
}

/* ══════════════════════════════════════════════════════════════════
 * Character classification
 * ══════════════════════════════════════════════════════════════════ */

int toupper(int c) { return (c >= 'a' && c <= 'z') ? c - 32 : c; }
int tolower(int c) { return (c >= 'A' && c <= 'Z') ? c + 32 : c; }
int isdigit(int c) { return c >= '0' && c <= '9'; }
int isspace(int c) { return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v'; }
int isalpha(int c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'); }
int isalnum(int c) { return isalpha(c) || isdigit(c); }
int isprint(int c) { return c >= 0x20 && c <= 0x7e; }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int isxdigit(int c) { return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }

/* ══════════════════════════════════════════════════════════════════
 * Number conversion
 * ══════════════════════════════════════════════════════════════════ */

int atoi(const char *s) {
    int n = 0, neg = 0;
    if (!s) return 0;
    while (isspace(*s)) s++;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    while (isdigit(*s)) { n = n * 10 + (*s++ - '0'); }
    return neg ? -n : n;
}

long atol(const char *s) { return (long)atoi(s); }

double atof(const char *s) {
    double result = 0.0, fraction = 0.0;
    int neg = 0, has_dot = 0;
    double divisor = 1.0;
    if (!s) return 0.0;
    while (isspace(*s)) s++;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    while (*s) {
        if (*s == '.' && !has_dot) { has_dot = 1; s++; continue; }
        if (*s < '0' || *s > '9') break;
        if (has_dot) { divisor *= 10.0; fraction += (*s - '0') / divisor; }
        else { result = result * 10.0 + (*s - '0'); }
        s++;
    }
    result += fraction;
    return neg ? -result : result;
}

long strtol(const char *s, char **end, int base) {
    long n = 0;
    int neg = 0;
    if (!s) { if (end) *end = (char *)s; return 0; }
    while (isspace(*s)) s++;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    if (base == 0) {
        if (*s == '0' && (s[1] == 'x' || s[1] == 'X')) { base = 16; s += 2; }
        else if (*s == '0') { base = 8; s++; }
        else base = 10;
    } else if (base == 16 && *s == '0' && (s[1] == 'x' || s[1] == 'X')) {
        s += 2;
    }
    while (*s) {
        int d;
        if (*s >= '0' && *s <= '9') d = *s - '0';
        else if (*s >= 'a' && *s <= 'z') d = *s - 'a' + 10;
        else if (*s >= 'A' && *s <= 'Z') d = *s - 'A' + 10;
        else break;
        if (d >= base) break;
        n = n * base + d;
        s++;
    }
    if (end) *end = (char *)s;
    return neg ? -n : n;
}

unsigned long strtoul(const char *s, char **end, int base) {
    return (unsigned long)strtol(s, end, base);
}

/* ══════════════════════════════════════════════════════════════════
 * Printf family → host_log
 * ══════════════════════════════════════════════════════════════════ */

/* Minimal vsnprintf — supports %d, %u, %x, %X, %s, %c, %p, %%, field widths, zero-pad, '-'. */

static int fmt_int(char *buf, size_t sz, unsigned long val, int base, int upper, int is_neg,
                   int width, int zero_pad, int left_align) {
    char tmp[24];
    int len = 0;
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    if (val == 0) { tmp[len++] = '0'; }
    else { while (val) { tmp[len++] = digits[val % base]; val /= base; } }
    int total = len + (is_neg ? 1 : 0);
    int pad = (width > total) ? width - total : 0;
    int pos = 0;

    if (!left_align && !zero_pad)
        for (int i = 0; i < pad && (size_t)pos < sz - 1; i++) buf[pos++] = ' ';
    if (is_neg && (size_t)pos < sz - 1) buf[pos++] = '-';
    if (!left_align && zero_pad)
        for (int i = 0; i < pad && (size_t)pos < sz - 1; i++) buf[pos++] = '0';
    for (int i = len - 1; i >= 0 && (size_t)pos < sz - 1; i--) buf[pos++] = tmp[i];
    if (left_align)
        for (int i = 0; i < pad && (size_t)pos < sz - 1; i++) buf[pos++] = ' ';
    return pos;
}

int vsnprintf(char *buf, size_t sz, const char *fmt, va_list ap) {
    size_t pos = 0;
    if (sz == 0) return 0;
    if (!buf) return 0;
    if (!fmt) { buf[0] = '\0'; return 0; }
    sz--; /* reserve for NUL */

    while (*fmt && pos < sz) {
        if (*fmt != '%') { buf[pos++] = *fmt++; continue; }
        fmt++; /* skip '%' */

        /* Flags */
        int zero_pad = 0, left_align = 0;
        while (*fmt == '0' || *fmt == '-') {
            if (*fmt == '0') zero_pad = 1;
            if (*fmt == '-') left_align = 1;
            fmt++;
        }
        if (left_align) zero_pad = 0;

        /* Width */
        int width = 0;
        if (*fmt == '*') { width = va_arg(ap, int); fmt++; }
        else { while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (*fmt++ - '0'); } }

        /* Precision (consume but mostly ignore) */
        int precision = -1;
        if (*fmt == '.') {
            fmt++;
            precision = 0;
            if (*fmt == '*') { precision = va_arg(ap, int); fmt++; }
            else { while (*fmt >= '0' && *fmt <= '9') { precision = precision * 10 + (*fmt++ - '0'); } }
        }

        /* Length modifier */
        int is_long = 0;
        if (*fmt == 'l') { is_long = 1; fmt++; if (*fmt == 'l') { fmt++; } }
        else if (*fmt == 'h') { fmt++; if (*fmt == 'h') fmt++; }
        else if (*fmt == 'z') { is_long = 1; fmt++; }

        /* For integer formats, precision means minimum digits (zero-padded).
         * E.g. %.3d with value 33 → "033". */
        int int_width = width;
        int int_zero = zero_pad;
        if (precision >= 0 && (*fmt == 'd' || *fmt == 'i' || *fmt == 'u' ||
                               *fmt == 'x' || *fmt == 'X' || *fmt == 'o')) {
            int_width = precision;
            int_zero = 1;
        }

        switch (*fmt) {
            case 'd': case 'i': {
                long val = is_long ? va_arg(ap, long) : (long)va_arg(ap, int);
                int neg = val < 0;
                unsigned long uval = neg ? (unsigned long)(-val) : (unsigned long)val;
                pos += fmt_int(buf + pos, sz - pos + 1, uval, 10, 0, neg, int_width, int_zero, left_align);
                break;
            }
            case 'u': {
                unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int);
                pos += fmt_int(buf + pos, sz - pos + 1, val, 10, 0, 0, int_width, int_zero, left_align);
                break;
            }
            case 'x': {
                unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int);
                pos += fmt_int(buf + pos, sz - pos + 1, val, 16, 0, 0, int_width, int_zero, left_align);
                break;
            }
            case 'X': {
                unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int);
                pos += fmt_int(buf + pos, sz - pos + 1, val, 16, 1, 0, int_width, int_zero, left_align);
                break;
            }
            case 'o': {
                unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int);
                pos += fmt_int(buf + pos, sz - pos + 1, val, 8, 0, 0, int_width, int_zero, left_align);
                break;
            }
            case 'p': {
                unsigned long val = (unsigned long)(uintptr_t)va_arg(ap, void *);
                if (pos + 2 < sz) { buf[pos++] = '0'; buf[pos++] = 'x'; }
                pos += fmt_int(buf + pos, sz - pos + 1, val, 16, 0, 0, 0, 0, 0);
                break;
            }
            case 's': {
                const char *s = va_arg(ap, const char *);
                if (!s) s = "(null)";
                size_t slen = strlen(s);
                if (precision >= 0 && (size_t)precision < slen) slen = precision;
                int pad = (width > (int)slen) ? width - (int)slen : 0;
                if (!left_align) for (int i = 0; i < pad && pos < sz; i++) buf[pos++] = ' ';
                for (size_t i = 0; i < slen && pos < sz; i++) buf[pos++] = s[i];
                if (left_align) for (int i = 0; i < pad && pos < sz; i++) buf[pos++] = ' ';
                break;
            }
            case 'c': {
                char c = (char)va_arg(ap, int);
                buf[pos++] = c;
                break;
            }
            case '%':
                buf[pos++] = '%';
                break;
            default:
                /* Unknown specifier — just emit it */
                buf[pos++] = '%';
                if (pos < sz) buf[pos++] = *fmt;
                break;
        }
        if (*fmt) fmt++;
    }
    buf[pos] = '\0';
    return (int)pos;
}

int snprintf(char *buf, size_t sz, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sz, fmt, ap);
    va_end(ap);
    return n;
}

int sprintf(char *buf, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, 4096, fmt, ap);
    va_end(ap);
    return n;
}

int vsprintf(char *buf, const char *fmt, va_list ap) {
    return vsnprintf(buf, 4096, fmt, ap);
}

static char log_buf[4096];

int printf(const char *fmt, ...) {
    va_list ap;
    int n;
    if (!fmt) return 0;
    va_start(ap, fmt);
    n = vsnprintf(log_buf, sizeof(log_buf), fmt, ap);
    va_end(ap);
    if (n > 0) host_log_wrapper(log_buf, n);
    return n;
}

int vfprintf(struct _FILE *stream, const char *fmt, va_list ap) {
    int n;
    (void)stream;
    if (!fmt) return 0;
    n = vsnprintf(log_buf, sizeof(log_buf), fmt, ap);
    if (n > 0) host_log_wrapper(log_buf, n);
    return n;
}

int fprintf(struct _FILE *stream, const char *fmt, ...) {
    (void)stream;
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf(stream, fmt, ap);
    va_end(ap);
    return n;
}

int puts(const char *s) {
    int len;
    if (!s) s = "(null)";
    len = strlen(s);
    host_log_wrapper(s, len);
    return len;
}

int putchar(int c) {
    char ch = (char)c;
    host_log_wrapper(&ch, 1);
    return c;
}

int fflush(struct _FILE *stream) { (void)stream; return 0; }

/* ══════════════════════════════════════════════════════════════════
 * sscanf — minimal implementation for doom's needs (%d, %s, %x)
 * ══════════════════════════════════════════════════════════════════ */

int sscanf(const char *str, const char *fmt, ...) {
    va_list ap;
    int matched = 0;
    if (!str || !fmt) return 0;
    va_start(ap, fmt);
    while (*fmt && *str) {
        if (*fmt == '%') {
            fmt++;
            if (*fmt == 'd' || *fmt == 'i') {
                int *out = va_arg(ap, int *);
                int neg = 0, val = 0;
                while (isspace(*str)) str++;
                if (*str == '-') { neg = 1; str++; }
                if (!isdigit(*str)) break;
                while (isdigit(*str)) { val = val * 10 + (*str++ - '0'); }
                *out = neg ? -val : val;
                matched++;
                fmt++;
            } else if (*fmt == 'x' || *fmt == 'X') {
                unsigned int *out = va_arg(ap, unsigned int *);
                unsigned int val = 0;
                while (isspace(*str)) str++;
                while (1) {
                    if (*str >= '0' && *str <= '9') val = val * 16 + (*str - '0');
                    else if (*str >= 'a' && *str <= 'f') val = val * 16 + (*str - 'a' + 10);
                    else if (*str >= 'A' && *str <= 'F') val = val * 16 + (*str - 'A' + 10);
                    else break;
                    str++;
                }
                *out = val;
                matched++;
                fmt++;
            } else if (*fmt == 's') {
                char *out = va_arg(ap, char *);
                while (isspace(*str)) str++;
                while (*str && !isspace(*str)) *out++ = *str++;
                *out = '\0';
                matched++;
                fmt++;
            } else {
                break;
            }
        } else if (isspace(*fmt)) {
            while (isspace(*str)) str++;
            while (isspace(*fmt)) fmt++;
        } else {
            if (*str != *fmt) break;
            str++; fmt++;
        }
    }
    va_end(ap);
    return matched;
}

/* ══════════════════════════════════════════════════════════════════
 * qsort
 * ══════════════════════════════════════════════════════════════════ */

/* Simple insertion sort — fine for the small arrays doom uses. */
void qsort(void *base, size_t nmemb, size_t size,
           int (*compar)(const void *, const void *)) {
    unsigned char *arr = (unsigned char *)base;
    unsigned char tmp[256]; /* doom never qsorts elements > 256 bytes */
    for (size_t i = 1; i < nmemb; i++) {
        memcpy(tmp, arr + i * size, size);
        size_t j = i;
        while (j > 0 && compar(arr + (j - 1) * size, tmp) > 0) {
            memcpy(arr + j * size, arr + (j - 1) * size, size);
            j--;
        }
        memcpy(arr + j * size, tmp, size);
    }
}

/* ══════════════════════════════════════════════════════════════════
 * Random / Time / Env / Signal stubs
 * ══════════════════════════════════════════════════════════════════ */

static unsigned int rand_state = 1;

int rand(void) {
    rand_state = rand_state * 1103515245 + 12345;
    return (rand_state >> 16) & 0x7fff;
}

void srand(unsigned int seed) { rand_state = seed; }

/* time() — doom uses it for srand seed; just return 0. */
long time(long *t) {
    if (t) *t = 0;
    return 0;
}

long clock(void) { return 0; }

char *getenv(const char *name) { (void)name; return NULL; }

/* signal / raise — just ignore */
void (*signal(int sig, void (*handler)(int)))(int) {
    (void)sig; (void)handler;
    return NULL;
}

int raise(int sig) { (void)sig; return 0; }

/* ══════════════════════════════════════════════════════════════════
 * Exit / Abort
 * ══════════════════════════════════════════════════════════════════ */

void exit(int status) {
    char msg[64];
    snprintf(msg, sizeof(msg), "exit(%d) called", status);
    host_log_wrapper(msg, strlen(msg));
    __builtin_trap();
}

void abort(void) {
    host_log_wrapper("abort() called", 14);
    __builtin_trap();
}

void _exit(int status) { exit(status); }

/* ══════════════════════════════════════════════════════════════════
 * Misc stubs doom needs
 * ══════════════════════════════════════════════════════════════════ */

unsigned int sleep(unsigned int seconds) { (void)seconds; return 0; }
int usleep(unsigned int usec) { (void)usec; return 0; }
int system(const char *command) { (void)command; return -1; }

/* errno */
int errno;
int *__errno_location(void) { return &errno; }

/* ── Standard stream handles (FILE* as declared in stdio.h) ── */
/* These are never used for actual I/O — doom just passes them to fprintf. */
struct _FILE;
struct _FILE *stderr = 0;
struct _FILE *stdout = 0;
struct _FILE *stdin = 0;
