#ifndef _CTYPE_H
#define _CTYPE_H

/* Character classification — implemented in libc_shim.c */
int isdigit(int c);
int isspace(int c);
int isalpha(int c);
int isalnum(int c);
int isprint(int c);
int isupper(int c);
int islower(int c);
int isxdigit(int c);

/* Character conversion — implemented in libc_shim.c */
int toupper(int c);
int tolower(int c);

#endif /* _CTYPE_H */
