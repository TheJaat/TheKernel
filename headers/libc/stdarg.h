#ifndef __STD_ARG_H__
#define __STD_ARG_H__

// Define va_list as char pointer
typedef char* va_list;

// Define the macros
/*
 * This macro calculates the size of a type n rounded up to the
 * nearest multiple of sizeof(int). This is necessary for
 * proper alignment.
 */
#define _INTSIZEOF(n) ((sizeof(n) + sizeof(int) - 1) & ~(sizeof(int) - 1))


/* This macro initializes the va_list variable ap to point to the
 * first variable argument. It does this by advancing past the last
 * named parameter v.
 */
#define va_start(ap, v) (ap = (va_list)&v + _INTSIZEOF(v))


/* This macro retrieves the next argument of type t from the va_list
 * and advances the va_list pointer ap.
 */
#define va_arg(ap, t) (*(t*)((ap += _INTSIZEOF(t)) - _INTSIZEOF(t)))


/*
 * This macro cleans up the va_list variable ap.
 */
#define va_end(ap) (ap = (va_list)0)

#endif /* __STD_ARG_H__ */