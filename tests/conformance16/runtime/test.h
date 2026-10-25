/*
 * A Win16 version of Wine's test framework (include/wine/test.h), for
 * Wine's conformance tests ported to Windows 3.1 programs: ok(), skip(),
 * trace(), todo_wine, broken() and START_TEST, with the results written
 * to standard output (DOS's handle 1) in the form Wine's tests write
 * them. Built with OpenWatcom in the large model, so that strings and
 * va_list are far pointers, as wvsprintf takes them.
 *
 * Copyright 2002 Alexandre Julliard (Wine's test.h, which this follows)
 *
 * This library is free software; you can redistribute it and/or modify
 * it under the terms of the GNU Lesser General Public License as
 * published by the Free Software Foundation; either version 2.1 of the
 * License, or (at your option) any later version.
 */

#ifndef WINE_TEST16_H
#define WINE_TEST16_H

#include <windows.h>
#include <stdarg.h>
#include <string.h>

static const char *winetest_file;
static int winetest_line;
static const char *winetest_name;
/* Longs: a test file's checks can be more than an int counts. */
static long winetest_successes;
static long winetest_failures;
static long winetest_todo_successes;
static long winetest_todo_failures;
static long winetest_skipped;
static int winetest_todo_level;

static void winetest_write(const char *text)
{
    _lwrite(1, text, lstrlen(text));
}

static void winetest_print(const char *format, va_list args)
{
    char buffer[512];
    wvsprintf(buffer, format, args);
    winetest_write(buffer);
}

static void winetest_printf(const char *format, ...)
{
    va_list args;
    va_start(args, format);
    winetest_print(format, args);
    va_end(args);
}

static void winetest_set_location(const char *file, int line)
{
    const char *p = file;
    winetest_file = file;
    for (; *p; p++)
        if (*p == '\\' || *p == '/')
            winetest_file = p + 1;
    winetest_line = line;
}

static int winetest_ok(int condition, const char *msg, ...)
{
    va_list args;
    if (winetest_todo_level) {
        if (condition) {
            winetest_printf("%s:%d: Test succeeded inside todo block: ",
                            winetest_file, winetest_line);
            va_start(args, msg);
            winetest_print(msg, args);
            va_end(args);
            winetest_todo_failures++;
            return 0;
        }
        winetest_todo_successes++;
        return 1;
    }
    if (!condition) {
        winetest_printf("%s:%d: Test failed: ", winetest_file, winetest_line);
        va_start(args, msg);
        winetest_print(msg, args);
        va_end(args);
        winetest_failures++;
        return 0;
    }
    winetest_successes++;
    return 1;
}

static void winetest_trace(const char *msg, ...)
{
    va_list args;
    winetest_printf("%s:%d: ", winetest_file, winetest_line);
    va_start(args, msg);
    winetest_print(msg, args);
    va_end(args);
}

static void winetest_skip(const char *msg, ...)
{
    va_list args;
    winetest_printf("%s:%d: Tests skipped: ", winetest_file, winetest_line);
    va_start(args, msg);
    winetest_print(msg, args);
    va_end(args);
    winetest_skipped++;
}

#define ok_(file, line) (winetest_set_location(file, line), 0) ? 0 : winetest_ok
#define trace_(file, line) (winetest_set_location(file, line), 0) ? (void)0 : winetest_trace
#define skip_(file, line) (winetest_set_location(file, line), 0) ? (void)0 : winetest_skip

#define ok ok_(__FILE__, __LINE__)
#define trace trace_(__FILE__, __LINE__)
#define skip skip_(__FILE__, __LINE__)
#define win_skip skip

/* What Windows 3.1 does differently from Wine's Win32 expectations is
 * broken() there: accepted, as Wine accepts old Windows' behaviour. */
#define broken(x) (x)

#define todo_wine for (winetest_todo_level = 1; winetest_todo_level; winetest_todo_level = 0)

/* A test file's tests: START_TEST(name) { ... } is the program's
 * WinMain, which runs them and writes the summary. */
#define START_TEST(name)                                                    \
    static void func_##name(void);                                          \
    int PASCAL WinMain(HINSTANCE instance, HINSTANCE previous,              \
                       LPSTR line, int show)                                \
    {                                                                       \
        winetest_name = #name;                                              \
        func_##name();                                                      \
        winetest_printf("%s: %ld tests executed (%ld marked as todo, "      \
                        "%ld failures), %ld skipped.\r\n",                  \
                        winetest_name,                                      \
                        winetest_successes + winetest_failures              \
                            + winetest_todo_successes                       \
                            + winetest_todo_failures,                       \
                        winetest_todo_successes,                            \
                        winetest_failures + winetest_todo_failures,         \
                        winetest_skipped);                                  \
        return winetest_failures + winetest_todo_failures != 0;             \
    }                                                                       \
    static void func_##name(void)

#endif
