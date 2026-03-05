#ifndef _SIGNAL_H
#define _SIGNAL_H

/* Signal handler function pointer type */
typedef void (*sighandler_t)(int);

/* Special handler values */
#define SIG_DFL ((sighandler_t)0)  /* Default action */
#define SIG_IGN ((sighandler_t)1)  /* Ignore signal */
#define SIG_ERR ((sighandler_t)-1) /* Error return */

/* Standard signal numbers */
#define SIGHUP  1   /* Hangup */
#define SIGINT  2   /* Interactive attention signal */
#define SIGQUIT 3   /* Quit */
#define SIGILL  4   /* Illegal instruction */
#define SIGTRAP 5   /* Trace/breakpoint trap */
#define SIGABRT 6   /* Abnormal termination (abort) */
#define SIGFPE  8   /* Floating-point exception */
#define SIGKILL 9   /* Kill (cannot be caught or ignored) */
#define SIGSEGV 11  /* Segmentation violation */
#define SIGPIPE 13  /* Broken pipe */
#define SIGALRM 14  /* Alarm clock */
#define SIGTERM 15  /* Termination */

/* Signal control — stub in libc_shim.c; handler is ignored, returns NULL */
sighandler_t signal(int sig, sighandler_t handler);

/* Raise a signal — stub in libc_shim.c; always returns 0 */
int raise(int sig);

#endif /* _SIGNAL_H */
