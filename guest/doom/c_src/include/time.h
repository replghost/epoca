#ifndef _TIME_H
#define _TIME_H

/* time_t — calendar time as seconds since epoch (long on riscv32) */
typedef long time_t;

/* clock_t — processor time (ticks) */
typedef long clock_t;

/* CLOCKS_PER_SEC — our stub clock() always returns 0, so this is nominal */
#define CLOCKS_PER_SEC 1000000L

/* Time functions — implemented in libc_shim.c (stubs returning 0) */
time_t  time(time_t *t);
clock_t clock(void);

#endif /* _TIME_H */
