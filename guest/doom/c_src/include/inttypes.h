#ifndef _INTTYPES_H
#define _INTTYPES_H

/* inttypes.h — pull in stdint.h and add printf/scanf format macros.
 * doom's doomtype.h includes this as a fallback for stdint.h on old systems. */

#include <stdint.h>

/* printf format specifiers for exact-width types (32-bit riscv) */
#define PRId8   "d"
#define PRId16  "d"
#define PRId32  "d"
#define PRId64  "lld"

#define PRIu8   "u"
#define PRIu16  "u"
#define PRIu32  "u"
#define PRIu64  "llu"

#define PRIx8   "x"
#define PRIx16  "x"
#define PRIx32  "x"
#define PRIx64  "llx"

#define PRIX8   "X"
#define PRIX16  "X"
#define PRIX32  "X"
#define PRIX64  "llX"

/* scanf format specifiers */
#define SCNd8   "hhd"
#define SCNd16  "hd"
#define SCNd32  "d"
#define SCNd64  "lld"

#define SCNu8   "hhu"
#define SCNu16  "hu"
#define SCNu32  "u"
#define SCNu64  "llu"

#endif /* _INTTYPES_H */
