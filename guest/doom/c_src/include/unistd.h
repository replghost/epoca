#ifndef _UNISTD_H
#define _UNISTD_H

#include <stddef.h>

/* access() mode flags */
#define F_OK 0  /* Test existence */
#define R_OK 4  /* Test read permission */
#define W_OK 2  /* Test write permission */
#define X_OK 1  /* Test execute permission */

/* Standard file descriptors */
#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

/* Offset whence values (also in stdio.h) */
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif

/* POSIX I/O — implemented as stubs in fileio_shim.c */
int   close(int fd);
int   read(int fd, void *buf, size_t count);
int   write(int fd, const void *buf, size_t count);
long  lseek(int fd, long offset, int whence);
int   access(const char *path, int mode);

/* Timing stubs — implemented in libc_shim.c */
unsigned int sleep(unsigned int seconds);
int          usleep(unsigned int usec);

#endif /* _UNISTD_H */
