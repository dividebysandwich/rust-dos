/*
 * Unit tests for module/DLL/library API
 *
 * Copyright (c) 2004 Eric Pouech
 *
 * Ported to Win16 from Wine's dlls/kernel32/tests/module.c: the ANSI
 * GetModuleFileName and LoadLibrary tests, on the system's modules and
 * system DLLs (MMSYSTEM, COMMDLG); a DLL that isn't there gives an
 * error code below 32 here. Left out are the Unicode functions,
 * LoadLibraryEx, SetLastError and GetLastError. Added are the module
 * and task functions of Windows 3.1 that programs start with:
 * GetModuleHandle, GetModuleUsage, GetProcAddress by name and by
 * ordinal on KERNEL, USER and GDI, GetVersion, GetWinFlags,
 * GetCurrentTask and GetCurrentPDB, KERNEL's and USER's string
 * functions (lstrcpy and the others, AnsiUpper and the others), Catch
 * and Throw, and WinMain's command line.
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

#define MAX_PATH 260

static HINSTANCE this_instance;

/* The instance handle is our DGROUP's. */
static HINSTANCE get_instance(void)
{
    WORD ds_;
    _asm mov ds_, ds
    return (HINSTANCE)LOWORD(GlobalHandle(ds_));
}

/* The file name part of a path. */
static LPCSTR file_part(LPCSTR path)
{
    LPCSTR p = path + lstrlen(path);
    while (p > path && p[-1] != '\\' && p[-1] != ':') p--;
    return p;
}

static void testGetModuleFileName(HINSTANCE hMod, const char *name)
{
    static char bufA[MAX_PATH];
    int len1A, len2A;

    /* first test, with enough space in buffer */
    memset(bufA, '-', sizeof(bufA));
    len1A = GetModuleFileName(hMod, bufA, sizeof(bufA));
    ok(len1A > 0, "Getting module filename for handle %x\n", hMod);

    ok(len1A == strlen(bufA), "Unexpected length of GetModuleFilename (%d/%d)\n", len1A, lstrlen(bufA));
    if (name)
        ok(!lstrcmpi(file_part(bufA), name), "module %x is %s, not %s\n",
           hMod, (LPSTR)bufA, (LPSTR)name);
    ok(bufA[1] == ':' && bufA[2] == '\\', "module %x's file %s isn't a full path\n",
       hMod, (LPSTR)bufA);

    /* second test with a buffer too small */
    memset(bufA, '-', sizeof(bufA));
    len2A = GetModuleFileName(hMod, bufA, len1A / 2);
    ok(len2A > 0, "Getting module filename for handle %x\n", hMod);

    ok(len1A / 2 == len2A ||
       len1A / 2 == len2A + 1, /* Win9x */
       "Correct length in GetModuleFilename with buffer too small (%d/%d)\n", len1A / 2, len2A);
    ok(bufA[len1A / 2 - 1] == 0 || bufA[len1A / 2] == '-',
       "GetModuleFileName wrote past the buffer\n");
}

static void testLoadLibrary(void)
{
    HINSTANCE hModule, hModule1;
    HMODULE module;
    FARPROC fp;
    int usage;

    module = GetModuleHandle("MMSYSTEM");
    usage = module ? GetModuleUsage(module) : 0;

    hModule = LoadLibrary("mmsystem.dll");
    ok( hModule >= (HINSTANCE)32, "mmsystem.dll should be loadable (%x)\n", hModule);
    if (hModule < (HINSTANCE)32) return;
    module = GetModuleHandle("MMSYSTEM");
    ok( module != NULL, "GetModuleHandle(MMSYSTEM) failed\n");
    ok( GetModuleHandle("mmsystem.dll") == module || broken(!GetModuleHandle("mmsystem.dll")),
        "GetModuleHandle(mmsystem.dll) %x\n", GetModuleHandle("mmsystem.dll"));
    ok( GetModuleUsage(hModule) == usage + 1, "MMSYSTEM used %d times, %d before\n",
        GetModuleUsage(hModule), usage);

    fp = GetProcAddress(hModule, "sndPlaySound");
    ok( fp != NULL, "sndPlaySound should be there\n");
    ok( GetProcAddress(hModule, MAKEINTRESOURCE(2)) == fp,
        "sndPlaySound isn't MMSYSTEM.2\n");
    fp = GetProcAddress(hModule, "timeGetTime");
    ok( fp != NULL, "timeGetTime should be there\n");
    ok( GetProcAddress(hModule, MAKEINTRESOURCE(607)) == fp,
        "timeGetTime isn't MMSYSTEM.607\n");

    hModule1 = LoadLibrary("MMSYSTEM.DLL");
    ok( hModule1 == hModule, "Loaded wrong module (%x, not %x)\n", hModule1, hModule);
    ok( GetModuleUsage(hModule) == usage + 2, "MMSYSTEM used %d times, %d before\n",
        GetModuleUsage(hModule), usage);
    FreeLibrary(hModule1);
    ok( GetModuleUsage(hModule) == usage + 1, "MMSYSTEM used %d times, %d before\n",
        GetModuleUsage(hModule), usage);
    FreeLibrary(hModule);
    if (usage)
        ok( GetModuleUsage(hModule) == usage, "MMSYSTEM used %d times, %d before\n",
            GetModuleUsage(hModule), usage);
    else
        ok( GetModuleHandle("MMSYSTEM") == NULL, "MMSYSTEM still loaded\n");

    /* COMMDLG isn't loaded until a program asks for it. */
    module = GetModuleHandle("COMMDLG");
    usage = module ? GetModuleUsage(module) : 0;
    hModule = LoadLibrary("COMMDLG.DLL");
    ok( hModule >= (HINSTANCE)32, "commdlg.dll should be loadable (%x)\n", hModule);
    if (hModule < (HINSTANCE)32) return;
    ok( GetModuleHandle("COMMDLG") != NULL, "GetModuleHandle(COMMDLG) failed\n");
    ok( GetModuleUsage(hModule) == usage + 1, "COMMDLG used %d times, %d before\n",
        GetModuleUsage(hModule), usage);
    fp = GetProcAddress(hModule, "GetOpenFileName");
    ok( fp != NULL, "GetOpenFileName should be there\n");
    ok( GetProcAddress(hModule, MAKEINTRESOURCE(1)) == fp,
        "GetOpenFileName isn't COMMDLG.1\n");
    testGetModuleFileName(hModule, "COMMDLG.DLL");
    FreeLibrary(hModule);
    if (!usage)
        ok( GetModuleHandle("COMMDLG") == NULL, "COMMDLG still loaded\n");
}

static void testLoadLibrary_Wrong(void)
{
    HINSTANCE hModule;

    /* Try to load a nonexistent dll */
    hModule = LoadLibrary("non_ex_p.dll");
    ok( hModule < (HINSTANCE)32, "non_ex_p.dll should be not loadable\n");
    ok( hModule == (HINSTANCE)2, "Expected 2 (file not found), got %d\n", hModule);
    hModule = LoadLibrary("C:\\NODIR\\non_ex_p.dll");
    ok( hModule == (HINSTANCE)3 || broken(hModule == (HINSTANCE)2),
        "Expected 3 (path not found), got %d\n", hModule);
}

static void test_module_handles(void)
{
    HMODULE kernel, user, gdi;

    kernel = GetModuleHandle("KERNEL");
    user = GetModuleHandle("USER");
    gdi = GetModuleHandle("GDI");
    ok(kernel != NULL, "GetModuleHandle(KERNEL) failed\n");
    ok(user != NULL, "GetModuleHandle(USER) failed\n");
    ok(gdi != NULL, "GetModuleHandle(GDI) failed\n");
    ok(kernel != user && user != gdi && kernel != gdi,
       "modules %x %x %x\n", kernel, user, gdi);
    ok(GetModuleHandle("kernel") == kernel, "GetModuleHandle(kernel) %x\n",
       GetModuleHandle("kernel"));
    ok(GetModuleHandle("NOSUCHMD") == NULL, "GetModuleHandle(NOSUCHMD) %x\n",
       GetModuleHandle("NOSUCHMD"));
    /* Our own module, by its name and from our instance. */
    ok(GetModuleHandle("MODULE") != NULL, "GetModuleHandle(MODULE) failed\n");
    ok(GetModuleHandle((LPCSTR)MAKELP(0, this_instance)) == GetModuleHandle("MODULE"),
       "GetModuleHandle of the instance %x gave %x, not %x\n", this_instance,
       GetModuleHandle((LPCSTR)MAKELP(0, this_instance)), GetModuleHandle("MODULE"));

    ok(GetModuleUsage(this_instance) == 1, "GetModuleUsage of ourselves %d\n",
       GetModuleUsage(this_instance));
    ok(GetModuleUsage(kernel) >= 1, "GetModuleUsage(KERNEL) %d\n",
       GetModuleUsage(kernel));

    testGetModuleFileName(this_instance, "MODULE.EXE");
    testGetModuleFileName(kernel, NULL);
    testGetModuleFileName(user, "USER.EXE");
    testGetModuleFileName(gdi, "GDI.EXE");
    {
        static char buf[MAX_PATH];
        static char sys[MAX_PATH];
        int len;
        GetModuleFileName(kernel, buf, sizeof(buf));
        ok(!lstrcmpi(file_part(buf), "KRNL386.EXE") ||
           !lstrcmpi(file_part(buf), "KRNL286.EXE"),
           "KERNEL is %s\n", (LPSTR)buf);
        /* The system's modules are in the system directory. */
        len = GetSystemDirectory(sys, sizeof(sys));
        ok(len > 0, "GetSystemDirectory failed\n");
        ok(!_fstrnicmp(buf, sys, len) && buf[len] == '\\',
           "KERNEL is %s, the system directory %s\n", (LPSTR)buf, (LPSTR)sys);
        GetModuleFileName(this_instance, buf, sizeof(buf));
        ok(!lstrcmpi(buf, "C:\\MODULE.EXE"), "we are %s\n", (LPSTR)buf);
    }
}

static void test_GetProcAddress(void)
{
    HMODULE kernel, user, gdi;
    FARPROC fp;

    kernel = GetModuleHandle("KERNEL");
    user = GetModuleHandle("USER");
    gdi = GetModuleHandle("GDI");

    /* By name, which isn't case sensitive, and by ordinal; what our
     * imports were bound to. */
    fp = GetProcAddress(kernel, "GlobalAlloc");
    ok(fp == (FARPROC)GlobalAlloc, "GlobalAlloc at %lx, imported at %lx\n",
       (DWORD)fp, (DWORD)(FARPROC)GlobalAlloc);
    ok(GetProcAddress(kernel, "GLOBALALLOC") == fp,
       "GetProcAddress(GLOBALALLOC) %lx\n", (DWORD)GetProcAddress(kernel, "GLOBALALLOC"));
    ok(GetProcAddress(kernel, MAKEINTRESOURCE(15)) == fp,
       "KERNEL.15 at %lx, GlobalAlloc at %lx\n",
       (DWORD)GetProcAddress(kernel, MAKEINTRESOURCE(15)), (DWORD)fp);
    fp = GetProcAddress(kernel, "GetVersion");
    ok(fp == (FARPROC)GetVersion, "GetVersion at %lx, imported at %lx\n",
       (DWORD)fp, (DWORD)(FARPROC)GetVersion);
    ok(GetProcAddress(kernel, MAKEINTRESOURCE(3)) == fp, "GetVersion isn't KERNEL.3\n");
    ok(GetProcAddress(kernel, "NoSuchFunction") == NULL,
       "GetProcAddress(NoSuchFunction) %lx\n",
       (DWORD)GetProcAddress(kernel, "NoSuchFunction"));

    fp = GetProcAddress(user, "IsWindow");
    ok(fp != NULL && fp == (FARPROC)IsWindow, "IsWindow at %lx, imported at %lx\n",
       (DWORD)fp, (DWORD)(FARPROC)IsWindow);
    ok(GetProcAddress(user, MAKEINTRESOURCE(47)) == fp, "IsWindow isn't USER.47\n");
    fp = GetProcAddress(user, "GetDesktopWindow");
    ok(fp != NULL, "GetDesktopWindow not found\n");
    ok(GetProcAddress(user, MAKEINTRESOURCE(286)) == fp,
       "GetDesktopWindow isn't USER.286\n");

    fp = GetProcAddress(gdi, "GetStockObject");
    ok(fp != NULL && fp == (FARPROC)GetStockObject,
       "GetStockObject at %lx, imported at %lx\n", (DWORD)fp,
       (DWORD)(FARPROC)GetStockObject);
    ok(GetProcAddress(gdi, MAKEINTRESOURCE(87)) == fp,
       "GetStockObject isn't GDI.87\n");
    fp = GetProcAddress(gdi, "LineTo");
    ok(fp != NULL, "LineTo not found\n");
    ok(GetProcAddress(gdi, MAKEINTRESOURCE(19)) == fp, "LineTo isn't GDI.19\n");

    /* A function found by name can be called. */
    {
        typedef DWORD (FAR PASCAL *GETVERSIONPROC)(void);
        GETVERSIONPROC proc = (GETVERSIONPROC)GetProcAddress(kernel, "GETVERSION");
        ok(proc && proc() == GetVersion(), "GetVersion through GetProcAddress\n");
    }
}

static void test_version(void)
{
    DWORD version = GetVersion();
    DWORD flags = GetWinFlags();

    ok(LOBYTE(LOWORD(version)) == 3, "Windows %d.%d\n", LOBYTE(LOWORD(version)),
       HIBYTE(LOWORD(version)));
    ok(HIBYTE(LOWORD(version)) >= 10, "Windows %d.%d\n",
       LOBYTE(LOWORD(version)), HIBYTE(LOWORD(version)));
    ok(HIBYTE(HIWORD(version)) >= 3, "DOS %d.%d\n", HIBYTE(HIWORD(version)),
       LOBYTE(HIWORD(version)));

    ok(flags & WF_PMODE, "GetWinFlags %lx without WF_PMODE\n", flags);
    ok(!(flags & WF_STANDARD) != !(flags & WF_ENHANCED),
       "GetWinFlags %lx: not one of standard and enhanced mode\n", flags);
    ok(flags & (WF_CPU286 | WF_CPU386 | WF_CPU486),
       "GetWinFlags %lx without a processor\n", flags);
    if (flags & WF_ENHANCED)
        ok(flags & (WF_CPU386 | WF_CPU486), "enhanced mode on %lx\n", flags);
}

static void test_task(void)
{
    HTASK task;
    UINT pdb;
    BYTE FAR *psp;
    WORD psp_dos;

    task = GetCurrentTask();
    ok(task != NULL, "GetCurrentTask failed\n");
    ok(IsTask(task), "IsTask(%x) failed\n", task);
    ok(!IsTask((HTASK)GetModuleHandle("KERNEL")), "KERNEL's module is a task\n");
    ok(GetNumTasks() >= 1, "GetNumTasks %u\n", GetNumTasks());

    pdb = GetCurrentPDB();
    ok(pdb != 0, "GetCurrentPDB failed\n");
    psp = (BYTE FAR *)MAKELP(pdb, 0);
    ok(psp[0] == 0xcd && psp[1] == 0x20, "PSP %x starts with %x %x\n", pdb,
       psp[0], psp[1]);
    /* DOS gives the same PSP. */
    _asm {
        mov ah, 51h
        int 21h
        mov psp_dos, bx
    }
    ok(psp_dos == pdb, "INT 21h 51h gave PSP %x, GetCurrentPDB %x\n", psp_dos, pdb);
    _asm {
        mov ah, 62h
        int 21h
        mov psp_dos, bx
    }
    ok(psp_dos == pdb, "INT 21h 62h gave PSP %x, GetCurrentPDB %x\n", psp_dos, pdb);
}

static void test_strings(void)
{
    static char buf[64];
    static char buf2[64];
    LPSTR p;

    p = lstrcpy(buf, "Hello");
    ok(p == buf, "lstrcpy returned %lx, not %lx\n", (DWORD)p, (DWORD)(LPSTR)buf);
    ok(!_fstrcmp(buf, "Hello"), "lstrcpy gave %s\n", (LPSTR)buf);
    p = lstrcat(buf, ", world");
    ok(p == buf, "lstrcat returned %lx, not %lx\n", (DWORD)p, (DWORD)(LPSTR)buf);
    ok(!_fstrcmp(buf, "Hello, world"), "lstrcat gave %s\n", (LPSTR)buf);
    ok(lstrlen(buf) == 12, "lstrlen %d\n", lstrlen(buf));
    ok(lstrlen("") == 0, "lstrlen of an empty string %d\n", lstrlen(""));

    _fmemset(buf2, 'x', sizeof(buf2));
    p = lstrcpyn(buf2, buf, 6);
    ok(p == buf2, "lstrcpyn returned %lx\n", (DWORD)p);
    ok(!_fstrcmp(buf2, "Hello") && buf2[6] == 'x', "lstrcpyn gave %s\n", (LPSTR)buf2);

    /* hmemcpy within a segment. */
    hmemcpy(buf2, "abcdef", 7);
    ok(!_fstrcmp(buf2, "abcdef"), "hmemcpy gave %s\n", (LPSTR)buf2);

    ok(lstrcmp("abc", "abc") == 0, "lstrcmp(abc, abc) %d\n", lstrcmp("abc", "abc"));
    ok(lstrcmp("abc", "abd") < 0, "lstrcmp(abc, abd) %d\n", lstrcmp("abc", "abd"));
    ok(lstrcmp("abd", "abc") > 0, "lstrcmp(abd, abc) %d\n", lstrcmp("abd", "abc"));
    ok(lstrcmp("ab", "abc") < 0, "lstrcmp(ab, abc) %d\n", lstrcmp("ab", "abc"));
    ok(lstrcmp("", "a") < 0, "lstrcmp(, a) %d\n", lstrcmp("", "a"));
    /* Not by character codes: B comes after a. */
    ok(lstrcmp("a", "B") < 0, "lstrcmp(a, B) %d\n", lstrcmp("a", "B"));
    ok(lstrcmp("A", "a") != 0, "lstrcmp(A, a) %d\n", lstrcmp("A", "a"));
    ok(lstrcmpi("ABC", "abc") == 0, "lstrcmpi(ABC, abc) %d\n", lstrcmpi("ABC", "abc"));
    ok(lstrcmpi("abc", "ABD") < 0, "lstrcmpi(abc, ABD) %d\n", lstrcmpi("abc", "ABD"));
    ok(lstrcmpi("b", "A") > 0, "lstrcmpi(b, A) %d\n", lstrcmpi("b", "A"));

    lstrcpy(buf, "MiXeD 123 \xe9\xe0");
    p = AnsiUpper(buf);
    ok(p == buf, "AnsiUpper returned %lx\n", (DWORD)p);
    ok(!_fstrcmp(buf, "MIXED 123 \xc9\xc0"), "AnsiUpper gave %s\n", (LPSTR)buf);
    p = AnsiLower(buf);
    ok(p == buf, "AnsiLower returned %lx\n", (DWORD)p);
    ok(!_fstrcmp(buf, "mixed 123 \xe9\xe0"), "AnsiLower gave %s\n", (LPSTR)buf);
    /* A character in the low word of the pointer. */
    p = AnsiUpper((LPSTR)MAKELP(0, 'q'));
    ok(SELECTOROF(p) == 0 && OFFSETOF(p) == 'Q', "AnsiUpper(q) gave %lx\n", (DWORD)p);
    p = AnsiLower((LPSTR)MAKELP(0, 'Q'));
    ok(SELECTOROF(p) == 0 && OFFSETOF(p) == 'q', "AnsiLower(Q) gave %lx\n", (DWORD)p);
    p = AnsiUpper((LPSTR)MAKELP(0, 0xe7));
    ok(SELECTOROF(p) == 0 && OFFSETOF(p) == 0xc7, "AnsiUpper(e7) gave %lx\n", (DWORD)p);

    lstrcpy(buf, "abc");
    buf[4] = 'z';
    ok(AnsiUpperBuff(buf, 5) == 5, "AnsiUpperBuff didn't return 5\n");
    ok(!_fstrcmp(buf, "ABC") && buf[4] == 'Z', "AnsiUpperBuff gave %s %c\n",
       (LPSTR)buf, buf[4]);
    ok(AnsiLowerBuff(buf, 2) == 2, "AnsiLowerBuff didn't return 2\n");
    ok(!_fstrcmp(buf, "abC"), "AnsiLowerBuff gave %s\n", (LPSTR)buf);

    lstrcpy(buf, "xyz");
    ok(AnsiNext(buf) == buf + 1, "AnsiNext moved %d\n", (int)(AnsiNext(buf) - buf));
    ok(AnsiNext(buf + 2) == buf + 3, "AnsiNext of the last character\n");
    ok(AnsiNext(buf + 3) == buf + 3, "AnsiNext past the end moved\n");
    ok(AnsiPrev(buf, buf + 3) == buf + 2, "AnsiPrev moved %d\n",
       (int)(AnsiPrev(buf, buf + 3) - (buf + 3)));
    ok(AnsiPrev(buf, buf) == buf, "AnsiPrev before the start moved\n");
}

static CATCHBUF catch_buf;
static int throw_depth;

static void thrower(int n)
{
    throw_depth++;
    if (n) thrower(n - 1);
    else Throw(catch_buf, 42);
}

static void test_catch_throw(void)
{
    int ret;
    static int passes;
    volatile int local = 1234;

    passes = 0;
    throw_depth = 0;
    ret = Catch(catch_buf);
    passes++;
    if (ret == 0)
    {
        ok(passes == 1, "Catch returned 0 on pass %d\n", passes);
        if (passes == 1) thrower(5);
        ok(0, "Throw returned\n");
    }
    else
    {
        ok(ret == 42, "Catch returned %d after Throw\n", ret);
        ok(passes == 2, "Catch returned on pass %d\n", passes);
        ok(throw_depth == 6, "thrown from depth %d\n", throw_depth);
        ok(local == 1234, "local %d after Throw\n", local);
    }

    /* Catch again, after the first is done with. */
    passes = 0;
    ret = Catch(catch_buf);
    passes++;
    if (ret == 0 && passes == 1)
        Throw(catch_buf, -1);
    ok(ret == -1 && passes == 2, "Catch returned %d on pass %d\n", ret, passes);
}

/* The command line of a program started without one is an empty
 * string, without the carriage return that ends the PSP's command tail
 * (added for Win16). */
static void test_command_line(void)
{
    ok(lstrlen(winetest_cmdline) == 0, "command line of %d characters\n",
       lstrlen(winetest_cmdline));
}

START_TEST(module)
{
    this_instance = get_instance();
    test_module_handles();
    test_GetProcAddress();
    testLoadLibrary();
    testLoadLibrary_Wrong();
    test_version();
    test_task();
    test_strings();
    test_catch_throw();
    test_command_line();
}
