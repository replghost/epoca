#ifndef _STDIO_H
#define _STDIO_H

#include <stddef.h>
#include <stdarg.h>

/* FILE is an opaque type. The actual struct is defined in fileio_shim.c.
 * Callers always use FILE * so the incomplete type is sufficient. */
typedef struct _FILE FILE;

/* Standard stream handles — defined in libc_shim.c as void* aliases */
extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

/* Seek whence constants */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* End-of-file indicator */
#define EOF (-1)

/* File operations — implemented in fileio_shim.c */
FILE  *fopen(const char *path, const char *mode);
int    fclose(FILE *f);
size_t fread(void *ptr, size_t elem_size, size_t count, FILE *f);
size_t fwrite(const void *ptr, size_t size, size_t count, FILE *f);
int    fseek(FILE *f, long offset, int whence);
long   ftell(FILE *f);
int    feof(FILE *f);
char  *fgets(char *buf, int n, FILE *f);
int    fflush(FILE *stream);

/* File system operations — stubs in fileio_shim.c */
int remove(const char *path);
int rename(const char *oldpath, const char *newpath);

/* Formatted output — implemented in libc_shim.c */
int printf(const char *fmt, ...);
int fprintf(FILE *stream, const char *fmt, ...);
int sprintf(char *buf, const char *fmt, ...);
int snprintf(char *buf, size_t size, const char *fmt, ...);
int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int vfprintf(FILE *stream, const char *fmt, va_list ap);
int vsprintf(char *buf, const char *fmt, va_list ap);

/* Formatted input — implemented in libc_shim.c */
int sscanf(const char *str, const char *fmt, ...);

/* Character output — implemented in libc_shim.c */
int puts(const char *s);
int putchar(int c);

#endif /* _STDIO_H */
