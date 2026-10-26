/*
 * Unit tests for wsprintf
 *
 * Copyright 2002 Bill Medland
 *
 * Ported to Win16 from Wine's dlls/user32/tests/wsprintf.c: wsprintf,
 * and AnsiUpper and AnsiLower, which are Windows 3.1's CharUpper and
 * CharLower. wsprintfW is left out, as Windows 3.1 has no Unicode
 * functions. A few checks of the formats a Windows 3.1 program uses,
 * where an int is 16 bits and a long 32, and of wvsprintf are added.
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301, USA
 */

#include "test.h"

static void wsprintfATest(void)
{
    char buf[25];
    int rc;

    rc=wsprintf(buf, "%010ld", -1L);
    ok(rc == 10, "wsPrintfA length failure: rc=%d\n",rc);
    ok((lstrcmp(buf, "-000000001") == 0),
       "wsprintfA zero padded negative value failure: buf=[%s]\n",(LPSTR)buf);
}

/* The formats with 16-bit ints and 32-bit longs. */
static void wsprintf16Test(void)
{
    static const struct
    {
        const char *expected;
        int rc;
    } results[] =
    {
        { "-1", 2 },
        { "65535", 5 },
        { "ffff", 4 },
        { "FFFF", 4 },
        { "-00042", 6 },
        { "42   |", 6 },
        { "   42|", 6 },
        { "12345678", 8 },
        { "-305419896", 10 },
        { "4294967295", 10 },
        { "abc|  abc|ab", 12 },
        { "x", 1 },
        { "%", 1 },
        { "0x1f", 4 },
    };
    char buf[64];
    int rc, i = 0;

#define CHECK(call) \
    rc = call; \
    ok(rc == results[i].rc && !lstrcmp(buf, results[i].expected), \
       "%d: got %d [%s], expected %d [%s]\n", i, rc, (LPSTR)buf, \
       results[i].rc, (LPSTR)results[i].expected); \
    i++

    CHECK(wsprintf(buf, "%d", -1));
    CHECK(wsprintf(buf, "%u", -1));
    CHECK(wsprintf(buf, "%x", 0xffff));
    CHECK(wsprintf(buf, "%X", 0xffff));
    CHECK(wsprintf(buf, "%06d", -42));
    CHECK(wsprintf(buf, "%-5d|", 42));
    CHECK(wsprintf(buf, "%5d|", 42));
    CHECK(wsprintf(buf, "%lx", 0x12345678L));
    CHECK(wsprintf(buf, "%ld", -0x12345678L));
    CHECK(wsprintf(buf, "%lu", 0xffffffffL));
    CHECK(wsprintf(buf, "%s|%5s|%.2s", (LPSTR)"abc", (LPSTR)"abc", (LPSTR)"abc"));
    CHECK(wsprintf(buf, "%c", 'x'));
    CHECK(wsprintf(buf, "%%"));
    CHECK(wsprintf(buf, "%#x", 0x1f));
#undef CHECK
}

static int call_wvsprintf(char *buf, const char *format, ...)
{
    va_list args;
    int rc;

    va_start(args, format);
    rc = wvsprintf(buf, format, args);
    va_end(args);
    return rc;
}

static void wvsprintfTest(void)
{
    char buf[64];
    int rc;

    rc = call_wvsprintf(buf, "%d %s %lx", 12, (LPSTR)"ab", 0xdeadbeefL);
    ok(rc == 14, "wvsprintf returned %d\n", rc);
    ok(!lstrcmp(buf, "12 ab deadbeef"), "wvsprintf gave [%s]\n", (LPSTR)buf);
}

/* Test if the CharUpper / CharLower functions return true 16 bit results,
   if the input is a 16 bit input value. Up to Wine 11-2003 the input value
   0xff returns 0xffffffff. */

static void CharUpperTest(void)
{
    long i,out,failed;

    failed = 0;
    for (i=0;i<256;i++)
    	{
	out = (long)AnsiUpper((LPSTR)i);
	if ((out >> 16) != 0)
	   {
	   failed = 1;
	   break;
	   }
	}
    ok(!failed,"CharUpper failed - 16bit input (0x%0lx) returned 32bit result (0x%0lx)\n",i,out);
    out = (long)AnsiUpper((LPSTR)(long)'a');
    ok(out == 'A', "AnsiUpper('a') returned %lx\n", out);
}

static void CharLowerTest(void)
{
    long i,out,failed;

    failed = 0;
    for (i=0;i<256;i++)
    	{
	out = (long)AnsiLower((LPSTR)i);
	if ((out >> 16) != 0)
	   {
	   failed = 1;
	   break;
	   }
	}
    ok(!failed,"CharLower failed - 16bit input (0x%0lx) returned 32bit result (0x%0lx)\n",i,out);
    out = (long)AnsiLower((LPSTR)(long)'Q');
    ok(out == 'q', "AnsiLower('Q') returned %lx\n", out);
}


START_TEST(wsprintf)
{
    wsprintfATest();
    wsprintf16Test();
    wvsprintfTest();
    CharUpperTest();
    CharLowerTest();
}
