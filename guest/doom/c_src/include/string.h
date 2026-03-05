#ifndef _STRING_H
#define _STRING_H

#include <stddef.h>

/* Memory operations — implemented in libc_shim.c */
void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int   memcmp(const void *a, const void *b, size_t n);

/* String length — implemented in libc_shim.c */
size_t strlen(const char *s);

/* String comparison — implemented in libc_shim.c */
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
int strcasecmp(const char *a, const char *b);
int strncasecmp(const char *a, const char *b, size_t n);

/* String copy and concatenation — implemented in libc_shim.c */
char *strcpy(char *dst, const char *src);
char *strncpy(char *dst, const char *src, size_t n);
char *strcat(char *dst, const char *src);
char *strncat(char *dst, const char *src, size_t n);

/* String search — implemented in libc_shim.c */
char *strchr(const char *s, int c);
char *strrchr(const char *s, int c);
char *strstr(const char *haystack, const char *needle);

/* String duplication — implemented in libc_shim.c (uses malloc) */
char *strdup(const char *s);

#endif /* _STRING_H */
