#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#include <stddef.h>

/* POSIX type aliases — minimal set for freestanding riscv32 */
typedef int            pid_t;
typedef unsigned int   uid_t;
typedef unsigned int   gid_t;
typedef unsigned int   mode_t;
typedef long           off_t;
typedef int            ssize_t;
typedef unsigned int   ino_t;
typedef unsigned int   dev_t;
typedef unsigned int   nlink_t;

#endif /* _SYS_TYPES_H */
