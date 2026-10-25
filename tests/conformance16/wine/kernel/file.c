/*
 * Unit tests for file functions in Wine
 *
 * Copyright (c) 2002, 2004 Jakob Eriksson
 * Copyright (c) 2008 Jeff Zaroyko
 *
 * Ported to Win16 from Wine's dlls/kernel32/tests/file.c: the tests of
 * _lopen, _lcreat, _lread, _lwrite, _llseek, _lclose, _hread and
 * _hwrite, of OpenFile and of GetTempFileName, with DOS's calls (INT
 * 21h, through DOS3Call and directly) where Wine uses DeleteFile,
 * FindFirstFile and SetFileAttributes, which Windows 3.1 doesn't have.
 * What _hwrite writes in blocks of 10000 is read back into a global
 * block of a megabyte, and _lwrite's test, whose count is 16 bits
 * here, writes less. Where Windows 9x and NT differ, Windows 9x's
 * answer is the one kept; OpenFile's handles are DOS's. Left out are
 * the Unicode functions, CreateFile and the functions that need it,
 * file names longer than DOS's 8.3, and a volume label made by _lcreat.
 * Added are GetDriveType, OpenFile's OF_PARSE, OF_REOPEN and search of
 * the Windows directory, and DOS's calls with far data: its version,
 * files created, written, read and deleted, a search through the DTA,
 * attributes and the current directory.
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
#include <stdlib.h>
#include <dos.h>

#define MAX_PATH 260
#ifndef FILE_BEGIN
#define FILE_BEGIN 0
#define FILE_CURRENT 1
#define FILE_END 2
#endif

static const char filename[] = "testfile.xxx";
static const char sillytext[] =
"en larvig liten text dx \033 gx hej 84 hej 4484 ! \001\033 bla bl\na.. bla bla."
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"1234 43 4kljf lf &%%%&&&&&& 34 4 34   3############# 33 3 3 3 # 3## 3"
"sdlkfjasdlkfj a dslkj adsklf  \n  \nasdklf askldfa sdlkf \nsadklf asdklf asdf ";

static char buffer[10000];

/* DOS's calls: the registers in and out, through DOS3Call or INT 21h. */
struct dosregs
{
    WORD ax, bx, cx, dx, si, di, ds, es, flags;
};

extern void FAR PASCAL DOS3Call(void);

#define CARRY(r) ((r)->flags & 1)

static void dos_call(struct dosregs *r, int through_dos3call)
{
    WORD ax_ = r->ax, bx_ = r->bx, cx_ = r->cx, dx_ = r->dx;
    WORD si_ = r->si, di_ = r->di, ds_ = r->ds, es_ = r->es, fl_;
    void (FAR PASCAL *proc)(void) = DOS3Call;
    int direct = !through_dos3call;

    _asm {
        push ds
        push es
        push si
        push di
        mov ax, ax_
        mov bx, bx_
        mov cx, cx_
        mov dx, dx_
        mov si, si_
        mov di, di_
        mov es, es_
        push ds_
        pop ds
        cmp direct, 0
        jne do_int
        call dword ptr proc
        jmp done
    do_int:
        int 21h
    done:
        pushf
        pop fl_
        mov ax_, ax
        mov bx_, bx
        mov cx_, cx
        mov dx_, dx
        mov si_, si
        mov di_, di
        mov es_, es
        mov ds_, ds
        pop di
        pop si
        pop es
        pop ds
    }
    r->ax = ax_; r->bx = bx_; r->cx = cx_; r->dx = dx_;
    r->si = si_; r->di = di_; r->ds = ds_; r->es = es_; r->flags = fl_;
}

static void dos_set_far(WORD FAR *seg, WORD FAR *off, const void FAR *p)
{
    *seg = SELECTOROF(p);
    *off = OFFSETOF(p);
}

/* DOS's delete (AH=41h), FALSE if it fails. */
static BOOL delete_file(LPCSTR name)
{
    struct dosregs r;
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4100;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, 1);
    return !CARRY(&r);
}

/* DOS's attributes (AX=4300h), or -1. */
static int get_attributes(LPCSTR name)
{
    struct dosregs r;
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4300;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, 1);
    return CARRY(&r) ? -1 : (int)r.cx;
}

static BOOL set_attributes(LPCSTR name, WORD attr)
{
    struct dosregs r;
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4301;
    r.cx = attr;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, 1);
    return !CARRY(&r);
}

/* A DOS find (AH=4Eh) of `name` with `attr`, through a DTA of our own;
 * the name found in `found`. */
static BOOL find_file(LPCSTR name, WORD attr, char *found)
{
    static BYTE dta[43];
    struct dosregs r;
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x1a00;
    dos_set_far(&r.ds, &r.dx, dta);
    dos_call(&r, 1);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4e00;
    r.cx = attr;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, 1);
    if (CARRY(&r)) return FALSE;
    if (found) lstrcpy(found, (LPCSTR)(dta + 0x1e));
    return TRUE;
}

static void test__hread( void )
{
    HFILE filehandle;
    long bytes_read;
    long bytes_wanted;
    long i;
    BOOL ret;

    set_attributes(filename, 0); /* be sure to remove stale files */
    delete_file( filename );
    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READ );

    ok( HFILE_ERROR != filehandle, "couldn't open file \"%s\" again\n", (LPCSTR)filename );

    bytes_read = _hread( filehandle, buffer, 2 * strlen( sillytext ) );

    ok( lstrlen( sillytext ) == bytes_read, "file read size error\n" );

    for (bytes_wanted = 0; bytes_wanted < lstrlen( sillytext ); bytes_wanted++)
    {
        ok( 0 == _llseek( filehandle, 0, FILE_BEGIN ), "_llseek complains\n" );
        ok( _hread( filehandle, buffer, bytes_wanted ) == bytes_wanted, "erratic _hread return value\n" );
        for (i = 0; i < bytes_wanted; i++)
        {
            ok( buffer[i] == sillytext[i], "that's not what's written\n" );
        }
    }

    ok( HFILE_ERROR != _lclose( filehandle ), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret != 0, "DeleteFile failed\n" );
}

static WORD ahincr(void)
{
    extern void FAR PASCAL __AHINCR(void);
    return OFFSETOF((void FAR *)__AHINCR);
}

static void test__hwrite( void )
{
    HFILE filehandle;
    long bytes_read;
    long bytes_written;
    unsigned long blocks;
    long i;
    HGLOBAL memory_object;
    BYTE FAR *contents;
    WORD sel;
    char checksum[1];
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, "", 0 ), "_hwrite complains\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READ );

    bytes_read = _hread( filehandle, buffer, 1);

    ok( 0 == bytes_read, "file read size error\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READWRITE );

    bytes_written = 0;
    checksum[0] = '\0';
    srand( (unsigned)GetTickCount() );
    for (blocks = 0; blocks < 100; blocks++)
    {
        for (i = 0; i < (long)sizeof( buffer ); i++)
        {
            buffer[i] = rand(  );
            checksum[0] = checksum[0] + buffer[i];
        }
        ok( HFILE_ERROR != _hwrite( filehandle, buffer, sizeof( buffer ) ), "_hwrite complains\n" );
        bytes_written = bytes_written + sizeof( buffer );
    }

    ok( HFILE_ERROR != _hwrite( filehandle, checksum, 1 ), "_hwrite complains\n" );
    bytes_written++;

    ok( HFILE_ERROR != _lclose( filehandle ), "_lclose complains\n" );

    memory_object = GlobalAlloc( GMEM_MOVEABLE | GMEM_ZEROINIT, bytes_written );

    ok( 0 != memory_object, "GlobalAlloc fails. (Could be out of memory.)\n" );
    if (!memory_object) return;

    filehandle = _lopen( filename, OF_READ );

    contents = GlobalLock( memory_object );

    ok( NULL != contents, "GlobalLock whines\n" );

    ok( bytes_written == _hread( filehandle, contents, bytes_written), "read length differ from write length\n" );

    /* The block's segments are __AHINCR apart. */
    sel = SELECTOROF(contents);
    checksum[0] = '\0';
    i = 0;
    do
    {
        BYTE FAR *p = MAKELP(sel + (WORD)(i >> 16) * ahincr(), 0);
        WORD n = (bytes_written - 1 - i) > 0xffffL ? 0xffff
                 : (WORD)(bytes_written - 1 - i);
        WORD j;
        for (j = 0; j < n; j++)
            checksum[0] += p[j];
        i += n;
        if (i < bytes_written - 1)
        {
            checksum[0] += p[0xffff];
            i++;
        }
    }
    while (i < bytes_written - 1);

    contents = MAKELP(sel + (WORD)(i >> 16) * ahincr(), (WORD)i);
    ok( checksum[0] == contents[0], "stored checksum differ from computed checksum\n" );

    ok( HFILE_ERROR != _lclose( filehandle ), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret != 0, "DeleteFile failed\n" );

    GlobalUnlock( memory_object );
    GlobalFree( memory_object );
}


static void test__lclose( void )
{
    HFILE filehandle;
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    /* A closed handle can't be closed again. */
    ok( HFILE_ERROR == _lclose(filehandle), "_lclose of a closed handle succeeded\n" );

    ret = delete_file( filename );
    ok( ret != 0, "DeleteFile failed\n" );
}


static void test__lcreat( void )
{
    HFILE filehandle;
    char slashname[] = "testfi/";
    char found[13];
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( 0 == _llseek( filehandle, 0, FILE_BEGIN ), "_llseek complains\n" );

    ok( _hread( filehandle, buffer, strlen( sillytext ) ) ==  lstrlen( sillytext ), "erratic _hread return value\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ok( find_file( filename, 0, found ), "should be able to find file\n" );
    ok( !lstrcmpi( found, filename ), "found \"%s\"\n", (LPSTR)found );
    /* Attributes are compared without the archive bit, which DOS sets. */
    ok( (get_attributes( filename ) & ~0x20) == 0x00, "attributes %x\n", get_attributes( filename ) );

    ret = delete_file(filename);
    ok( ret != 0, "DeleteFile failed\n");

    filehandle = _lcreat( filename, 1 ); /* readonly */
    ok( HFILE_ERROR != filehandle, "couldn't create file \"%s\"\n", (LPCSTR)filename );

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite shouldn't be able to write never the less\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ok( find_file( filename, 0, NULL ), "should be able to find file\n" );
    ok( (get_attributes( filename ) & ~0x20) == 0x01, "attributes %x\n", get_attributes( filename ) );

    ok( 0 == delete_file( filename ), "shouldn't be able to delete a readonly file\n" );

    ok( set_attributes(filename, 0 ) != 0, "couldn't change attributes on file\n" );

    ok( delete_file( filename ) != 0, "now it should be possible to delete the file!\n" );

    filehandle = _lcreat( filename, 2 );
    ok( HFILE_ERROR != filehandle, "couldn't create file \"%s\"\n", (LPCSTR)filename );

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( 0 == _llseek( filehandle, 0, FILE_BEGIN ), "_llseek complains\n" );

    ok( _hread( filehandle, buffer, strlen( sillytext ) ) ==  lstrlen( sillytext ), "erratic _hread return value\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    /* A hidden file is found when the search asks for hidden ones. */
    ok( !find_file( filename, 0, NULL ), "found a hidden file\n" );
    ok( find_file( filename, 2, NULL ), "should STILL be able to find file\n" );
    ok( (get_attributes( filename ) & ~0x20) == 0x02, "attributes %x\n", get_attributes( filename ) );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );

    filehandle = _lcreat( filename, 4 ); /* SYSTEM file */
    ok( HFILE_ERROR != filehandle, "couldn't create file \"%s\"\n", (LPCSTR)filename );

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( 0 == _llseek( filehandle, 0, FILE_BEGIN ), "_llseek complains\n" );

    ok( _hread( filehandle, buffer, strlen( sillytext ) ) ==  lstrlen( sillytext ), "erratic _hread return value\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ok( find_file( filename, 4, NULL ), "should STILL be able to find file\n" );
    ok( (get_attributes( filename ) & ~0x20) == 0x04, "attributes %x\n", get_attributes( filename ) );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );

    filehandle=_lcreat (slashname, 0); /* illegal name */
    ok( filehandle == HFILE_ERROR, "created \"%s\"\n", (LPSTR)slashname );
    if (filehandle != HFILE_ERROR) _lclose(filehandle);
}


static void test__llseek( void )
{
    int i;
    HFILE filehandle;
    char buffer[1];
    long bytes_read;
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    for (i = 0; i < 400; i++)
    {
        ok( _hwrite( filehandle, sillytext, strlen( sillytext ) ) != -1, "_hwrite complains\n" );
    }
    ok( _llseek( filehandle, 400L * strlen( sillytext ), FILE_CURRENT ) != -1, "should be able to seek\n" );
    ok( _llseek( filehandle, 27 + 35L * strlen( sillytext ), FILE_BEGIN ) != -1, "should be able to seek\n" );

    bytes_read = _hread( filehandle, buffer, 1);
    ok( 1 == bytes_read, "file read size error\n" );
    ok( buffer[0] == sillytext[27], "_llseek error, it got lost seeking\n" );
    ok( _llseek( filehandle, -400L * (long)strlen( sillytext ), FILE_END ) != -1, "should be able to seek\n" );

    bytes_read = _hread( filehandle, buffer, 1);
    ok( 1 == bytes_read, "file read size error\n" );
    ok( buffer[0] == sillytext[0], "_llseek error, it got lost seeking\n" );
    ok( _llseek( filehandle, 1000000, FILE_END ) != -1, "should be able to seek past file; poor, poor Windows programmers\n" );

    /* The positions _llseek returns. */
    ok( _llseek( filehandle, 0, FILE_END ) == 400L * strlen( sillytext ),
        "end at %ld\n", _llseek( filehandle, 0, FILE_END ) );
    ok( _llseek( filehandle, 10, FILE_BEGIN ) == 10, "_llseek to 10\n" );
    ok( _llseek( filehandle, 5, FILE_CURRENT ) == 15, "_llseek by 5\n" );
    ok( _llseek( filehandle, -20, FILE_CURRENT ) == HFILE_ERROR,
        "_llseek before the start succeeded\n" );
    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );
}


static void test__llopen( void )
{
    HFILE filehandle;
    UINT bytes_read;
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );
    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READ );
    ok( HFILE_ERROR == _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite shouldn't be able to write!\n" );
    bytes_read = _hread( filehandle, buffer, strlen( sillytext ) );
    ok( strlen( sillytext )  == bytes_read, "file read size error\n" );
    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READWRITE );
    bytes_read = _hread( filehandle, buffer, 2 * strlen( sillytext ) );
    ok( strlen( sillytext )  == bytes_read, "file read size error\n" );
    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite should write just fine\n" );
    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_WRITE );
    ok( HFILE_ERROR == _hread( filehandle, buffer, 1 ), "you should only be able to write this file\n" );
    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite should write just fine\n" );
    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );

    /* A file that isn't there. */
    ok( _lopen( filename, OF_READ ) == HFILE_ERROR, "opened a deleted file\n" );
}


static void test__lread( void )
{
    HFILE filehandle;
    UINT bytes_read;
    UINT bytes_wanted;
    UINT i;
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _hwrite( filehandle, sillytext, strlen( sillytext ) ), "_hwrite complains\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READ );

    ok( HFILE_ERROR != filehandle, "couldn't open file \"%s\" again\n", (LPCSTR)filename);

    bytes_read = _lread( filehandle, buffer, 2 * strlen( sillytext ) );

    ok( lstrlen( sillytext ) == bytes_read, "file read size error\n" );

    for (bytes_wanted = 0; bytes_wanted < strlen( sillytext ); bytes_wanted++)
    {
        ok( 0 == _llseek( filehandle, 0, FILE_BEGIN ), "_llseek complains\n" );
        ok( _lread( filehandle, buffer, bytes_wanted ) == bytes_wanted, "erratic _hread return value\n" );
        for (i = 0; i < bytes_wanted; i++)
        {
            ok( buffer[i] == sillytext[i], "that's not what's written\n" );
        }
    }

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );
}


static void test__lwrite( void )
{
    HFILE filehandle;
    UINT bytes_read;
    UINT bytes_written;
    UINT blocks;
    int i;
    char FAR *contents;
    HGLOBAL memory_object;
    char checksum[1];
    BOOL ret;

    filehandle = _lcreat( filename, 0 );
    if (filehandle == HFILE_ERROR)
    {
        ok(0,"couldn't create file \"%s\"\n",(LPCSTR)filename);
        return;
    }

    ok( HFILE_ERROR != _lwrite( filehandle, "", 0 ), "_hwrite complains\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READ );

    bytes_read = _hread( filehandle, buffer, 1);

    ok( 0 == bytes_read, "file read size error\n" );

    ok( HFILE_ERROR != _lclose(filehandle), "_lclose complains\n" );

    filehandle = _lopen( filename, OF_READWRITE );

    bytes_written = 0;
    checksum[0] = '\0';
    srand( (unsigned)GetTickCount() );
    for (blocks = 0; blocks < 6; blocks++)
    {
        for (i = 0; i < (int)sizeof( buffer ); i++)
        {
            buffer[i] = rand(  );
            checksum[0] = checksum[0] + buffer[i];
        }
        ok( HFILE_ERROR != _lwrite( filehandle, buffer, sizeof( buffer ) ), "_hwrite complains\n" );
        bytes_written = bytes_written + sizeof( buffer );
    }

    ok( HFILE_ERROR != _lwrite( filehandle, checksum, 1 ), "_hwrite complains\n" );
    bytes_written++;

    ok( HFILE_ERROR != _lclose( filehandle ), "_lclose complains\n" );

    memory_object = GlobalAlloc( GPTR, bytes_written );

    ok( 0 != memory_object, "GlobalAlloc fails, could be out of memory\n" );
    if (!memory_object) return;

    filehandle = _lopen( filename, OF_READ );

    contents = GlobalLock( memory_object );

    ok( NULL != contents, "GlobalLock whines\n" );

    ok( bytes_written == _lread( filehandle, contents, bytes_written), "read length differ from write length\n" );

    checksum[0] = '\0';
    i = 0;
    do
    {
        checksum[0] += contents[(UINT)i];
        i++;
    }
    while ((UINT)i < bytes_written - 1);

    ok( checksum[0] == contents[(UINT)i], "stored checksum differ from computed checksum\n" );

    ok( HFILE_ERROR != _lclose( filehandle ), "_lclose complains\n" );

    ret = delete_file( filename );
    ok( ret, "DeleteFile failed\n" );

    GlobalUnlock( memory_object );
    GlobalFree( memory_object );
}

static void test_GetTempFileName(void)
{
    UINT result;
    char out[MAX_PATH];
    char *p;
    OFSTRUCT ofs;

    /* On the drive asked for. */
    result = GetTempFileName(TF_FORCEDRIVE | 'C', "abc", 1, out);
    ok(result == 1, "GetTempFileName: returned %u\n", result);
    ok(out[0] == 'C' && out[1] == ':' && out[2] == '\\',
       "GetTempFileName: first three characters should be C:\\, string was actually %s\n",
       (LPSTR)out);
    p = out + lstrlen(out);
    while (p > out && p[-1] != '\\') p--;
    ok(!lstrcmpi(p, "~abc0001.tmp"), "GetTempFileName: Unexpected name \"%s\"\n",
       (LPSTR)out);
    ok(OpenFile(out, &ofs, OF_EXIST) == HFILE_ERROR,
       "GetTempFileName with a number made %s\n", (LPSTR)out);

    result = GetTempFileName(TF_FORCEDRIVE | 'C', "abc", 0x2f, out);
    ok(result == 0x2f, "GetTempFileName: returned %u\n", result);
    p = out + lstrlen(out);
    while (p > out && p[-1] != '\\') p--;
    ok(!lstrcmpi(p, "~abc002f.tmp"), "GetTempFileName: Unexpected name \"%s\"\n",
       (LPSTR)out);

    /* Without a number, it makes a file of its own. */
    result = GetTempFileName(TF_FORCEDRIVE | 'C', "wt", 0, out);
    ok(result != 0, "GetTempFileName: returned %u\n", result);
    ok(out[0] == 'C' && out[1] == ':', "GetTempFileName gave %s\n", (LPSTR)out);
    ok(OpenFile(out, &ofs, OF_EXIST) != HFILE_ERROR,
       "GetTempFileName without a number didn't make %s\n", (LPSTR)out);
    p = out + lstrlen(out);
    while (p > out && p[-1] != '\\') p--;
    ok(lstrlen(p) == 11 && p[0] == '~' && (p[1] | 0x20) == 'w' &&
       (p[2] | 0x20) == 't' && !lstrcmpi(p + 7, ".tmp"),
       "GetTempFileName: Unexpected name \"%s\"\n", (LPSTR)out);
    OpenFile(out, &ofs, OF_DELETE);

    /* In the directory TEMP names, of the drive GetTempDrive gives. */
    result = GetTempFileName(0, "abc", 3, out);
    ok(result == 3, "GetTempFileName: returned %u\n", result);
    ok((out[0] | 0x20) == (GetTempDrive(0) | 0x20) && out[1] == ':',
       "GetTempFileName gave %s, temp drive %c\n", (LPSTR)out, GetTempDrive(0));
    p = out + lstrlen(out);
    while (p > out && p[-1] != '\\') p--;
    ok(!lstrcmpi(p, "~abc0003.tmp"), "GetTempFileName: Unexpected name \"%s\"\n",
       (LPSTR)out);
}

static void test_GetDriveType(void)
{
    UINT type;

    type = GetDriveType(2);
    ok(type == DRIVE_FIXED, "GetDriveType(C:) %u\n", type);
    type = GetDriveType(26);
    ok(type == 0, "GetDriveType of drive 26 %u\n", type);
}

static void test_OpenFile(void)
{
    HFILE hFile;
    OFSTRUCT ofs;
    BOOL ret;
    UINT length;

    static const char foo[] = ".\\nofile.baz";
    char buff[MAX_PATH];
    char *p;

    /* Check for existing file */
    length = GetWindowsDirectory(buff, MAX_PATH);
    ok(length > 0, "GetWindowsDirectory failed\n");
    p = buff + strlen(buff);
    if (p > buff && p[-1] != '\\') *p++ = '\\';
    strcpy( p, "wtopen.tst" );
    hFile = _lcreat(buff, 0);
    ok(hFile != HFILE_ERROR, "couldn't create %s\n", (LPSTR)buff);
    _lclose(hFile);

    memset(&ofs, 0xA5, sizeof(ofs));
    hFile = OpenFile(buff, &ofs, OF_EXIST);
    /* The handle it opened and closed: one after DOS's own five. */
    ok( hFile != HFILE_ERROR && hFile > 4, "%s not found : %d\n", (LPSTR)buff, hFile );
    ok( ofs.cBytes == sizeof(ofs), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0, "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( ofs.fFixedDisk, "OpenFile set ofs.fFixedDisk to %d\n", ofs.fFixedDisk );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    /* It's closed (not tried on a standard handle, which would then be
     * closed for good). */
    if (hFile > 4)
        ok( _lclose(hFile) == HFILE_ERROR, "OF_EXIST left handle %d open\n", hFile );

    /* Found in the Windows directory by its name alone. */
    memset(&ofs, 0xA5, sizeof(ofs));
    hFile = OpenFile("wtopen.tst", &ofs, OF_EXIST);
    ok( hFile != HFILE_ERROR, "wtopen.tst not found in the Windows directory\n" );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    hFile = OpenFile(buff, &ofs, OF_DELETE);
    ok( hFile != HFILE_ERROR, "OpenFile failed on delete (%d)\n", hFile );

    /* Check for nonexistent file */
    memset(&ofs, 0xA5, sizeof(ofs));
    hFile = OpenFile(foo, &ofs, OF_EXIST);
    ok( hFile == HFILE_ERROR, "hFile != HFILE_ERROR : %d\n", hFile);
    ok( ofs.nErrCode == 2, "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );

    /* OF_PARSE fills in the path without opening anything. */
    memset(&ofs, 0xA5, sizeof(ofs));
    hFile = OpenFile(foo, &ofs, OF_PARSE);
    ok( hFile == 0, "OF_PARSE returned %d\n", hFile );
    ok( ofs.cBytes == sizeof(ofs), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( lstrcmpi(ofs.szPathName, "C:\\NOFILE.BAZ") == 0,
        "OF_PARSE gave '%s'\n", (LPSTR)ofs.szPathName );
    hFile = OpenFile("sub\\..\\x.y", &ofs, OF_PARSE);
    ok( hFile == 0, "OF_PARSE returned %d\n", hFile );
    ok( lstrcmpi(ofs.szPathName, "C:\\X.Y") == 0 ||
        broken(lstrcmpi(ofs.szPathName, "C:\\SUB\\..\\X.Y") == 0),
        "OF_PARSE gave '%s'\n", (LPSTR)ofs.szPathName );

    length = GetWindowsDirectory(buff, MAX_PATH);
    strcpy( buff, "C:\\" );
    strcat( buff, filename );

    memset(&ofs, 0xA5, sizeof(ofs));
    /* Create an empty file */
    hFile = OpenFile(filename, &ofs, OF_CREATE);
    ok( hFile != HFILE_ERROR, "OpenFile failed to create nonexistent file\n" );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    ok( _lwrite(hFile, "abc", 3) == 3, "couldn't write to the created file\n" );
    ret = _lclose(hFile);
    ok( !ret, "_lclose() returns %d\n", ret );
    ok( get_attributes(filename) != -1, "file wasn't made\n" );

    memset(&ofs, 0xA5, sizeof(ofs));
    /* Check various opening options: */
    /* for reading only, */
    hFile = OpenFile(filename, &ofs, OF_READ);
    ok( hFile != HFILE_ERROR, "OpenFile failed on read\n" );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    ok( _lread(hFile, buffer, 10) == 3 && !memcmp(buffer, "abc", 3),
        "read back something else\n" );
    ok( _lwrite(hFile, "x", 1) == (UINT)HFILE_ERROR, "wrote to a file opened for reading\n" );
    ret = _lclose(hFile);
    ok( !ret, "_lclose() returns %d\n", ret );

    /* OF_REOPEN opens the file of the OFSTRUCT again. */
    hFile = OpenFile(NULL, &ofs, OF_REOPEN | OF_READ);
    ok( hFile != HFILE_ERROR, "OpenFile failed on reopen\n" );
    ok( _lread(hFile, buffer, 10) == 3, "reopened file reads something else\n" );
    _lclose(hFile);

    memset(&ofs, 0xA5, sizeof(ofs));
    /* for writing only, */
    hFile = OpenFile(filename, &ofs, OF_WRITE);
    ok( hFile != HFILE_ERROR, "OpenFile failed on write\n" );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    ret = _lclose(hFile);
    ok( !ret, "_lclose() returns %d\n", ret );

    memset(&ofs, 0xA5, sizeof(ofs));
    /* for reading and writing, */
    hFile = OpenFile(filename, &ofs, OF_READWRITE);
    ok( hFile != HFILE_ERROR, "OpenFile failed on read/write\n" );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );
    ret = _lclose(hFile);
    ok( !ret, "_lclose() returns %d\n", ret );

    memset(&ofs, 0xA5, sizeof(ofs));
    /* for checking file presence. */
    hFile = OpenFile(filename, &ofs, OF_EXIST);
    ok( hFile != HFILE_ERROR, "OpenFile failed on finding our created file\n" );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );

    /* OF_CREATE empties a file that is there. */
    hFile = OpenFile(filename, &ofs, OF_CREATE | OF_READWRITE);
    ok( hFile != HFILE_ERROR, "OpenFile failed to create an existing file\n" );
    ok( _llseek(hFile, 0, FILE_END) == 0, "OF_CREATE didn't empty the file\n" );
    _lclose(hFile);

    memset(&ofs, 0xA5, sizeof(ofs));
    /* Delete the file and make sure it doesn't exist anymore */
    hFile = OpenFile(filename, &ofs, OF_DELETE);
    ok( hFile != HFILE_ERROR, "OpenFile failed on delete (%d)\n", hFile );
    ok( ofs.cBytes == sizeof(OFSTRUCT), "OpenFile set ofs.cBytes to %d\n", ofs.cBytes );
    ok( ofs.nErrCode == 0 || broken(ofs.nErrCode != 0) /* win9x */,
        "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
    ok( lstrcmpi(ofs.szPathName, buff) == 0,
        "OpenFile returned '%s', but was expected to return '%s'\n",
        (LPSTR)ofs.szPathName, (LPSTR)buff );

    ok( get_attributes(filename) == -1, "file still there after OF_DELETE\n" );
    hFile = OpenFile(filename, &ofs, OF_DELETE);
    ok( hFile == HFILE_ERROR, "OF_DELETE of a deleted file returned %d\n", hFile );
    ok( ofs.nErrCode == 2, "OpenFile set ofs.nErrCode to %d\n", ofs.nErrCode );
}

/* DOS's calls from a Windows program, with far data in segments other
 * than DS. */
static void test_dos_calls(int through_dos3call)
{
    struct dosregs r;
    union REGS in, out;
    HGLOBAL hname, hdata, hdta;
    char FAR *name;
    char FAR *data;
    BYTE FAR *dta;
    WORD handle, i;
    DWORD version;
    static char dir[64];
    static char name1[13], name2[13];
    const char *how = through_dos3call ? "DOS3Call" : "INT 21h";

    /* The version, as GetVersion has it. */
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3000;
    dos_call(&r, through_dos3call);
    version = GetVersion();
    ok((r.ax & 0xff) == HIBYTE(HIWORD(version)) &&
       (r.ax >> 8) == LOBYTE(HIWORD(version)),
       "%s: DOS version %x, GetVersion %lx\n", (LPSTR)how, r.ax, version);
    ok((r.ax & 0xff) >= 3, "%s: DOS version %x\n", (LPSTR)how, r.ax);

    /* The current drive is C:. */
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x1900;
    dos_call(&r, through_dos3call);
    ok((r.ax & 0xff) == 2, "%s: current drive %x\n", (LPSTR)how, r.ax & 0xff);

    hname = GlobalAlloc(GMEM_FIXED | GMEM_ZEROINIT, 256);
    hdata = GlobalAlloc(GMEM_FIXED | GMEM_ZEROINIT, 0x2000);
    hdta = GlobalAlloc(GMEM_FIXED | GMEM_ZEROINIT, 128);
    name = GlobalLock(hname);
    data = GlobalLock(hdata);
    dta = GlobalLock(hdta);

    /* Create (3Ch), write (40h), seek (42h), read (3Fh), close (3Eh). */
    lstrcpy(name, "C:\\DOSFILE.TST");
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3c00;
    r.cx = 0;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: create failed, %x\n", (LPSTR)how, r.ax);
    handle = r.ax;

    for (i = 0; i < 0x1000; i++) data[i] = (char)(i * 7);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4000;
    r.bx = handle;
    r.cx = 0x1000;
    dos_set_far(&r.ds, &r.dx, data);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r) && r.ax == 0x1000, "%s: write gave %x, carry %d\n",
       (LPSTR)how, r.ax, CARRY(&r));

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4200;
    r.bx = handle;
    r.cx = 0;
    r.dx = 0x100;
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r) && r.ax == 0x100 && r.dx == 0, "%s: seek gave %x:%x\n",
       (LPSTR)how, r.dx, r.ax);

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3f00;
    r.bx = handle;
    r.cx = 0x1000;
    dos_set_far(&r.ds, &r.dx, data + 0x1000);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r) && r.ax == 0xf00, "%s: read gave %x, carry %d\n",
       (LPSTR)how, r.ax, CARRY(&r));
    ok(!_fmemcmp(data + 0x1000, data + 0x100, 0xf00),
       "%s: read something else\n", (LPSTR)how);

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3e00;
    r.bx = handle;
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: close failed, %x\n", (LPSTR)how, r.ax);

    /* The file's size, as _lopen and _llseek see it. */
    handle = _lopen(name, OF_READ);
    ok(handle != (WORD)HFILE_ERROR, "%s: _lopen of the file failed\n", (LPSTR)how);
    ok(_llseek(handle, 0, FILE_END) == 0x1000, "%s: the file has %ld bytes\n",
       (LPSTR)how, _llseek(handle, 0, FILE_END));
    _lclose(handle);

    /* A second file, and a search through the DTA (1Ah, 4Eh, 4Fh). */
    lstrcpy(name + 128, "C:\\DOSFILE2.TST");
    handle = _lcreat(name + 128, 0);
    _lwrite(handle, "12345", 5);
    _lclose(handle);

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x1a00;
    dos_set_far(&r.ds, &r.dx, dta);
    dos_call(&r, through_dos3call);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x2f00;
    dos_call(&r, through_dos3call);
    ok(r.es == SELECTOROF(dta) && r.bx == OFFSETOF(dta),
       "%s: DTA at %x:%x, set at %x:%x\n", (LPSTR)how, r.es, r.bx,
       SELECTOROF(dta), OFFSETOF(dta));

    lstrcpy(name + 64, "C:\\DOSFILE*.TST");
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4e00;
    r.cx = 0;
    dos_set_far(&r.ds, &r.dx, name + 64);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: find first failed, %x\n", (LPSTR)how, r.ax);
    lstrcpy(name1, (LPCSTR)(dta + 0x1e));
    ok(dta[0x15] == 0x20, "%s: found attributes %x\n", (LPSTR)how, dta[0x15]);
    i = !lstrcmp(name1, "DOSFILE.TST") ? 0x1000 : 5;
    ok(*(DWORD FAR *)(dta + 0x1a) == i, "%s: %s has %ld bytes\n", (LPSTR)how,
       (LPSTR)name1, *(DWORD FAR *)(dta + 0x1a));

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4f00;
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: find next failed, %x\n", (LPSTR)how, r.ax);
    lstrcpy(name2, (LPCSTR)(dta + 0x1e));
    ok((!lstrcmp(name1, "DOSFILE.TST") && !lstrcmp(name2, "DOSFILE2.TST")) ||
       (!lstrcmp(name2, "DOSFILE.TST") && !lstrcmp(name1, "DOSFILE2.TST")),
       "%s: found %s and %s\n", (LPSTR)how, (LPSTR)name1, (LPSTR)name2);

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4f00;
    dos_call(&r, through_dos3call);
    ok(CARRY(&r) && r.ax == 0x12, "%s: third find next gave %x, carry %d\n",
       (LPSTR)how, r.ax, CARRY(&r));

    /* Delete (41h) through far names. */
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4100;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: delete failed, %x\n", (LPSTR)how, r.ax);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4100;
    dos_set_far(&r.ds, &r.dx, name + 128);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: delete failed, %x\n", (LPSTR)how, r.ax);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4100;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(CARRY(&r) && r.ax == 2, "%s: second delete gave %x, carry %d\n",
       (LPSTR)how, r.ax, CARRY(&r));

    /* The current directory (47h), in and out of a directory of ours
     * (39h, 3Bh, 3Ah). */
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4700;
    r.dx = 0;
    _fmemset(dta, 0x55, 64);
    dos_set_far(&r.ds, &r.si, dta);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r) && dta[0] == 0, "%s: current directory \"%s\"\n",
       (LPSTR)how, (LPSTR)dta);

    lstrcpy(name, "C:\\DOSDIR16");
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3900;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: mkdir failed, %x\n", (LPSTR)how, r.ax);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3b00;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: chdir failed, %x\n", (LPSTR)how, r.ax);

    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x4700;
    r.dx = 3; /* C: */
    dos_set_far(&r.ds, &r.si, dta);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r) && !lstrcmp((LPCSTR)dta, "DOSDIR16"),
       "%s: current directory \"%s\"\n", (LPSTR)how, (LPSTR)dta);

    /* What intdosx sees, too. */
    in.x.ax = 0x4700;
    in.x.dx = 0;
    in.x.si = (WORD)(void NEAR *)dir;
    intdos(&in, &out);
    ok(!out.x.cflag && !lstrcmp(dir, "DOSDIR16"),
       "intdos: current directory \"%s\"\n", (LPSTR)dir);
    in.x.ax = 0x3000;
    intdos(&in, &out);
    ok(out.h.al == HIBYTE(HIWORD(version)), "intdos: DOS version %x\n",
       out.x.ax);

    lstrcpy(name, "\\");
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3b00;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: chdir \\ failed, %x\n", (LPSTR)how, r.ax);
    lstrcpy(name, "DOSDIR16");
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3a00;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(!CARRY(&r), "%s: rmdir failed, %x\n", (LPSTR)how, r.ax);
    _fmemset(&r, 0, sizeof(r));
    r.ax = 0x3b00;
    dos_set_far(&r.ds, &r.dx, name);
    dos_call(&r, through_dos3call);
    ok(CARRY(&r) && r.ax == 3, "%s: chdir to a removed directory gave %x\n",
       (LPSTR)how, r.ax);

    GlobalUnlock(hname);
    GlobalUnlock(hdata);
    GlobalUnlock(hdta);
    GlobalFree(hname);
    GlobalFree(hdata);
    GlobalFree(hdta);
}

START_TEST(file)
{
    test__hread(  );
    test__hwrite(  );
    test__lclose(  );
    test__lcreat(  );
    test__llseek(  );
    test__llopen(  );
    test__lread(  );
    test__lwrite(  );
    test_GetTempFileName();
    test_GetDriveType();
    test_OpenFile();
    test_dos_calls(1);
    test_dos_calls(0);
}
