/*
 * What Wine 1.2.3's tests take from Wine's own headers and MinGW's
 * headers lack. Each test file is built with this header included
 * first (-include), so the files themselves stay unchanged.
 *
 * Part of rust-dos, GPL-2.0-or-later.
 */

#include <stdarg.h>

/* Wine's names for the Microsoft va_list, which on x86 is va_list. */
#define __ms_va_list va_list
#define __ms_va_start va_start
#define __ms_va_end va_end

/* A GetLocaleInfo flag of Windows Vista. */
#ifndef LOCALE_RETURN_GENITIVE_NAMES
#define LOCALE_RETURN_GENITIVE_NAMES 0x10000000
#endif
