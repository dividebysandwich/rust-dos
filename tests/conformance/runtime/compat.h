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

/* A pointer type of mmsystem.h that MinGW's lacks (winmm's midi.c). */
#define LPMIDIEVENT MIDIEVENT *

/* GUID_NULL, which Wine's objbase.h declares and MinGW's only in
 * cguid.h (ole32's compobj.c). A declaration of the struct's type is
 * all C needs here; libuuid has the GUID. */
struct _GUID;
extern const struct _GUID GUID_NULL;

/* IObjContext, an interface of Windows 2000's object contexts that
 * MinGW's headers lack (compobj.c). The tests only take a pointer to it
 * and release it, as an IUnknown. */
#define IObjContext IUnknown
#define IID_IObjContext IID_IUnknown
#define IObjContext_Release IUnknown_Release
