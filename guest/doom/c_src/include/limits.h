#ifndef _LIMITS_H
#define _LIMITS_H

/* Number of bits in a char */
#define CHAR_BIT    8

/* char limits — assume signed char on riscv32 */
#define CHAR_MIN    (-128)
#define CHAR_MAX    127
#define UCHAR_MAX   255U
#define SCHAR_MIN   (-128)
#define SCHAR_MAX   127

/* short limits (16-bit) */
#define SHRT_MIN    (-32768)
#define SHRT_MAX    32767
#define USHRT_MAX   65535U

/* int limits (32-bit on riscv32) */
#define INT_MIN     (-2147483648)
#define INT_MAX     2147483647
#define UINT_MAX    4294967295U

/* long limits (32-bit on riscv32) */
#define LONG_MIN    (-2147483648L)
#define LONG_MAX    2147483647L
#define ULONG_MAX   4294967295UL

/* long long limits (64-bit) */
#define LLONG_MIN   (-9223372036854775807LL - 1)
#define LLONG_MAX   9223372036854775807LL
#define ULLONG_MAX  18446744073709551615ULL

/* Maximum bytes in a path — doom uses this for savegame paths */
#define PATH_MAX    4096

#endif /* _LIMITS_H */
