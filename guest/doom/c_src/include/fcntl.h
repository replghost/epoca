#ifndef _FCNTL_H
#define _FCNTL_H

/* File access mode flags for open() */
#define O_RDONLY   0x0000  /* Open for reading only */
#define O_WRONLY   0x0001  /* Open for writing only */
#define O_RDWR     0x0002  /* Open for reading and writing */

/* File creation flags for open() */
#define O_CREAT    0x0040  /* Create file if it does not exist */
#define O_TRUNC    0x0200  /* Truncate file to zero length */
#define O_APPEND   0x0400  /* Writes append to end of file */
#define O_EXCL     0x0080  /* Error if O_CREAT and file exists */

/* open() — stub in fileio_shim.c; always returns -1 (no fd-based I/O) */
int open(const char *path, int flags, ...);

#endif /* _FCNTL_H */
