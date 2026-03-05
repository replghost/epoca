#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <stddef.h>

/* Minimal stat structure — only the fields doom actually references */
struct stat {
    unsigned int  st_mode;   /* File mode and type */
    unsigned int  st_size;   /* Total size in bytes */
    long          st_mtime;  /* Time of last modification */
};

/* File mode bits */
#define S_IFMT   0170000  /* Bitmask for the file type bitfields */
#define S_IFREG  0100000  /* Regular file */
#define S_IFDIR  0040000  /* Directory */
#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)
#define S_ISDIR(m) (((m) & S_IFMT) == S_IFDIR)

/* Permission bits */
#define S_IRWXU  0000700  /* Owner: read, write, execute */
#define S_IRUSR  0000400  /* Owner: read */
#define S_IWUSR  0000200  /* Owner: write */
#define S_IXUSR  0000100  /* Owner: execute */
#define S_IRWXG  0000070  /* Group: read, write, execute */
#define S_IRWXO  0000007  /* Others: read, write, execute */

/* stat() and mkdir() — stubs in fileio_shim.c; return -1 */
int stat(const char *path, struct stat *buf);
int mkdir(const char *path, unsigned int mode);

#endif /* _SYS_STAT_H */
