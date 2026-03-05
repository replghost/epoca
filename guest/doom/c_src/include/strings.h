#ifndef _STRINGS_H
#define _STRINGS_H

/* strings.h — legacy BSD header; doom includes it for strcasecmp/strncasecmp.
 * On our freestanding target these are declared in string.h and implemented
 * in libc_shim.c.  We just pull in string.h to satisfy the include. */

#include <string.h>

#endif /* _STRINGS_H */
