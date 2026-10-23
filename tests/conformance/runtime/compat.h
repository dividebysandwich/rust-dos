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

/* Flags of Windows Vista's common controls that comctl32's header.c and
 * listview.c test, which MinGW's commctrl.h has only for Vista on. */
#ifndef HDF_FIXEDWIDTH
#define HDF_FIXEDWIDTH 0x0100
#endif
#ifndef HDS_NOSIZING
#define HDS_NOSIZING 0x0800
#endif
#ifndef LVS_EX_TRANSPARENTBKGND
#define LVS_EX_TRANSPARENTBKGND 0x00400000
#endif
#ifndef LVS_EX_HEADERINALLVIEWS
#define LVS_EX_HEADERINALLVIEWS 0x02000000
#endif
#ifndef LVFI_SUBSTRING
#define LVFI_SUBSTRING 0x0004
#endif
