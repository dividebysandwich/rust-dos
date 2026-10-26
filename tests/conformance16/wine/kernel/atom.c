/*
 * Unit tests for atom functions
 *
 * Copyright (c) 2002 Alexandre Julliard
 *
 * Ported to Win16 from Wine's dlls/kernel32/tests/atom.c: the ANSI
 * functions of KERNEL (GlobalAddAtom and AddAtom and the others), which
 * Windows 3.1 has, without the Unicode ones and SetLastError, which it
 * doesn't. Where Windows 9x and NT differ, Windows 9x's answers are the
 * ones Windows 3.1 gives.
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

static void do_initA(char *tmp, const char *pattern, int len)
{
    const char *p = pattern;

    while (len--)
    {
        *tmp++ = *p++;
        if (!*p) p = pattern;
    }
    *tmp = '\0';
}

static void test_add_atom(void)
{
    ATOM atom;
    long i;

    atom = GlobalAddAtom( "foobar" );
    ok( atom >= 0xc000, "bad atom id %x\n", atom );

    /* Verify that it can be found (or not) appropriately */
    ok( GlobalFindAtom( "foobar" ) == atom, "could not find atom foobar\n" );
    ok( GlobalFindAtom( "FOOBAR" ) == atom, "could not find atom FOOBAR\n" );
    ok( !GlobalFindAtom( "_foobar" ), "found _foobar\n" );

    /* Test integer atoms
     * (0x0001 .. 0xbfff) should be valid;
     * (0xc000 .. 0xffff) should be invalid */

    ok( GlobalAddAtom(0) == 0, "succeeded to add atom 0\n" );

    for (i = 1; i <= 0xbfff; i++)
    {
        ok( GlobalAddAtom(MAKEINTATOM(i)) == (ATOM)i,
            "failed to add atom %lx\n", i );
    }

    for (i = 0xc000; i <= 0xffff; i++)
    {
        ok( !GlobalAddAtom(MAKEINTATOM(i)), "succeeded adding %lx\n", i );
    }
}

static void test_get_atom_name(void)
{
    char buf[10];
    long i;
    UINT len;
    char in[257], out[257];

    ATOM atom = GlobalAddAtom( "foobar" );

    /* Get the name of the atom we added above */
    _fmemset( buf, '.', sizeof(buf) );
    len = GlobalGetAtomName( atom, buf, 10 );
    ok( len == strlen("foobar"), "bad length %d\n", len );
    ok( !_fmemcmp( buf, "foobar\0...", 10 ), "bad buffer contents\n" );

    /* Check error code returns */
    _fmemset(buf, '.', 10);
    ok( !GlobalGetAtomName( atom, buf,  0 ), "succeeded\n" );
    ok( !_fmemcmp( buf, "..........", 10 ), "should not touch buffer\n" );

    /* Test integer atoms */
    for (i = 0; i <= 0xbfff; i++)
    {
        _fmemset( buf, 'a', 10 );
        len = GlobalGetAtomName( (ATOM)i, buf, 10 );
        if (i)
        {
            char res[20];
            ok( (len > 1) && (len < 7), "bad length %d\n", len );
            wsprintf( res, "#%u", (UINT)i );
            _fmemset( res + strlen(res) + 1, 'a', 10 );
            ok( !_fmemcmp( res, buf, 10 ), "bad buffer contents %s\n", (LPSTR)buf );
            if (len <= 1 || len >= 7) break;  /* don't bother testing all of them */
        }
        else
            ok( !len, "bad length %d\n", len );
    }

    _fmemset( buf, '.', sizeof(buf) );
    len = GlobalGetAtomName( atom, buf, 6 );
    ok( len == 0 ||
        len == 5, /* win9x */
        "bad length %d\n", len );
    ok( !_fmemcmp( buf, "fooba\0....", 10 ), "bad buffer contents\n");

    /* test string limits & overflow */
    do_initA(in, "abcdefghij", 255);
    atom = GlobalAddAtom(in);
    ok(atom, "couldn't add atom for %s\n", (LPSTR)in);
    len = GlobalGetAtomName(atom, out, sizeof(out));
    ok(len == 255, "length mismatch (%u instead of 255)\n", len);
    for (i = 0; i < 255; i++)
    {
        ok(out[i] == "abcdefghij"[i % 10], "wrong string at %i (%c instead of %c)\n", (int)i, out[i], "abcdefghij"[i % 10]);
    }
    ok(out[255] == '\0', "wrong end of string\n");
    _fmemset(out, '.', sizeof(out));
    len = GlobalGetAtomName(atom, out, 10);
    ok(len == 9, "length %u instead of 9\n", len);
    for (i = 0; i < 9; i++)
    {
        ok(out[i] == "abcdefghij"[i % 10], "wrong string at %i (%c instead of %c)\n", (int)i, out[i], "abcdefghij"[i % 10]);
    }
    ok(out[9] == '\0', "wrong end of string\n");
    ok(out[10] == '.', "wrote after end of buf\n");
    do_initA(in, "abcdefghij", 256);
    atom = GlobalAddAtom(in);
    ok(!atom, "succeeded\n");
}

static void test_error_handling(void)
{
    char buffer[260];

    _fmemset( buffer, 'a', 256 );
    buffer[256] = 0;
    ok( !GlobalAddAtom(buffer), "add succeeded\n" );
    ok( !GlobalFindAtom(buffer), "find succeeded\n" );
}

static void test_local_add_atom(void)
{
    ATOM atom;
    long i;

    atom = AddAtom( "foobar" );
    ok( atom >= 0xc000, "bad atom id %x\n", atom );

    /* Verify that it can be found (or not) appropriately */
    ok( FindAtom( "foobar" ) == atom, "could not find atom foobar\n" );
    ok( FindAtom( "FOOBAR" ) == atom, "could not find atom FOOBAR\n" );
    ok( !FindAtom( "_foobar" ), "found _foobar\n" );

    ok( AddAtom(0) == 0, "succeeded to add atom 0\n" );

    for (i = 1; i <= 0xbfff; i++)
    {
        ok( AddAtom(MAKEINTATOM(i)) == (ATOM)i, "failed to add atom %lx\n", i );
    }

    for (i = 0xc000; i <= 0xffff; i++)
    {
        ok( !AddAtom(MAKEINTATOM(i)), "succeeded adding %lx\n", i );
    }
}

static void test_local_get_atom_name(void)
{
    char buf[10], in[257], out[257];
    long i;
    UINT len;

    ATOM atom = AddAtom( "foobar" );

    /* Get the name of the atom we added above */
    _fmemset( buf, '.', sizeof(buf) );
    len = GetAtomName( atom, buf, 10 );
    ok( len == strlen("foobar"), "bad length %d\n", len );
    ok( !_fmemcmp( buf, "foobar\0...", 10 ), "bad buffer contents\n" );

    /* Get the name of the atom we added above */
    _fmemset( buf, '.', sizeof(buf) );
    len = GetAtomName( atom, buf, 6 );
    ok( len == 5, "bad length %d\n", len );
    ok( !_fmemcmp( buf, "fooba\0....", 10 ), "bad buffer contents\n" );

    /* Check error code returns */
    _fmemset(buf, '.', 10);
    ok( !GetAtomName( atom, buf, 0 ), "succeeded\n" );
    ok( !_fmemcmp( buf, "..........", 10 ), "should not touch buffer\n" );

    /* Test integer atoms */
    for (i = 0; i <= 0xbfff; i++)
    {
        _fmemset( buf, 'a', 10 );
        len = GetAtomName( (ATOM)i, buf, 10 );
        if (i)
        {
            char res[20];
            ok( (len > 1) && (len < 7), "bad length %d for %s\n", len, (LPSTR)buf );
            wsprintf( res, "#%u", (UINT)i );
            _fmemset( res + strlen(res) + 1, 'a', 10 );
            ok( !_fmemcmp( res, buf, 10 ), "bad buffer contents %s\n", (LPSTR)buf );
            if (len <= 1 || len >= 7) break;  /* don't bother testing all of them */
        }
        else
            ok( !len, "bad length %d\n", len );
    }

    /* test string limits & overflow */
    do_initA(in, "abcdefghij", 255);
    atom = AddAtom(in);
    ok(atom, "couldn't add atom for %s\n", (LPSTR)in);
    len = GetAtomName(atom, out, sizeof(out));
    ok(len == 255, "length mismatch (%u instead of 255)\n", len);
    for (i = 0; i < 255; i++)
    {
        ok(out[i] == "abcdefghij"[i % 10], "wrong string at %i (%c instead of %c)\n", (int)i, out[i], "abcdefghij"[i % 10]);
    }
    ok(out[255] == '\0', "wrong end of string\n");
    _fmemset(out, '.', sizeof(out));
    len = GetAtomName(atom, out, 10);
    ok(len == 9, "succeeded %d\n", len);
    for (i = 0; i < 9; i++)
    {
        ok(out[i] == "abcdefghij"[i % 10], "wrong string at %i (%c instead of %c)\n", (int)i, out[i], "abcdefghij"[i % 10]);
    }
    ok(out[9] == '\0', "wrong end of string\n");
    ok(out[10] == '.', "buffer overwrite\n");
    do_initA(in, "abcdefghij", 256);
    atom = AddAtom(in);
    ok(!atom, "succeeded\n");
}

static void test_local_error_handling(void)
{
    char buffer[260];

    _fmemset( buffer, 'a', 256 );
    buffer[256] = 0;
    ok( !AddAtom(buffer), "add succeeded\n" );
    ok( !FindAtom(buffer), "find succeeded\n" );
}

START_TEST(atom)
{
    test_add_atom();
    test_get_atom_name();
    test_error_handling();
    test_local_add_atom();
    test_local_get_atom_name();
    test_local_error_handling();
}
