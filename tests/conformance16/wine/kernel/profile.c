/*
 * Unit tests for profile functions
 *
 * Copyright (c) 2003 Stefan Leichter
 *
 * Ported to Win16 from Wine's dlls/kernel32/tests/profile.c: the ANSI
 * GetPrivateProfileInt, GetPrivateProfileString and
 * WritePrivateProfileString tests, with files made by _lcreat and
 * removed by OpenFile(OF_DELETE), and the same for WIN.INI through
 * GetProfileInt, GetProfileString and WriteProfileString. Left out are
 * the Unicode functions, GetPrivateProfileSection and
 * GetPrivateProfileSectionNames and the NULL section that lists the
 * sections, which are Win32's, and the tests of sharing, of
 * delete-on-close files and of the cache, which need CreateFile. Where
 * Windows 9x and NT differ, Windows 9x's answer (whose profile code is
 * KRNL386's) is accepted. The checks of WritePrivateProfileString read
 * the file back with GetPrivateProfileString instead of comparing its
 * bytes, as Wine does for Windows 9x, which lays the file out
 * differently. GetPrivateProfileInt returns 16 bits here.
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

#define KEY      "ProfileInt"
#define SECTION  "Test"
#define TESTFILE ".\\testwine.ini"
#define TESTFILE2 ".\\testwin2.ini"
#define MAX_PATH 260

static void delete_file(LPCSTR name)
{
    OFSTRUCT of;
    OpenFile(name, &of, OF_DELETE);
}

static int file_exists(LPCSTR name)
{
    OFSTRUCT of;
    return OpenFile(name, &of, OF_EXIST) != HFILE_ERROR;
}

static void create_test_file(LPCSTR name, LPCSTR data, UINT size)
{
    HFILE hfile;

    hfile = _lcreat(name, 0);
    ok(hfile != HFILE_ERROR, "cannot create %s\n", name);
    if (hfile == HFILE_ERROR) return;
    ok(_lwrite(hfile, data, size) == size, "cannot write %s\n", name);
    _lclose(hfile);
}

/* Whether the file holds `data` somewhere. */
static int file_contains(LPCSTR path, LPCSTR data)
{
    HFILE file;
    static char buf[2048];
    UINT size, len, i;

    file = _lopen(path, OF_READ);
    if (file == HFILE_ERROR)
        return 0;
    size = _lread(file, buf, sizeof(buf) - 1);
    _lclose(file);
    if (size == (UINT)HFILE_ERROR) return 0;
    len = strlen(data);
    for (i = 0; i + len <= size; i++)
        if (!memcmp(buf + i, data, len)) return 1;
    return 0;
}

struct _profileInt {
    LPCSTR section;
    LPCSTR key;
    LPCSTR value;
    LPCSTR iniFile;
    int defaultVal;
    UINT result;
    UINT result9x;
};

static void test_profile_int(void)
{
    static const struct _profileInt profileInt[]={
         { NULL,    NULL, NULL,          TESTFILE, -1, 65535U, 0},
         { NULL,    NULL, NULL,          TESTFILE,  1, 1     , 0},
         { SECTION, NULL, NULL,          TESTFILE, -1, 65535U, 0},
         { SECTION, NULL, NULL,          TESTFILE,  1, 1     , 0},
         { NULL,    KEY,  NULL,          TESTFILE, -1, 65535U, 0}, /*  5 */
         { NULL,    KEY,  NULL,          TESTFILE,  1, 1     , 0},
         { SECTION, KEY,  NULL,          TESTFILE, -1, 65535U, 65535U},
         { SECTION, KEY,  NULL,          TESTFILE,  1, 1     , 1},
         { SECTION, KEY,  "-1",          TESTFILE, -1, 65535U, 65535U},
         { SECTION, KEY,  "-1",          TESTFILE,  1, 65535U, 65535U}, /* 10 */
         { SECTION, KEY,  "1",           TESTFILE, -1, 1     , 1},
         { SECTION, KEY,  "1",           TESTFILE,  1, 1     , 1},
         { SECTION, KEY,  "+1",          TESTFILE, -1, 1     , 0},
         { SECTION, KEY,  "+1",          TESTFILE,  1, 1     , 0},
         { SECTION, KEY,  "4294967296",  TESTFILE, -1, 0     , 0}, /* 15 */
         { SECTION, KEY,  "4294967296",  TESTFILE,  1, 0     , 0},
         { SECTION, KEY,  "4294967297",  TESTFILE, -1, 1     , 1},
         { SECTION, KEY,  "4294967297",  TESTFILE,  1, 1     , 1},
         { SECTION, KEY,  "-4294967297", TESTFILE, -1, 65535U, 65535U},
         { SECTION, KEY,  "-4294967297", TESTFILE,  1, 65535U, 65535U}, /* 20 */
         { SECTION, KEY,  "42A94967297", TESTFILE, -1, 42    , 42},
         { SECTION, KEY,  "42A94967297", TESTFILE,  1, 42    , 42},
         { SECTION, KEY,  "B4294967297", TESTFILE, -1, 0     , 0},
         { SECTION, KEY,  "B4294967297", TESTFILE,  1, 0     , 0},
    };
    int i, num_test = (sizeof(profileInt)/sizeof(struct _profileInt));
    UINT res;

    delete_file( TESTFILE);

    for (i=0; i < num_test; i++) {
        if (profileInt[i].value)
            WritePrivateProfileString(SECTION, KEY, profileInt[i].value,
                                      profileInt[i].iniFile);

       res = GetPrivateProfileInt(profileInt[i].section, profileInt[i].key,
                 profileInt[i].defaultVal, profileInt[i].iniFile);
       ok((res == profileInt[i].result) || (res == profileInt[i].result9x),
	    "test<%02d>: ret<%05u> exp<%05u><%05u>\n",
            i + 1, res, profileInt[i].result, profileInt[i].result9x);
    }

    delete_file( TESTFILE);
}

static void test_profile_string(void)
{
    int ret;
    char buf[100];
    char *p;
    /* test that lines without an '=' will not be enumerated */
    /* in the case below, name2 is a key while name3 is not. */
    static const char content[]="[s]\r\nname1=val1\r\nname2=\r\nname3\r\nname4=val4\r\n";
    delete_file( TESTFILE2);
    create_test_file( TESTFILE2, content, sizeof(content) - 1);

    /* enumerate the keys */
    ret=GetPrivateProfileString( "s", NULL, "", buf, sizeof(buf),
        TESTFILE2);
    for( p = buf + strlen(buf) + 1; *p;p += strlen(p)+1)
        p[-1] = ',';
    /* and test */
    ok( ret == 18 && !strcmp( buf, "name1,name2,name4"), "wrong keys returned(%d): %s\n", ret,
            buf);

    /* add a new key to test that the file is quite usable */
    WritePrivateProfileString( "s", "name5", "val5", TESTFILE2);
    ret=GetPrivateProfileString( "s", NULL, "", buf, sizeof(buf),
        TESTFILE2);
    for( p = buf + strlen(buf) + 1; *p;p += strlen(p)+1)
        p[-1] = ',';
    ok( ret == 24 && !strcmp( buf, "name1,name2,name4,name5"), "wrong keys returned(%d): %s\n",
            ret, buf);

    delete_file( TESTFILE2);
}

static BOOL emptystr_ok(char emptystr[MAX_PATH])
{
    int i;

    for(i = 0;i < MAX_PATH;++i)
        if(emptystr[i] != 0)
        {
            trace("emptystr[%d] = %d\n",i,emptystr[i]);
            return FALSE;
        }

    return TRUE;
}

static void test_GetPrivateProfileString(const char *content, const char *descript)
{
    int ret;
    static char buf[MAX_PATH];
    static char def_val[MAX_PATH];
    static char path[MAX_PATH];
    /* NT series crashes on r/o empty strings, so pass an r/w
       empty string and check for modification */
    static char emptystr[MAX_PATH];
    LPCSTR tempfile;

    static const char filename[] = ".\\winetest.ini";

    trace("test_GetPrivateProfileString: %s\n", descript);

    create_test_file(filename, content, lstrlen(content));

    /* lpAppName is empty */
    memset(buf, 0xc, sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString(emptystr, "name1", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpAppName is missing */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("notasection", "name1", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);

    /* lpAppName is empty, lpDefault is NULL */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString(emptystr, "name1", NULL,
                                   buf, MAX_PATH, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, "") ||
       broken(!lstrcmp(buf, "kumquat")), /* Win9x, WinME */
       "Expected \"\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpAppName is empty, lpDefault is empty */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString(emptystr, "name1", "",
                                   buf, MAX_PATH, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, ""), "Expected \"\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpAppName is empty, lpDefault has trailing blank characters */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    /* lpDefault must be writable (trailing blanks are removed inplace in win9x) */
    lstrcpy(def_val, "default  ");
    ret = GetPrivateProfileString(emptystr, "name1", def_val,
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpAppName is empty, many blank characters in lpDefault */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    /* lpDefault must be writable (trailing blanks are removed inplace in win9x) */
    lstrcpy(def_val, "one two  ");
    ret = GetPrivateProfileString(emptystr, "name1", def_val,
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "one two"), "Expected \"one two\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpAppName is empty, blank character but not trailing in lpDefault */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString(emptystr, "name1", "one two",
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "one two"), "Expected \"one two\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "AppName modified\n");

    /* lpKeyName is NULL */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", NULL, "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 18, "Expected 18, got %d\n", ret);
    ok(!memcmp(buf, "name1\0name2\0name4\0", ret + 1),
       "Expected \"name1\\0name2\\0name4\\0\", got \"%s\"\n", (LPSTR)buf);

    /* lpKeyName is empty */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", emptystr, "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "KeyName modified\n");

    /* lpKeyName is missing */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "notakey", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);

    /* lpKeyName is empty, lpDefault is NULL */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", emptystr, NULL,
                                   buf, MAX_PATH, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, "") ||
       broken(!lstrcmp(buf, "kumquat")), /* Win9x, WinME */
       "Expected \"\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "KeyName modified\n");

    /* lpKeyName is empty, lpDefault is empty */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", emptystr, "",
                                   buf, MAX_PATH, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, ""), "Expected \"\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "KeyName modified\n");

    /* lpKeyName is empty, lpDefault has trailing blank characters */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    /* lpDefault must be writable (trailing blanks are removed inplace in win9x) */
    lstrcpy(def_val, "default  ");
    ret = GetPrivateProfileString("section1", emptystr, def_val,
                                   buf, MAX_PATH, filename);
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);
    ok(emptystr_ok(emptystr), "KeyName modified\n");

    /* lpFileName is empty */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, MAX_PATH, "");
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);

    /* lpFileName is nonexistent */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, MAX_PATH, "nonexist");
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);

    /* nSize is 0 */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, 0, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, "kumquat"), "Expected buf to be unchanged, got \"%s\"\n", (LPSTR)buf);

    /* nSize is exact size of output */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, 4, filename);
    ok(ret == 3, "Expected 3, got %d\n", ret);
    ok(!lstrcmp(buf, "val"), "Expected \"val\", got \"%s\"\n", (LPSTR)buf);

    /* nSize has room for NULL terminator */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, 5, filename);
    ok(ret == 4, "Expected 4, got %d\n", ret);
    ok(!lstrcmp(buf, "val1"), "Expected \"val1\", got \"%s\"\n", (LPSTR)buf);

    /* output is 1 character */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name4", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 1, "Expected 1, got %d\n", ret);
    ok(!lstrcmp(buf, "a"), "Expected \"a\", got \"%s\"\n", (LPSTR)buf);

    /* output is 1 character, no room for NULL terminator */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name4", "default",
                                   buf, 1, filename);
    ok(ret == 0, "Expected 0, got %d\n", ret);
    ok(!lstrcmp(buf, ""), "Expected \"\", got \"%s\"\n", (LPSTR)buf);

    /* lpKeyName is NULL, not enough room for final key name */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", NULL, "default",
                                   buf, 16, filename);
    ok(ret == 14, "Expected 14, got %d\n", ret);
    ok(!memcmp(buf, "name1\0name2\0na\0\0", ret + 2) ||
       broken(!memcmp(buf, "name1\0name2\0n\0\0", ret + 1)), /* Win9x, WinME */
       "Expected \"name1\\0name2\\0na\\0\\0\", got \"%s\"\n", (LPSTR)buf);

    /* key value has quotation marks which are stripped */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name2", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 4, "Expected 4, got %d\n", ret);
    ok(!lstrcmp(buf, "val2"), "Expected \"val2\", got \"%s\"\n", (LPSTR)buf);

    /* case does not match */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "NaMe1", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 4, "Expected 4, got %d\n", ret);
    ok(!lstrcmp(buf, "val1"), "Expected \"val1\", got \"%s\"\n", (LPSTR)buf);

    /* only filename is used */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "NaMe1", "default",
                                   buf, MAX_PATH, "winetest.ini");
    ok(ret == 7, "Expected 7, got %d\n", ret);
    ok(!lstrcmp(buf, "default"), "Expected \"default\", got \"%s\"\n", (LPSTR)buf);

    /* A file of the Windows directory instead. */
    ret = GetWindowsDirectory(path, MAX_PATH);
    ok(ret > 0 && ret < MAX_PATH, "GetWindowsDirectory gave %d\n", ret);
    if (path[lstrlen(path) - 1] != '\\') lstrcat(path, "\\");
    lstrcat(path, "WTPROF.INI");
    tempfile = "wtprof.ini";
    create_test_file(path, content, lstrlen(content));

    /* only filename is used, file exists in windows directory */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "NaMe1", "default",
                                   buf, MAX_PATH, tempfile);
    ok(ret == 4, "Expected 4, got %d\n", ret);
    ok(!lstrcmp(buf, "val1"), "Expected \"val1\", got \"%s\"\n", (LPSTR)buf);

    /* successful case */
    memset(buf, 0xc,sizeof(buf));
    lstrcpy(buf, "kumquat");
    ret = GetPrivateProfileString("section1", "name1", "default",
                                   buf, MAX_PATH, filename);
    ok(ret == 4, "Expected 4, got %d\n", ret);
    ok(!lstrcmp(buf, "val1"), "Expected \"val1\", got \"%s\"\n", (LPSTR)buf);

 /* Existing section with no keys in an existing file */
    memset(buf, 0xc,sizeof(buf));
    ret=GetPrivateProfileString("section2", "DoesntExist", "",
                                 buf, MAX_PATH, filename);
    ok( ret == 0, "expected return size 0, got %d\n", ret );
    ok(!lstrcmp(buf, ""), "Expected \"\", got \"%s\"\n", (LPSTR)buf);

    delete_file(path);
    delete_file(filename);
}

static void test_WritePrivateProfileString(void)
{
    BOOL ret;
    int len;
    static char path[MAX_PATH];
    static char buf[MAX_PATH];
    static const char file[] = "C:\\WTWRITE.INI";

    delete_file(file);

    /* empty lpString */
    ret = WritePrivateProfileString("App", "key", "", file);
    ok(ret == TRUE ||
       broken(!ret), /* Win9x and WinME */
       "Expected TRUE, got %d\n", ret);
    if (ret)
    {
        ok(file_contains(file, "[App]"), "File doesn't match\n");
        ok(file_contains(file, "key="), "File doesn't match\n");
        len = GetPrivateProfileString("App", "key", "default", buf,
                                      sizeof(buf), file);
        ok(len == 0 && !buf[0], "read back %d \"%s\"\n", len, (LPSTR)buf);
    }
    delete_file(file);

    /* The resulting file will be  X:\\%WINDIR%\\win1.tmp */
    GetWindowsDirectory(path, MAX_PATH);
    if (path[lstrlen(path) - 1] != '\\') lstrcat(path, "\\");
    lstrcat(path, "WIN1.TMP");
    delete_file(path);

    /* relative path in lpFileName */
    ret = WritePrivateProfileString("App", "key", "string", "win1.tmp");
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    ok(file_exists(path), "%s wasn't made\n", (LPSTR)path);
    ok(file_contains(path, "[App]") && file_contains(path, "key=string"),
       "File doesn't match\n");
    ok(!file_exists("C:\\WIN1.TMP"), "win1.tmp made in the current directory\n");
    delete_file(path);

    /* build up an INI file */
    WritePrivateProfileString("App1", "key1", "string1", file);
    WritePrivateProfileString("App1", "key2", "string2", file);
    WritePrivateProfileString("App1", "key3", "string3", file);
    WritePrivateProfileString("App2", "key4", "string4", file);

    /* make an addition and verify the INI */
    ret = WritePrivateProfileString("App3", "key5", "string5", file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    len = GetPrivateProfileString("App1", NULL, "", buf, sizeof(buf), file);
    ok(len == 15 && !memcmp(buf, "key1\0key2\0key3\0\0", 16),
       "App1 has keys (%d) \"%s\"\n", len, (LPSTR)buf);
    len = GetPrivateProfileString("App2", "key4", "", buf, sizeof(buf), file);
    ok(len == 7 && !lstrcmp(buf, "string4"), "key4 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    len = GetPrivateProfileString("App3", "key5", "", buf, sizeof(buf), file);
    ok(len == 7 && !lstrcmp(buf, "string5"), "key5 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    ok(file_contains(file, "[App1]\r\nkey1=string1\r\nkey2=string2\r\n"
                     "key3=string3\r\n"), "File doesn't match\n");

    /* rewriting a key changes its value in place */
    ret = WritePrivateProfileString("App1", "KEY2", "other", file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    len = GetPrivateProfileString("App1", "key2", "", buf, sizeof(buf), file);
    ok(len == 5 && !lstrcmp(buf, "other"), "key2 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    len = GetPrivateProfileString("App1", NULL, "", buf, sizeof(buf), file);
    ok((len == 15 && !memcmp(buf, "key1\0key2\0key3\0\0", 16)) ||
       broken(len == 15 && !memcmp(buf, "key1\0KEY2\0key3\0\0", 16)),
       "App1 has keys (%d) \"%s\"\n", len, (LPSTR)buf);

    /* lpString is NULL, key2 key is deleted */
    ret = WritePrivateProfileString("App1", "key2", NULL, file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    len = GetPrivateProfileString("App1", NULL, "", buf, sizeof(buf), file);
    ok(len == 10 && !memcmp(buf, "key1\0key3\0\0", 11),
       "App1 has keys (%d) \"%s\"\n", len, (LPSTR)buf);
    len = GetPrivateProfileString("App1", "key2", "gone", buf, sizeof(buf),
                                  file);
    ok(len == 4 && !lstrcmp(buf, "gone"), "key2 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    ok(!file_contains(file, "key2"), "key2 still in the file\n");

    /* try to delete key2 again */
    ret = WritePrivateProfileString("App1", "key2", NULL, file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    len = GetPrivateProfileString("App1", NULL, "", buf, sizeof(buf), file);
    ok(len == 10 && !memcmp(buf, "key1\0key3\0\0", 11),
       "App1 has keys (%d) \"%s\"\n", len, (LPSTR)buf);

    /* lpKeyName is NULL, App1 section is deleted */
    ret = WritePrivateProfileString("App1", NULL, "string1", file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    len = GetPrivateProfileString("App1", NULL, "", buf, sizeof(buf), file);
    ok(len == 0, "App1 has keys (%d) \"%s\"\n", len, (LPSTR)buf);
    len = GetPrivateProfileString("App1", "key1", "gone", buf, sizeof(buf),
                                  file);
    ok(len == 4 && !lstrcmp(buf, "gone"), "key1 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    ok(!file_contains(file, "[App1]"), "App1 still in the file\n");
    ok(file_contains(file, "[App2]"), "App2 not in the file\n");

    /* lpString is not needed to delete a section */
    ret = WritePrivateProfileString("App2", NULL, NULL, file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    ok(!file_contains(file, "[App2]"), "App2 still in the file\n");
    len = GetPrivateProfileString("App3", "key5", "", buf, sizeof(buf), file);
    ok(len == 7 && !lstrcmp(buf, "string5"), "key5 is (%d) \"%s\"\n", len,
       (LPSTR)buf);

    /* leave just the section */
    ret = WritePrivateProfileString("App3", "key5", NULL, file);
    ok(ret == TRUE, "Expected TRUE, got %d\n", ret);
    ok(file_contains(file, "[App3]"), "App3 not in the file\n");
    ok(!file_contains(file, "key5"), "key5 still in the file\n");

    /* the numbers GetPrivateProfileInt reads */
    WritePrivateProfileString("Num", "b", "007", file);
    WritePrivateProfileString("Num", "c", "65535", file);
    WritePrivateProfileString("Num", "d", "32768", file);
    ok(GetPrivateProfileInt("Num", "b", 5, file) == 7, "b is %u\n",
       GetPrivateProfileInt("Num", "b", 5, file));
    ok(GetPrivateProfileInt("Num", "c", 5, file) == 65535U, "c is %u\n",
       GetPrivateProfileInt("Num", "c", 5, file));
    ok(GetPrivateProfileInt("num", "D", 5, file) == 32768U, "d is %u\n",
       GetPrivateProfileInt("num", "D", 5, file));
    ok(GetPrivateProfileInt("Num", "e", 5, file) == 5, "e is %u\n",
       GetPrivateProfileInt("Num", "e", 5, file));
    delete_file(file);
}

/* WIN.INI, in the Windows directory. */
static void test_profile_win_ini(void)
{
    static char buf[MAX_PATH];
    static char path[MAX_PATH];
    int len;
    BOOL ret;

    GetWindowsDirectory(path, MAX_PATH);
    if (path[lstrlen(path) - 1] != '\\') lstrcat(path, "\\");
    lstrcat(path, "WIN.INI");

    ret = WriteProfileString("WineTest16", "key1", "value one");
    ok(ret, "WriteProfileString failed\n");
    ret = WriteProfileString("WineTest16", "key2", "42");
    ok(ret, "WriteProfileString failed\n");

    len = GetProfileString("WineTest16", "key1", "default", buf, sizeof(buf));
    ok(len == 9 && !lstrcmp(buf, "value one"), "key1 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    len = GetProfileString("winetest16", "KEY1", "default", buf, sizeof(buf));
    ok(len == 9 && !lstrcmp(buf, "value one"), "KEY1 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    ok(GetProfileInt("WineTest16", "key2", 0) == 42, "key2 is %u\n",
       GetProfileInt("WineTest16", "key2", 0));
    ok(GetProfileInt("WineTest16", "key3", 17) == 17, "key3 is %u\n",
       GetProfileInt("WineTest16", "key3", 17));
    len = GetProfileString("WineTest16", "key3", "default", buf, sizeof(buf));
    ok(len == 7 && !lstrcmp(buf, "default"), "key3 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    len = GetProfileString("WineTest16", NULL, "", buf, sizeof(buf));
    ok(len == 10 && !memcmp(buf, "key1\0key2\0\0", 11),
       "WineTest16 has keys (%d) \"%s\"\n", len, (LPSTR)buf);

    /* The same through its name. */
    len = GetPrivateProfileString("WineTest16", "key1", "default", buf,
                                  sizeof(buf), "win.ini");
    ok(len == 9 && !lstrcmp(buf, "value one"), "key1 in win.ini is (%d) \"%s\"\n",
       len, (LPSTR)buf);
    len = GetPrivateProfileString("WineTest16", "key2", "default", buf,
                                  sizeof(buf), path);
    ok(len == 2 && !lstrcmp(buf, "42"), "key2 in %s is (%d) \"%s\"\n",
       (LPSTR)path, len, (LPSTR)buf);
    ok(file_contains(path, "[WineTest16]"), "%s has no [WineTest16]\n",
       (LPSTR)path);
    ok(file_contains(path, "key1=value one"), "%s has no key1\n", (LPSTR)path);

    /* And written through its name. */
    ret = WritePrivateProfileString("WineTest16", "key3", "three", "win.ini");
    ok(ret, "WritePrivateProfileString(win.ini) failed\n");
    len = GetProfileString("WineTest16", "key3", "default", buf, sizeof(buf));
    ok(len == 5 && !lstrcmp(buf, "three"), "key3 is (%d) \"%s\"\n", len,
       (LPSTR)buf);

    /* Deleting a key, then the section. */
    ret = WriteProfileString("WineTest16", "key1", NULL);
    ok(ret, "WriteProfileString(NULL) failed\n");
    len = GetProfileString("WineTest16", "key1", "gone", buf, sizeof(buf));
    ok(len == 4 && !lstrcmp(buf, "gone"), "key1 is (%d) \"%s\"\n", len,
       (LPSTR)buf);
    ret = WriteProfileString("WineTest16", NULL, NULL);
    ok(ret, "WriteProfileString(NULL, NULL) failed\n");
    len = GetProfileString("WineTest16", NULL, "", buf, sizeof(buf));
    ok(len == 0, "WineTest16 has keys (%d) \"%s\"\n", len, (LPSTR)buf);
    ok(!file_contains(path, "[WineTest16]"), "%s still has [WineTest16]\n",
       (LPSTR)path);
}

START_TEST(profile)
{
    test_profile_int();
    test_profile_string();
    test_GetPrivateProfileString(
        "[section1]\r\n"
        "name1=val1\r\n"
        "name2=\"val2\"\r\n"
        "name3\r\n"
        "name4=a\r\n"
        "[section2]\r\n",
        "CR+LF");
    test_WritePrivateProfileString();
    test_profile_win_ini();
}
