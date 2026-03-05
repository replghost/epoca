#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>

/* Exit status constants */
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* RAND_MAX — our shim uses 15-bit output (0x7fff) */
#define RAND_MAX 0x7fff

/* Memory allocation — implemented in libc_shim.c */
void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *ptr, size_t new_size);
void  free(void *ptr);

/* Process control — implemented in libc_shim.c */
__attribute__((noreturn)) void exit(int status);
__attribute__((noreturn)) void abort(void);
__attribute__((noreturn)) void _exit(int status);

/* Number conversion — implemented in libc_shim.c */
int           atoi(const char *s);
long          atol(const char *s);
long          strtol(const char *s, char **endptr, int base);
unsigned long strtoul(const char *s, char **endptr, int base);

/* Sorting — implemented in libc_shim.c */
void qsort(void *base, size_t nmemb, size_t size,
           int (*compar)(const void *, const void *));

/* Random number generation — implemented in libc_shim.c */
int  rand(void);
void srand(unsigned int seed);

/* Environment — implemented in libc_shim.c (stub returns NULL) */
char *getenv(const char *name);

/* System — stub, always returns -1 */
int system(const char *command);

/* Float conversion */
double atof(const char *s);

/* Arithmetic */
static inline int abs(int x) { return x < 0 ? -x : x; }

#endif /* _STDLIB_H */
