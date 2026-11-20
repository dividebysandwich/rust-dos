/*
 * RegisterDragDrop and RevokeDragDrop as Windows 98 SE's OLE32 returned
 * them, beyond what Wine's dragdrop.c checks: the order of the checks,
 * a process that never called OleInitialize, threads that initialized
 * COM, OLE or nothing, the property set by hand, and windows of another
 * thread and process. rust-dos's own test, written with Wine's test
 * framework. It runs in a process of its own, as what it checks first
 * is a process in which OleInitialize was never called.
 *
 * Copyright the rust-dos authors
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 */

#define COBJMACROS
#define CONST_VTABLE

#include <stdarg.h>
#include <stdio.h>

#include "windef.h"
#include "winbase.h"
#include "objbase.h"
#include "ole2.h"

#include "wine/test.h"

#define PROP "OleDropTargetInterface"

static int addrefs, releases;

static HRESULT WINAPI dt_QueryInterface(IDropTarget *iface, REFIID riid, void **out)
{
    *out = NULL;
    return E_NOINTERFACE;
}

static ULONG WINAPI dt_AddRef(IDropTarget *iface)
{
    addrefs++;
    return 2;
}

static ULONG WINAPI dt_Release(IDropTarget *iface)
{
    releases++;
    return 1;
}

static HRESULT WINAPI dt_DragEnter(IDropTarget *iface, IDataObject *data, DWORD keys,
                                   POINTL pt, DWORD *effect)
{
    return E_NOTIMPL;
}

static HRESULT WINAPI dt_DragOver(IDropTarget *iface, DWORD keys, POINTL pt, DWORD *effect)
{
    return E_NOTIMPL;
}

static HRESULT WINAPI dt_DragLeave(IDropTarget *iface)
{
    return E_NOTIMPL;
}

static HRESULT WINAPI dt_Drop(IDropTarget *iface, IDataObject *data, DWORD keys,
                              POINTL pt, DWORD *effect)
{
    return E_NOTIMPL;
}

static const IDropTargetVtbl dt_vtbl =
{
    dt_QueryInterface,
    dt_AddRef,
    dt_Release,
    dt_DragEnter,
    dt_DragOver,
    dt_DragLeave,
    dt_Drop
};

static IDropTarget target = { &dt_vtbl };
static IDropTarget other = { &dt_vtbl };

static LRESULT CALLBACK wndproc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
    if (msg == WM_DESTROY)
    {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcA(hwnd, msg, wp, lp);
}

static void register_class(void)
{
    WNDCLASSA wc;

    memset(&wc, 0, sizeof(wc));
    wc.lpfnWndProc = wndproc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "DropRegClass";
    RegisterClassA(&wc);
}

static HWND make_window(const char *title)
{
    return CreateWindowA("DropRegClass", title, 0, 0, 0, 100, 100, NULL, NULL, NULL, NULL);
}

static void run_thread(LPTHREAD_START_ROUTINE func, void *arg)
{
    DWORD tid;
    HANDLE thread = CreateThread(NULL, 0, func, arg, 0, &tid);

    WaitForSingleObject(thread, INFINITE);
    CloseHandle(thread);
}

/* What a thread got: the initialization's result, then Register's and
   Revoke's on a window of its own. */
static HRESULT got[3];

enum init { INIT_NONE, INIT_COM };

static DWORD WINAPI register_own(void *arg)
{
    enum init init = (enum init)(INT_PTR)arg;
    HWND hwnd = make_window("own");

    got[0] = S_OK;
    if (init == INIT_COM)
        got[0] = CoInitialize(NULL);
    got[1] = RegisterDragDrop(hwnd, &target);
    got[2] = RevokeDragDrop(hwnd);
    if (init != INIT_NONE)
        CoUninitialize();
    DestroyWindow(hwnd);
    return 0;
}

static HWND shared;

/* Registers `shared`, a window of the main thread, then ends. */
static DWORD WINAPI register_shared(void *arg)
{
    OleInitialize(NULL);
    got[1] = RegisterDragDrop(shared, &target);
    OleUninitialize();
    return 0;
}

/* Revokes `shared` with COM not initialized. */
static DWORD WINAPI revoke_shared(void *arg)
{
    got[1] = RegisterDragDrop(shared, &other);
    got[2] = RevokeDragDrop(shared);
    return 0;
}

/* Before any OleInitialize in the process: E_OUTOFMEMORY once the
   target and window pass, CoInitialize or not, and before the check
   for a window registered already. */
static void test_no_ole(void)
{
    HWND hwnd = make_window("no ole");
    HRESULT hr;

    hr = RegisterDragDrop(hwnd, NULL);
    ok(hr == E_INVALIDARG, "got 0x%08x\n", hr);
    hr = RegisterDragDrop((HWND)0xdeadbeef, NULL);
    ok(hr == E_INVALIDARG, "got 0x%08x\n", hr);
    hr = RegisterDragDrop(NULL, &target);
    ok(hr == DRAGDROP_E_INVALIDHWND, "got 0x%08x\n", hr);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == E_OUTOFMEMORY, "got 0x%08x\n", hr);

    SetPropA(hwnd, PROP, &other);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == E_OUTOFMEMORY, "got 0x%08x\n", hr);
    RemovePropA(hwnd, PROP);

    hr = RevokeDragDrop(hwnd);
    ok(hr == DRAGDROP_E_NOTREGISTERED, "got 0x%08x\n", hr);
    hr = RevokeDragDrop((HWND)0xdeadbeef);
    ok(hr == DRAGDROP_E_INVALIDHWND, "got 0x%08x\n", hr);

    hr = CoInitialize(NULL);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == E_OUTOFMEMORY, "got 0x%08x\n", hr);
    CoUninitialize();

    run_thread(register_own, (void *)INIT_COM);
    ok(got[1] == E_OUTOFMEMORY, "got 0x%08x\n", got[1]);
    ok(addrefs == 0 && releases == 0, "AddRef %d, Release %d\n", addrefs, releases);
    DestroyWindow(hwnd);
}

/* Threads of a process whose main thread called OleInitialize. */
static void test_threads(void)
{
    HWND hwnd = make_window("threads");
    HRESULT hr;

    hr = OleInitialize(NULL);
    ok(hr == S_OK, "got 0x%08x\n", hr);

    /* COM is enough for another thread; nothing is not. */
    run_thread(register_own, (void *)INIT_NONE);
    ok(got[1] == CO_E_NOTINITIALIZED, "got 0x%08x\n", got[1]);
    ok(got[2] == DRAGDROP_E_NOTREGISTERED, "got 0x%08x\n", got[2]);
    run_thread(register_own, (void *)INIT_COM);
    ok(got[1] == S_OK, "got 0x%08x\n", got[1]);
    ok(got[2] == S_OK, "got 0x%08x\n", got[2]);
    ok(addrefs == 1 && releases == 1, "AddRef %d, Release %d\n", addrefs, releases);

    addrefs = releases = 0;

    /* A thread without COM finds the window registered, and revokes
       the main thread's target. */
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    shared = hwnd;
    run_thread(revoke_shared, NULL);
    ok(got[1] == DRAGDROP_E_ALREADYREGISTERED, "got 0x%08x\n", got[1]);
    ok(got[2] == S_OK, "got 0x%08x\n", got[2]);
    ok(addrefs == 1 && releases == 1, "AddRef %d, Release %d\n", addrefs, releases);
    ok(GetPropA(hwnd, PROP) == NULL, "property %p\n", GetPropA(hwnd, PROP));

    /* Another thread registers this thread's window and ends: its
       target can't be revoked, but its property goes. */
    run_thread(register_shared, NULL);
    ok(got[1] == S_OK, "got 0x%08x\n", got[1]);
    ok(GetPropA(hwnd, PROP) == &target, "property %p\n", GetPropA(hwnd, PROP));
    hr = RevokeDragDrop(hwnd);
    ok(hr == RPC_E_WRONG_THREAD, "got 0x%08x\n", hr);
    ok(GetPropA(hwnd, PROP) == NULL, "property %p\n", GetPropA(hwnd, PROP));
    ok(addrefs == 2 && releases == 1, "AddRef %d, Release %d\n", addrefs, releases);
    addrefs = releases = 0;

    DestroyWindow(hwnd);
}

/* The property is where RegisterDragDrop looks for a target, but one
   set by hand can't be revoked. */
static void test_property(void)
{
    HWND hwnd = make_window("property");
    HRESULT hr;

    SetPropA(hwnd, PROP, &other);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == DRAGDROP_E_ALREADYREGISTERED, "got 0x%08x\n", hr);
    hr = RevokeDragDrop(hwnd);
    ok(hr == RPC_E_WRONG_THREAD, "got 0x%08x\n", hr);
    ok(GetPropA(hwnd, PROP) == NULL, "property %p\n", GetPropA(hwnd, PROP));

    /* A property of 0 is no target. */
    SetPropA(hwnd, PROP, NULL);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    ok(GetPropA(hwnd, PROP) == &target, "property %p\n", GetPropA(hwnd, PROP));
    hr = RevokeDragDrop(hwnd);
    ok(hr == S_OK, "got 0x%08x\n", hr);

    /* Nor is a property taken away. */
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    RemovePropA(hwnd, PROP);
    hr = RevokeDragDrop(hwnd);
    ok(hr == DRAGDROP_E_NOTREGISTERED, "got 0x%08x\n", hr);
    ok(addrefs == 2 && releases == 1, "AddRef %d, Release %d\n", addrefs, releases);
    addrefs = releases = 0;

    DestroyWindow(hwnd);
}

/* OleInitialize's count, and the thread after its last
   OleUninitialize: CO_E_NOTINITIALIZED, as the process has had OLE,
   and CoInitialize is enough again. */
static void test_uninitialize(void)
{
    HWND hwnd = make_window("uninitialize"), second = make_window("second");
    HRESULT hr;

    hr = OleInitialize(NULL);
    ok(hr == S_FALSE, "got 0x%08x\n", hr);
    OleUninitialize();
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);

    OleUninitialize();
    ok(releases == 0, "Release %d\n", releases);
    ok(GetPropA(hwnd, PROP) == &target, "property %p\n", GetPropA(hwnd, PROP));
    hr = RegisterDragDrop(second, &target);
    ok(hr == CO_E_NOTINITIALIZED, "got 0x%08x\n", hr);
    hr = CoInitialize(NULL);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    hr = RegisterDragDrop(second, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    hr = RevokeDragDrop(second);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    CoUninitialize();
    hr = RevokeDragDrop(hwnd);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    ok(addrefs == 2 && releases == 2, "AddRef %d, Release %d\n", addrefs, releases);
    addrefs = releases = 0;

    /* A window destroyed while registered keeps the reference. */
    hr = OleInitialize(NULL);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    hr = RegisterDragDrop(hwnd, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    DestroyWindow(hwnd);
    ok(releases == 0, "Release %d\n", releases);
    hr = RevokeDragDrop(hwnd);
    ok(hr == DRAGDROP_E_INVALIDHWND, "got 0x%08x\n", hr);
    addrefs = releases = 0;

    DestroyWindow(second);
}

/* The child: a window it registered and one it didn't, until the
   parent closes them. */
static void child(void)
{
    HWND own, plain;
    MSG msg;

    OleInitialize(NULL);
    own = make_window("dropreg own");
    plain = make_window("dropreg plain");
    RegisterDragDrop(own, &target);
    while (GetMessageA(&msg, NULL, 0, 0) > 0)
        DispatchMessageA(&msg);
    ExitProcess(0);
}

/* A window of another process can be registered, and its own target
   can't be revoked from here, though its property goes. */
static void test_other_process(void)
{
    char cmdline[MAX_PATH + 32], **argv;
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    HWND own = NULL, plain = NULL;
    HRESULT hr;
    int i;

    winetest_get_mainargs(&argv);
    sprintf(cmdline, "%s %s child", argv[0], argv[1]);
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    ok(CreateProcessA(NULL, cmdline, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi),
       "CreateProcess failed: %u\n", GetLastError());
    for (i = 0; i < 100 && !(own && plain); i++)
    {
        Sleep(50);
        own = FindWindowA("DropRegClass", "dropreg own");
        plain = FindWindowA("DropRegClass", "dropreg plain");
    }
    ok(own && plain, "the child's windows aren't there\n");
    Sleep(100);

    hr = RegisterDragDrop(plain, &target);
    ok(hr == S_OK, "got 0x%08x\n", hr);
    ok(GetPropA(plain, PROP) == &target, "property %p\n", GetPropA(plain, PROP));
    hr = RevokeDragDrop(plain);
    ok(hr == S_OK, "got 0x%08x\n", hr);

    hr = RegisterDragDrop(own, &target);
    ok(hr == DRAGDROP_E_ALREADYREGISTERED, "got 0x%08x\n", hr);
    hr = RevokeDragDrop(own);
    ok(hr == RPC_E_WRONG_THREAD, "got 0x%08x\n", hr);
    ok(GetPropA(own, PROP) == NULL, "property %p\n", GetPropA(own, PROP));
    ok(addrefs == 1 && releases == 1, "AddRef %d, Release %d\n", addrefs, releases);

    PostMessageA(own, WM_CLOSE, 0, 0);
    ok(WaitForSingleObject(pi.hProcess, 5000) == WAIT_OBJECT_0, "the child didn't end\n");
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    OleUninitialize();
}

START_TEST(dropreg)
{
    char **argv;
    int argc = winetest_get_mainargs(&argv);

    register_class();
    if (argc >= 3)
    {
        child();
        return;
    }

    test_no_ole();
    test_threads();
    test_property();
    test_uninitialize();
    test_other_process();
}
