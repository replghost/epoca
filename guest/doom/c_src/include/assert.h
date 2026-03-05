#ifndef _ASSERT_H
#define _ASSERT_H

/* assert() — in a freestanding PolkaVM guest, failed assertions trap.
 * NDEBUG disables assertions at compile time (standard behavior). */

#ifdef NDEBUG
#  define assert(expr) ((void)0)
#else
#  define assert(expr) \
    ((expr) ? (void)0 : __builtin_trap())
#endif

#endif /* _ASSERT_H */
