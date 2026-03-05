#ifndef _STDDEF_H
#define _STDDEF_H

/* size_t — unsigned integer type for object sizes (32-bit on riscv32) */
typedef unsigned int size_t;

/* ptrdiff_t — signed integer type for pointer differences */
typedef int ptrdiff_t;

/* wchar_t — wide character type (doom doesn't use this, but some headers need it) */
typedef int wchar_t;

/* NULL — null pointer constant */
#ifndef NULL
#define NULL ((void *)0)
#endif

/* offsetof — byte offset of a member within a struct */
#define offsetof(type, member) __builtin_offsetof(type, member)

#endif /* _STDDEF_H */
