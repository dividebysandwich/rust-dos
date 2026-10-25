/*
 * Unit tests for window handling
 *
 * Copyright 2002 Bill Medland
 * Copyright 2002 Alexandre Julliard
 * Copyright 2003 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/user32/tests/win.c, in part: the
 * parents and owners of windows (GetParent, GetWindow(GW_OWNER) and
 * SetParent, IsChild), EnumChildWindows, the Z order of child windows,
 * SetWindowPos with WM_NCCALCSIZE and HWND_TOPMOST, the styles
 * CreateWindow and CreateWindowEx add, a window's rectangles against
 * AdjustWindowRectEx, ShowWindow and WM_SYSCOMMAND, and WM_GETMINMAXINFO
 * before WM_NCCREATE. Windows 3.1 has no GetAncestor, no threads (the
 * EnumThreadWindows test is EnumTaskWindows here) and no
 * SetLastError, and its windows have none of Windows 95's extended
 * styles, such as WS_EX_WINDOWEDGE, so those are left out, as are the
 * GWL_HWNDPARENT checks (Windows 3.1's GWW_HWNDPARENT is a plain field)
 * and the Win32 tests of message-only, layered and full-screen
 * windows, capture, focus, scrolling, painting, MDI and coordinates
 * past 16 bits. Where Windows 9x and NT differ, Wine's Windows 9x
 * branch is the one kept. Added for Win16 are checks of subclassing
 * through GWL_WNDPROC, Get/SetWindowWord, MoveWindow, window text,
 * FindWindow and EnumWindows, and an enumeration procedure that loads
 * its own data segment (__loadds) in place of MakeProcInstance.
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

#define COUNTOF(arr) (sizeof(arr)/sizeof(arr[0]))

static HWND hwndMain, hwndMain2;
static HINSTANCE main_instance;
static HMENU hmenu;
static const char* szAWRClass = "Winsize";

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

/* What WM_GETMINMAXINFO leaves in a window's extra bytes, as Wine's
 * test leaves it in GWLP_USERDATA. */
#define MINMAX_MAGIC 0x20031021L

/* check the values returned by the various parent/owner functions on a given window */
static void check_parents_( int line, HWND hwnd, HWND get_parent, HWND gw_owner )
{
    HWND res;

    res = GetParent( hwnd );
    ok_(__FILE__,line)( res == get_parent, "Wrong result for GetParent %04x expected %04x\n", res, get_parent );
    res = GetWindow( hwnd, GW_OWNER );
    ok_(__FILE__,line)( res == gw_owner, "Wrong result for GW_OWNER %04x expected %04x\n", res, gw_owner );
}
/* Wine's arguments, of which Windows 3.1 has GetParent's and GW_OWNER's */
#define check_parents(hwnd, ga_parent, gwl_parent, get_parent, gw_owner, ga_root, ga_root_owner) \
    check_parents_(__LINE__, hwnd, get_parent, gw_owner)

/* Wine's two procedures trace the windows they get, which here would
 * move the place the ok() around their EnumChildWindows reports, so
 * they don't. */
BOOL CALLBACK __export EnumChildProc( HWND hwndChild, LPARAM lParam)
{
    (*(LPINT)lParam)++;
    if (*(LPINT)lParam > 1) return FALSE;
    return TRUE;
}

/* will search for the given window */
BOOL CALLBACK __export EnumChildProc1( HWND hwndChild, LPARAM lParam)
{
    if ((HWND)lParam == hwndChild) return FALSE;
    return TRUE;
}

static FARPROC enum_child_proc, enum_child_proc1;

static HWND create_tool_window( LONG style, HWND parent )
{
    HWND ret = CreateWindow("ToolWindowClass", "Tool window 1", style,
                            0, 0, 100, 100, parent, 0, main_instance, NULL );
    ok( ret != 0, "Creation failed\n" );
    return ret;
}

/* test parent and owner values for various combinations */
static void test_parent_owner(void)
{
    HWND test, owner, ret;
    HWND desktop = GetDesktopWindow();
    HWND child = create_tool_window( WS_CHILD, hwndMain );
    int  numChildren;

    trace( "main window %04x main2 %04x desktop %04x child %04x\n", hwndMain, hwndMain2, desktop, child );

    /* child without parent, should fail */
    test = CreateWindow("ToolWindowClass", "Tool window 1",
                        WS_CHILD, 0, 0, 100, 100, 0, 0, main_instance, NULL );
    ok( !test, "WS_CHILD without parent created\n" );

    /* desktop window */
    check_parents( desktop, 0, 0, 0, 0, 0, 0 );

    /* normal child window */
    test = create_tool_window( WS_CHILD, hwndMain );
    trace( "created child %04x\n", test );
    check_parents( test, hwndMain, hwndMain, hwndMain, 0, hwndMain, hwndMain );
    SetWindowLong( test, GWL_STYLE, 0 );
    check_parents( test, hwndMain, hwndMain, 0, 0, hwndMain, test );
    SetWindowLong( test, GWL_STYLE, WS_POPUP );
    check_parents( test, hwndMain, hwndMain, 0, 0, hwndMain, test );
    SetWindowLong( test, GWL_STYLE, WS_POPUP|WS_CHILD );
    check_parents( test, hwndMain, hwndMain, 0, 0, hwndMain, test );
    SetWindowLong( test, GWL_STYLE, WS_CHILD );
    DestroyWindow( test );

    /* normal child window with WS_MAXIMIZE */
    test = create_tool_window( WS_CHILD | WS_MAXIMIZE, hwndMain );
    DestroyWindow( test );

    /* normal child window with WS_THICKFRAME */
    test = create_tool_window( WS_CHILD | WS_THICKFRAME, hwndMain );
    DestroyWindow( test );

    /* popup window with WS_THICKFRAME */
    test = create_tool_window( WS_POPUP | WS_THICKFRAME, hwndMain );
    DestroyWindow( test );

    /* child of desktop */
    test = create_tool_window( WS_CHILD, desktop );
    trace( "created child of desktop %04x\n", test );
    check_parents( test, desktop, 0, desktop, 0, test, desktop );
    SetWindowLong( test, GWL_STYLE, WS_POPUP );
    check_parents( test, desktop, 0, 0, 0, test, test );
    SetWindowLong( test, GWL_STYLE, 0 );
    check_parents( test, desktop, 0, 0, 0, test, test );
    DestroyWindow( test );

    /* child of desktop with WS_MAXIMIZE */
    test = create_tool_window( WS_CHILD | WS_MAXIMIZE, desktop );
    DestroyWindow( test );

    /* child of desktop with WS_MINIMIZE */
    test = create_tool_window( WS_CHILD | WS_MINIMIZE, desktop );
    DestroyWindow( test );

    /* child of child */
    test = create_tool_window( WS_CHILD, child );
    trace( "created child of child %04x\n", test );
    check_parents( test, child, child, child, 0, hwndMain, hwndMain );
    SetWindowLong( test, GWL_STYLE, 0 );
    check_parents( test, child, child, 0, 0, hwndMain, test );
    SetWindowLong( test, GWL_STYLE, WS_POPUP );
    check_parents( test, child, child, 0, 0, hwndMain, test );
    DestroyWindow( test );

    /* child of child with WS_MAXIMIZE */
    test = create_tool_window( WS_CHILD | WS_MAXIMIZE, child );
    DestroyWindow( test );

    /* child of child with WS_MINIMIZE */
    test = create_tool_window( WS_CHILD | WS_MINIMIZE, child );
    DestroyWindow( test );

    /* not owned top-level window */
    test = create_tool_window( 0, 0 );
    trace( "created top-level %04x\n", test );
    check_parents( test, desktop, 0, 0, 0, test, test );
    SetWindowLong( test, GWL_STYLE, WS_POPUP );
    check_parents( test, desktop, 0, 0, 0, test, test );
    SetWindowLong( test, GWL_STYLE, WS_CHILD );
    check_parents( test, desktop, 0, desktop, 0, test, desktop );
    DestroyWindow( test );

    /* not owned top-level window with WS_MAXIMIZE */
    test = create_tool_window( WS_MAXIMIZE, 0 );
    DestroyWindow( test );

    /* owned top-level window */
    test = create_tool_window( 0, hwndMain );
    trace( "created owned top-level %04x\n", test );
    check_parents( test, desktop, hwndMain, 0, hwndMain, test, test );
    SetWindowLong( test, GWL_STYLE, WS_POPUP );
    check_parents( test, desktop, hwndMain, hwndMain, hwndMain, test, hwndMain );
    SetWindowLong( test, GWL_STYLE, WS_CHILD );
    check_parents( test, desktop, hwndMain, desktop, hwndMain, test, desktop );
    DestroyWindow( test );

    /* owned top-level window with WS_MAXIMIZE */
    test = create_tool_window( WS_MAXIMIZE, hwndMain );
    DestroyWindow( test );

    /* not owned popup */
    test = create_tool_window( WS_POPUP, 0 );
    trace( "created popup %04x\n", test );
    check_parents( test, desktop, 0, 0, 0, test, test );
    SetWindowLong( test, GWL_STYLE, WS_CHILD );
    check_parents( test, desktop, 0, desktop, 0, test, desktop );
    SetWindowLong( test, GWL_STYLE, 0 );
    check_parents( test, desktop, 0, 0, 0, test, test );
    DestroyWindow( test );

    /* not owned popup with WS_MAXIMIZE */
    test = create_tool_window( WS_POPUP | WS_MAXIMIZE, 0 );
    DestroyWindow( test );

    /* owned popup */
    test = create_tool_window( WS_POPUP, hwndMain );
    trace( "created owned popup %04x\n", test );
    check_parents( test, desktop, hwndMain, hwndMain, hwndMain, test, hwndMain );
    SetWindowLong( test, GWL_STYLE, WS_CHILD );
    check_parents( test, desktop, hwndMain, desktop, hwndMain, test, desktop );
    SetWindowLong( test, GWL_STYLE, 0 );
    check_parents( test, desktop, hwndMain, 0, hwndMain, test, test );
    DestroyWindow( test );

    /* owned popup with WS_MAXIMIZE */
    test = create_tool_window( WS_POPUP | WS_MAXIMIZE, hwndMain );
    DestroyWindow( test );

    /* top-level window owned by child (same as owned by top-level) */
    test = create_tool_window( 0, child );
    trace( "created top-level owned by child %04x\n", test );
    check_parents( test, desktop, hwndMain, 0, hwndMain, test, test );
    DestroyWindow( test );

    /* top-level window owned by child (same as owned by top-level) with WS_MAXIMIZE */
    test = create_tool_window( WS_MAXIMIZE, child );
    DestroyWindow( test );

    /* popup owned by desktop (same as not owned) */
    test = create_tool_window( WS_POPUP, desktop );
    trace( "created popup owned by desktop %04x\n", test );
    check_parents( test, desktop, 0, 0, 0, test, test );
    DestroyWindow( test );

    /* popup owned by desktop (same as not owned) with WS_MAXIMIZE */
    test = create_tool_window( WS_POPUP | WS_MAXIMIZE, desktop );
    DestroyWindow( test );

    /* popup owned by child (same as owned by top-level) */
    test = create_tool_window( WS_POPUP, child );
    trace( "created popup owned by child %04x\n", test );
    check_parents( test, desktop, hwndMain, hwndMain, hwndMain, test, hwndMain );
    DestroyWindow( test );

    /* popup owned by child (same as owned by top-level) with WS_MAXIMIZE */
    test = create_tool_window( WS_POPUP | WS_MAXIMIZE, child );
    DestroyWindow( test );

    /* not owned popup with WS_CHILD (same as WS_POPUP only) */
    test = create_tool_window( WS_POPUP | WS_CHILD, 0 );
    trace( "created WS_CHILD popup %04x\n", test );
    check_parents( test, desktop, 0, 0, 0, test, test );
    DestroyWindow( test );

    /* not owned popup with WS_CHILD | WS_MAXIMIZE (same as WS_POPUP only) */
    test = create_tool_window( WS_POPUP | WS_CHILD | WS_MAXIMIZE, 0 );
    DestroyWindow( test );

    /* owned popup with WS_CHILD (same as WS_POPUP only) */
    test = create_tool_window( WS_POPUP | WS_CHILD, hwndMain );
    trace( "created owned WS_CHILD popup %04x\n", test );
    check_parents( test, desktop, hwndMain, hwndMain, hwndMain, test, hwndMain );
    DestroyWindow( test );

    /* owned popup with WS_CHILD (same as WS_POPUP only) with WS_MAXIMIZE */
    test = create_tool_window( WS_POPUP | WS_CHILD | WS_MAXIMIZE, hwndMain );
    DestroyWindow( test );

    /******************** parent changes *************************/
    trace( "testing parent changes\n" );

    /* desktop window */
    check_parents( desktop, 0, 0, 0, 0, 0, 0 );

    /* normal child window */
    test = create_tool_window( WS_CHILD, hwndMain );
    trace( "created child %04x\n", test );

    ret = SetParent( test, desktop );
    ok( ret == hwndMain, "SetParent return value %04x expected %04x\n", ret, hwndMain );
    check_parents( test, desktop, 0, desktop, 0, test, desktop );

    ret = SetParent( test, child );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );
    check_parents( test, child, child, child, 0, hwndMain, hwndMain );

    ret = SetParent( test, hwndMain2 );
    ok( ret == child, "SetParent return value %04x expected %04x\n", ret, child );
    check_parents( test, hwndMain2, hwndMain2, hwndMain2, 0, hwndMain2, hwndMain2 );
    DestroyWindow( test );

    /* not owned top-level window */
    test = create_tool_window( 0, 0 );
    trace( "created top-level %04x\n", test );

    ret = SetParent( test, child );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );
    check_parents( test, child, child, 0, 0, hwndMain, test );
    DestroyWindow( test );

    /* owned popup */
    test = create_tool_window( WS_POPUP, hwndMain2 );
    trace( "created owned popup %04x\n", test );

    ret = SetParent( test, child );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );
    check_parents( test, child, child, hwndMain2, hwndMain2, hwndMain, hwndMain2 );
    DestroyWindow( test );

    /**************** test owner destruction *******************/

    /* owned child popup */
    owner = create_tool_window( 0, 0 );
    test = create_tool_window( WS_POPUP, owner );
    trace( "created owner %04x and popup %04x\n", owner, test );
    ret = SetParent( test, child );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );
    check_parents( test, child, child, owner, owner, hwndMain, owner );
    /* window is now child of 'child' but owned by 'owner' */
    DestroyWindow( owner );
    ok( IsWindow(test), "Window %04x destroyed by owner destruction\n", test );
    ok( !IsWindow(owner), "Owner %04x not destroyed\n", owner );
    DestroyWindow(test);

    /* owned top-level popup */
    owner = create_tool_window( 0, 0 );
    test = create_tool_window( WS_POPUP, owner );
    trace( "created owner %04x and popup %04x\n", owner, test );
    check_parents( test, desktop, owner, owner, owner, test, owner );
    DestroyWindow( owner );
    ok( !IsWindow(test), "Window %04x not destroyed by owner destruction\n", test );

    /* final cleanup */
    DestroyWindow(child);


    owner = create_tool_window( WS_OVERLAPPED, 0 );
    test = create_tool_window( WS_POPUP, desktop );

    ok( !GetWindow( test, GW_OWNER ), "Wrong owner window\n" );
    numChildren = 0;
    ok( !EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned FALSE\n" );
    ok( numChildren == 0, "numChildren should be 0 got %d\n", numChildren );

    SetWindowLong( test, GWL_STYLE, (GetWindowLong( test, GWL_STYLE ) & ~WS_POPUP) | WS_CHILD );
    ret = SetParent( test, owner );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );

    numChildren = 0;
    ok( EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned TRUE\n" );
    ok( numChildren == 1, "numChildren should be 1 got %d\n", numChildren );

    child = create_tool_window( WS_CHILD, owner );
    numChildren = 0;
    ok( !EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned FALSE\n" );
    ok( numChildren == 2, "numChildren should be 2 got %d\n", numChildren );
    DestroyWindow( child );

    child = create_tool_window( WS_VISIBLE | WS_OVERLAPPEDWINDOW, owner );
    ok( GetWindow( child, GW_OWNER ) == owner, "Wrong owner window\n" );
    numChildren = 0;
    ok( EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned TRUE\n" );
    ok( numChildren == 1, "numChildren should be 1 got %d\n", numChildren );

    ret = SetParent( child, owner );
    ok( GetWindow( child, GW_OWNER ) == owner, "Wrong owner window\n" );
    ok( ret == desktop, "SetParent return value %04x expected %04x\n", ret, desktop );
    numChildren = 0;
    ok( !EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned FALSE\n" );
    ok( numChildren == 2, "numChildren should be 2 got %d\n", numChildren );

    ret = SetParent( child, NULL );
    ok( GetWindow( child, GW_OWNER ) == owner, "Wrong owner window\n" );
    ok( ret == owner, "SetParent return value %04x expected %04x\n", ret, owner );
    numChildren = 0;
    ok( EnumChildWindows( owner, enum_child_proc, (LPARAM)(LPINT)&numChildren ),
        "EnumChildWindows should have returned TRUE\n" );
    ok( numChildren == 1, "numChildren should be 1 got %d\n", numChildren );

    /* even GW_OWNER == owner it's still a desktop's child */
    ok( !EnumChildWindows( desktop, enum_child_proc1, (LPARAM)child ),
        "EnumChildWindows should have found %04x and returned FALSE\n", child );

    DestroyWindow( child );
    child = create_tool_window( WS_VISIBLE | WS_OVERLAPPEDWINDOW, NULL );

    ok( !EnumChildWindows( desktop, enum_child_proc1, (LPARAM)child ),
        "EnumChildWindows should have found %04x and returned FALSE\n", child );

    DestroyWindow( child );
    DestroyWindow( test );
    DestroyWindow( owner );
}

/* Enumeration procedures that load their data segment themselves, so
 * they are passed without MakeProcInstance. Not exported, so that the
 * prologue keeps the caller's DS until then, and without the stack check,
 * which would read that DS. */
#pragma off (check_stack)
BOOL CALLBACK __loadds enum_proc( HWND hwnd, LPARAM lParam)
{
    (*(LPINT)lParam)++;
    if (*(LPINT)lParam > 2) return FALSE;
    return TRUE;
}

static int task_windows;

BOOL CALLBACK __loadds count_proc( HWND hwnd, LPARAM lParam)
{
    if (lParam != 0x12345678L) return FALSE;
    task_windows++;
    return TRUE;
}
#pragma on (check_stack)

/* EnumThreadWindows's test, with the task's windows: the two main
 * windows are there already. */
static void test_enum_task_windows(void)
{
    int count, before;
    HWND hwnd[2];
    BOOL ret;

    task_windows = 0;
    ret = EnumTaskWindows( GetCurrentTask(), (WNDENUMPROC)count_proc, 0x12345678L );
    ok( ret, "EnumTaskWindows should have returned TRUE\n" );
    ok( task_windows >= 2, "%d windows, expected at least the 2 main ones\n", task_windows );
    before = task_windows;

    hwnd[0] = CreateWindow("ToolWindowClass", "Tool window 1", WS_POPUP,
                           0, 0, 100, 100, 0, 0, main_instance, NULL );
    task_windows = 0;
    ret = EnumTaskWindows( GetCurrentTask(), (WNDENUMPROC)count_proc, 0x12345678L );
    ok( ret, "EnumTaskWindows should have returned TRUE\n" );
    ok( task_windows == before + 1, "count should be %d got %d\n", before + 1, task_windows );

    hwnd[1] = CreateWindow("ToolWindowClass", "Tool window 3", WS_POPUP,
                           0, 0, 100, 100, 0, 0, main_instance, NULL );
    count = 0;
    ret = EnumTaskWindows( GetCurrentTask(), (WNDENUMPROC)enum_proc, (LPARAM)(LPINT)&count );
    ok( !ret, "EnumTaskWindows should have returned FALSE\n" );
    ok( count == 3, "count should be 3 got %d\n", count );

    /* a child window isn't a task window */
    task_windows = 0;
    CreateWindow("ToolWindowClass", "Tool child", WS_CHILD,
                 0, 0, 10, 10, hwnd[0], 0, main_instance, NULL );
    ret = EnumTaskWindows( GetCurrentTask(), (WNDENUMPROC)count_proc, 0x12345678L );
    ok( task_windows == before + 2, "count should be %d got %d\n", before + 2, task_windows );

    /* EnumWindows finds them among the others */
    ok( !EnumWindows( enum_child_proc1, (LPARAM)hwnd[1] ),
        "EnumWindows should have found %04x and returned FALSE\n", hwnd[1] );

    DestroyWindow(hwnd[1]);
    DestroyWindow(hwnd[0]);
}

LRESULT CALLBACK __export main_window_procA(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    switch (msg)
    {
	case WM_GETMINMAXINFO:
	{
	    SetWindowLong(hwnd, 0, MINMAX_MAGIC);
	    break;
	}
	case WM_NCCREATE:
	{
	    BOOL got_getminmaxinfo = GetWindowLong(hwnd, 0) == MINMAX_MAGIC;
	    CREATESTRUCT FAR *cs = (CREATESTRUCT FAR *)lparam;

            trace("WM_NCCREATE: hwnd %04x, parent %04x, style %08lx\n", hwnd, cs->hwndParent, cs->style);
	    if (got_getminmaxinfo)
		trace("%04x got WM_GETMINMAXINFO\n", hwnd);

	    if ((cs->style & WS_THICKFRAME) || !(cs->style & (WS_POPUP | WS_CHILD)))
		ok(got_getminmaxinfo, "main: WM_GETMINMAXINFO should have been received before WM_NCCREATE\n");
	    else
		ok(!got_getminmaxinfo, "main: WM_GETMINMAXINFO should NOT have been received before WM_NCCREATE\n");
	    break;
	}
    }

    return DefWindowProc(hwnd, msg, wparam, lparam);
}

LRESULT CALLBACK __export tool_window_procA(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    switch (msg)
    {
	case WM_GETMINMAXINFO:
	{
	    SetWindowLong(hwnd, 0, MINMAX_MAGIC);
	    break;
	}
	case WM_NCCREATE:
	{
	    BOOL got_getminmaxinfo = GetWindowLong(hwnd, 0) == MINMAX_MAGIC;
	    CREATESTRUCT FAR *cs = (CREATESTRUCT FAR *)lparam;

            trace("WM_NCCREATE: hwnd %04x, parent %04x, style %08lx\n", hwnd, cs->hwndParent, cs->style);
	    if (got_getminmaxinfo)
		trace("%04x got WM_GETMINMAXINFO\n", hwnd);

	    if ((cs->style & WS_THICKFRAME) || !(cs->style & (WS_POPUP | WS_CHILD)))
		ok(got_getminmaxinfo, "tool: WM_GETMINMAXINFO should have been received before WM_NCCREATE\n");
	    else
		ok(!got_getminmaxinfo, "tool: WM_GETMINMAXINFO should NOT have been received before WM_NCCREATE\n");
	    break;
	}
    }

    return DefWindowProc(hwnd, msg, wparam, lparam);
}

static BOOL RegisterWindowClasses(void)
{
    WNDCLASS cls;

    cls.style = CS_DBLCLKS;
    cls.lpfnWndProc = main_window_procA;
    cls.cbClsExtra = 0;
    cls.cbWndExtra = sizeof(LONG);
    cls.hInstance = main_instance;
    cls.hIcon = 0;
    cls.hCursor = LoadCursor(0, IDC_ARROW);
    cls.hbrBackground = GetStockObject(WHITE_BRUSH);
    cls.lpszMenuName = NULL;
    cls.lpszClassName = "MainWindowClass";

    if(!RegisterClass(&cls)) return FALSE;

    cls.style = 0;
    cls.lpfnWndProc = tool_window_procA;
    cls.cbClsExtra = 0;
    cls.cbWndExtra = sizeof(LONG);
    cls.hInstance = main_instance;
    cls.hIcon = 0;
    cls.hCursor = LoadCursor(0, IDC_ARROW);
    cls.hbrBackground = GetStockObject(WHITE_BRUSH);
    cls.lpszMenuName = NULL;
    cls.lpszClassName = "ToolWindowClass";

    if(!RegisterClass(&cls)) return FALSE;

    return TRUE;
}

static void FixedAdjustWindowRectEx(RECT* rc, LONG style, BOOL menu, LONG exstyle)
{
    AdjustWindowRectEx(rc, style, menu, exstyle);
    /* AdjustWindowRectEx does not include scroll bars */
    if (style & WS_VSCROLL)
        rc->right += GetSystemMetrics(SM_CXVSCROLL);
    if (style & WS_HSCROLL)
	rc->bottom += GetSystemMetrics(SM_CYHSCROLL);
}

static void test_nonclient_area(HWND hwnd)
{
    DWORD style, exstyle;
    RECT rc_window, rc_client, rc;
    BOOL menu;

    style = GetWindowLong(hwnd, GWL_STYLE);
    exstyle = GetWindowLong(hwnd, GWL_EXSTYLE);
    menu = !(style & WS_CHILD) && GetMenu(hwnd) != 0;

    GetWindowRect(hwnd, &rc_window);
    GetClientRect(hwnd, &rc_client);

    /* avoid some cases when things go wrong */
    if (IsRectEmpty(&rc_window) || IsRectEmpty(&rc_client)) return;

    CopyRect(&rc, &rc_client);
    MapWindowPoints(hwnd, 0, (LPPOINT)&rc, 2);
    FixedAdjustWindowRectEx(&rc, style, menu, exstyle);

    ok(EqualRect(&rc, &rc_window),
       "window rect does not match: style:exstyle=0x%08lx:0x%08lx, menu=%d, win=(%d,%d)-(%d,%d), calc=(%d,%d)-(%d,%d)\n",
       style, exstyle, menu, rc_window.left, rc_window.top, rc_window.right, rc_window.bottom,
       rc.left, rc.top, rc.right, rc.bottom);


    CopyRect(&rc, &rc_window);
    DefWindowProc(hwnd, WM_NCCALCSIZE, 0, (LPARAM)(LPRECT)&rc);
    MapWindowPoints(0, hwnd, (LPPOINT)&rc, 2);
    ok(EqualRect(&rc, &rc_client),
       "client rect does not match: style:exstyle=0x%08lx:0x%08lx, menu=%d client=(%d,%d)-(%d,%d), calc=(%d,%d)-(%d,%d)\n",
       style, exstyle, menu, rc_client.left, rc_client.top, rc_client.right, rc_client.bottom,
       rc.left, rc.top, rc.right, rc.bottom);
}

LRESULT CALLBACK __export nccalcsize_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    if (msg == WM_NCCALCSIZE)
    {
        RECT FAR *rect = (RECT FAR *)lparam;
        /* first time around increase the rectangle, next time decrease it */
        if (rect->left == 100) InflateRect( rect, 10, 10 );
        else InflateRect( rect, -10, -10 );
        return 0;
    }
    return DefWindowProc( hwnd, msg, wparam, lparam );
}

static void test_SetWindowPos(HWND hwnd)
{
    RECT orig_win_rc, rect;
    LONG old_proc;

    GetWindowRect(hwnd, &orig_win_rc);

    old_proc = SetWindowLong( hwnd, GWL_WNDPROC, (LONG)nccalcsize_proc );
    SetWindowPos(hwnd, 0, 100, 100, 0, 0, SWP_NOZORDER|SWP_FRAMECHANGED);
    GetWindowRect( hwnd, &rect );
    ok( rect.left == 100 && rect.top == 100 && rect.right == 100 && rect.bottom == 100,
        "invalid window rect %d,%d-%d,%d\n", rect.left, rect.top, rect.right, rect.bottom );
    GetClientRect( hwnd, &rect );
    MapWindowPoints( hwnd, 0, (POINT FAR *)&rect, 2 );
    ok( rect.left == 90 && rect.top == 90 && rect.right == 110 && rect.bottom == 110,
        "invalid client rect %d,%d-%d,%d\n", rect.left, rect.top, rect.right, rect.bottom );

    SetWindowPos(hwnd, 0, 200, 200, 0, 0, SWP_NOZORDER|SWP_FRAMECHANGED);
    GetWindowRect( hwnd, &rect );
    ok( rect.left == 200 && rect.top == 200 && rect.right == 200 && rect.bottom == 200,
        "invalid window rect %d,%d-%d,%d\n", rect.left, rect.top, rect.right, rect.bottom );
    GetClientRect( hwnd, &rect );
    MapWindowPoints( hwnd, 0, (POINT FAR *)&rect, 2 );
    ok( rect.left == 210 && rect.top == 210 && rect.right == 190 && rect.bottom == 190,
        "invalid client rect %d,%d-%d,%d\n", rect.left, rect.top, rect.right, rect.bottom );

    SetWindowPos(hwnd, 0, orig_win_rc.left, orig_win_rc.top,
                 orig_win_rc.right, orig_win_rc.bottom, 0);
    SetWindowLong( hwnd, GWL_WNDPROC, old_proc );

    SetWindowPos(hwnd, 0, orig_win_rc.left, orig_win_rc.top,
                 orig_win_rc.right, orig_win_rc.bottom, 0);

    ok(!(GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST), "WS_EX_TOPMOST should not be set\n");
    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOSIZE|SWP_NOMOVE);
    ok(GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST, "WS_EX_TOPMOST should be set\n");
    SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOSIZE|SWP_NOMOVE);
    ok(GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST, "WS_EX_TOPMOST should be set\n");
    SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOSIZE|SWP_NOMOVE);
    ok(!(GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST), "WS_EX_TOPMOST should not be set\n");
}

/* MoveWindow and a child's rectangles (added for Win16) */
static void test_MoveWindow(HWND parent)
{
    HWND child;
    RECT rc;
    POINT origin;

    origin.x = origin.y = 0;
    ClientToScreen(parent, &origin);

    child = CreateWindow("static", NULL, WS_CHILD | WS_VISIBLE, 10, 20, 30, 40,
                         parent, (HMENU)5, main_instance, NULL);
    ok(child != 0, "CreateWindow failed\n");
    GetWindowRect(child, &rc);
    OffsetRect(&rc, -origin.x, -origin.y);
    ok(rc.left == 10 && rc.top == 20 && rc.right == 40 && rc.bottom == 60,
       "invalid window rect %d,%d-%d,%d\n", rc.left, rc.top, rc.right, rc.bottom);
    GetClientRect(child, &rc);
    ok(rc.left == 0 && rc.top == 0 && rc.right == 30 && rc.bottom == 40,
       "invalid client rect %d,%d-%d,%d\n", rc.left, rc.top, rc.right, rc.bottom);

    ok(MoveWindow(child, 5, 6, 70, 80, TRUE), "MoveWindow failed\n");
    GetWindowRect(child, &rc);
    MapWindowPoints(0, parent, (POINT FAR *)&rc, 2);
    ok(rc.left == 5 && rc.top == 6 && rc.right == 75 && rc.bottom == 86,
       "invalid window rect %d,%d-%d,%d after MoveWindow\n", rc.left, rc.top, rc.right, rc.bottom);
    GetClientRect(child, &rc);
    ok(rc.right == 70 && rc.bottom == 80,
       "invalid client rect %d,%d-%d,%d after MoveWindow\n", rc.left, rc.top, rc.right, rc.bottom);

    ok(SetWindowPos(child, 0, 1, 2, 0, 0, SWP_NOSIZE | SWP_NOZORDER), "SetWindowPos failed\n");
    GetWindowRect(child, &rc);
    MapWindowPoints(0, parent, (POINT FAR *)&rc, 2);
    ok(rc.left == 1 && rc.top == 2 && rc.right == 71 && rc.bottom == 82,
       "invalid window rect %d,%d-%d,%d after SWP_NOSIZE\n", rc.left, rc.top, rc.right, rc.bottom);
    ok(SetWindowPos(child, 0, 50, 50, 11, 12, SWP_NOMOVE | SWP_NOZORDER), "SetWindowPos failed\n");
    GetWindowRect(child, &rc);
    MapWindowPoints(0, parent, (POINT FAR *)&rc, 2);
    ok(rc.left == 1 && rc.top == 2 && rc.right == 12 && rc.bottom == 14,
       "invalid window rect %d,%d-%d,%d after SWP_NOMOVE\n", rc.left, rc.top, rc.right, rc.bottom);

    ok(GetDlgCtrlID(child) == 5, "GetDlgCtrlID is %d\n", GetDlgCtrlID(child));
    ok(GetWindowWord(child, GWW_ID) == 5, "GWW_ID is %d\n", GetWindowWord(child, GWW_ID));
    ok(GetDlgItem(parent, 5) == child, "GetDlgItem is %04x, not %04x\n", GetDlgItem(parent, 5), child);
    DestroyWindow(child);
}

static void test_window_tree(HWND parent, const DWORD *style, const int *order, int total)
{
    HWND child[5], hwnd;
    int i;

    hwnd = GetWindow(parent, GW_CHILD);
    ok(!hwnd, "have to start without children to perform the test\n");

    for (i = 0; i < total; i++)
    {
        child[i] = CreateWindow("static", "", style[i], 0,0,10,10,
                                parent, (HMENU)i, main_instance, NULL);
        trace("child[%d] = %04x\n", i, child[i]);
        ok(child[i] != 0, "CreateWindowEx failed to create child window\n");
    }

    hwnd = GetWindow(parent, GW_CHILD);
    ok(hwnd != 0, "GetWindow(GW_CHILD) failed\n");
    ok(hwnd == GetWindow(child[total - 1], GW_HWNDFIRST), "GW_HWNDFIRST is wrong\n");
    ok(child[order[total - 1]] == GetWindow(child[0], GW_HWNDLAST), "GW_HWNDLAST is wrong\n");

    for (i = 0; i < total; i++)
    {
        trace("hwnd[%d] = %04x\n", i, hwnd);
        ok(child[order[i]] == hwnd, "Z order of child #%d is wrong\n", i);

        hwnd = GetWindow(hwnd, GW_HWNDNEXT);
    }

    for (i = 0; i < total; i++)
        ok(DestroyWindow(child[i]), "DestroyWindow failed\n");
}

static void test_children_zorder(HWND parent)
{
    const DWORD simple_style[5] = { WS_CHILD, WS_CHILD, WS_CHILD, WS_CHILD,
                                    WS_CHILD };
    const int simple_order[5] = { 0, 1, 2, 3, 4 };

    const DWORD complex_style[5] = { WS_CHILD, WS_CHILD | WS_MAXIMIZE,
                             WS_CHILD | WS_VISIBLE, WS_CHILD,
                             WS_CHILD | WS_MAXIMIZE | WS_VISIBLE };
    const int complex_order_1[1] = { 0 };
    const int complex_order_2[2] = { 1, 0 };
    const int complex_order_3[3] = { 1, 0, 2 };
    const int complex_order_4[4] = { 1, 0, 2, 3 };
    const int complex_order_5[5] = { 4, 1, 0, 2, 3 };

    /* simple WS_CHILD */
    test_window_tree(parent, simple_style, simple_order, 5);

    /* complex children styles */
    test_window_tree(parent, complex_style, complex_order_1, 1);
    test_window_tree(parent, complex_style, complex_order_2, 2);
    test_window_tree(parent, complex_style, complex_order_3, 3);
    test_window_tree(parent, complex_style, complex_order_4, 4);
    test_window_tree(parent, complex_style, complex_order_5, 5);
}

static void test_SetParent(void)
{
    BOOL ret;
    HWND desktop = GetDesktopWindow();
    HMENU hMenu;
    HWND parent, child1, child2, child3, child4, sibling;

    parent = CreateWindow("static", NULL, WS_OVERLAPPEDWINDOW,
			     100, 100, 200, 200, 0, 0, main_instance, NULL);
    ok(parent != 0, "parent creation failed\n");
    child1 = CreateWindow("static", NULL, WS_CHILD,
			     0, 0, 50, 50, parent, 0, main_instance, NULL);
    ok(child1 != 0, "child1 creation failed\n");
    child2 = CreateWindow("static", NULL, WS_POPUP,
			     0, 0, 50, 50, child1, 0, main_instance, NULL);
    ok(child2 != 0, "child2 creation failed\n");
    child3 = CreateWindow("static", NULL, WS_CHILD,
			     0, 0, 50, 50, child2, 0, main_instance, NULL);
    ok(child3 != 0, "child3 creation failed\n");
    child4 = CreateWindow("static", NULL, WS_POPUP,
			     0, 0, 50, 50, child3, 0, main_instance, NULL);
    ok(child4 != 0, "child4 creation failed\n");

    trace("parent %04x, child1 %04x, child2 %04x, child3 %04x, child4 %04x\n",
	   parent, child1, child2, child3, child4);

    check_parents(parent, desktop, 0, 0, 0, parent, parent);
    check_parents(child1, parent, parent, parent, 0, parent, parent);
    check_parents(child2, desktop, parent, parent, parent, child2, parent);
    check_parents(child3, child2, child2, child2, 0, child2, parent);
    check_parents(child4, desktop, child2, child2, child2, child4, parent);

    ok(!IsChild(desktop, parent), "wrong parent/child %04x/%04x\n", desktop, parent);
    ok(!IsChild(desktop, child1), "wrong parent/child %04x/%04x\n", desktop, child1);
    ok(!IsChild(desktop, child2), "wrong parent/child %04x/%04x\n", desktop, child2);
    ok(!IsChild(desktop, child3), "wrong parent/child %04x/%04x\n", desktop, child3);
    ok(!IsChild(desktop, child4), "wrong parent/child %04x/%04x\n", desktop, child4);

    ok(IsChild(parent, child1), "wrong parent/child %04x/%04x\n", parent, child1);
    ok(!IsChild(desktop, child2), "wrong parent/child %04x/%04x\n", desktop, child2);
    ok(!IsChild(parent, child2), "wrong parent/child %04x/%04x\n", parent, child2);
    ok(!IsChild(child1, child2), "wrong parent/child %04x/%04x\n", child1, child2);
    ok(!IsChild(parent, child3), "wrong parent/child %04x/%04x\n", parent, child3);
    ok(IsChild(child2, child3), "wrong parent/child %04x/%04x\n", child2, child3);
    ok(!IsChild(parent, child4), "wrong parent/child %04x/%04x\n", parent, child4);
    ok(!IsChild(child3, child4), "wrong parent/child %04x/%04x\n", child3, child4);
    ok(!IsChild(desktop, child4), "wrong parent/child %04x/%04x\n", desktop, child4);

    hMenu = CreateMenu();
    sibling = CreateWindow("static", NULL, WS_OVERLAPPEDWINDOW,
			     100, 100, 200, 200, 0, hMenu, main_instance, NULL);
    ok(sibling != 0, "sibling creation failed\n");

    ok(SetParent(sibling, parent) != 0, "SetParent should not fail\n");
    ok(GetMenu(sibling) == hMenu, "SetParent should not remove menu\n");

    ret = DestroyWindow(parent);
    ok( ret, "DestroyWindow() failed\n");

    ok(!IsWindow(parent), "parent still exists\n");
    ok(!IsWindow(sibling), "sibling still exists\n");
    ok(!IsWindow(child1), "child1 still exists\n");
    ok(!IsWindow(child2), "child2 still exists\n");
    ok(!IsWindow(child3), "child3 still exists\n");
    ok(!IsWindow(child4), "child4 still exists\n");
}

/* Wine's STYLESTRUCT, which Windows 3.1 doesn't have */
typedef struct
{
    DWORD styleOld;
    DWORD styleNew;
} STYLES;

LRESULT CALLBACK __export StyleCheckProc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    CREATESTRUCT FAR *lpcs;
    STYLES FAR *lpss;

    switch (msg)
    {
    case WM_NCCREATE:
    case WM_CREATE:
        lpcs = (CREATESTRUCT FAR *)lparam;
        lpss = (STYLES FAR *)lpcs->lpCreateParams;
        if (lpss)
        {
            ok(lpss->styleOld == lpcs->dwExStyle,
                "Ex style (0x%08lx) should match what the caller passed to CreateWindowEx (0x%08lx)\n",
                lpss->styleOld, lpcs->dwExStyle);

            ok(lpss->styleNew == lpcs->style,
                "Style (0x%08lx) should match what the caller passed to CreateWindowEx (0x%08lx)\n",
                lpss->styleNew, lpcs->style);
        }
        break;
    }
    return DefWindowProc(hwnd, msg, wparam, lparam);
}

static void register_style_check_class(void)
{
    WNDCLASS wc;

    memset(&wc, 0, sizeof(wc));
    wc.lpfnWndProc = StyleCheckProc;
    wc.hInstance = main_instance;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE+1);
    wc.lpszClassName = "WineStyleCheck";

    ok(RegisterClass(&wc), "RegisterClass failed\n");
}

static void check_window_style(DWORD dwStyleIn, DWORD dwExStyleIn, DWORD dwStyleOut, DWORD dwExStyleOut)
{
    DWORD dwActualStyle;
    DWORD dwActualExStyle;
    STYLES ss;
    HWND hwnd;
    HWND hwndParent = NULL;

    ss.styleNew = dwStyleIn;
    ss.styleOld = dwExStyleIn;

    if (dwStyleIn & WS_CHILD)
    {
        hwndParent = CreateWindowEx(0, "WineStyleCheck", NULL,
            WS_OVERLAPPEDWINDOW, 0, 0, 0, 0, NULL, NULL, main_instance, NULL);
    }

    hwnd = CreateWindowEx(dwExStyleIn, "WineStyleCheck", NULL,
                    dwStyleIn, 0, 0, 0, 0, hwndParent, NULL, main_instance, &ss);
    ok(hwnd != 0, "CreateWindowEx failed for style %08lx/%08lx\n", dwStyleIn, dwExStyleIn);
    if (!hwnd) return;

    dwActualStyle = GetWindowLong(hwnd, GWL_STYLE);
    dwActualExStyle = GetWindowLong(hwnd, GWL_EXSTYLE);
    ok((dwActualStyle == dwStyleOut) && (dwActualExStyle == dwExStyleOut),
        "Style (0x%08lx) should really be 0x%08lx and/or Ex style (0x%08lx) should really be 0x%08lx\n",
        dwActualStyle, dwStyleOut, dwActualExStyle, dwExStyleOut);

    DestroyWindow(hwnd);
    if (hwndParent) DestroyWindow(hwndParent);
}

/* tests what window styles the window manager automatically adds */
static void test_window_styles(void)
{
    register_style_check_class();

    check_window_style(0, 0, WS_CLIPSIBLINGS|WS_CAPTION, 0);
    check_window_style(WS_OVERLAPPEDWINDOW, 0, WS_CLIPSIBLINGS|WS_OVERLAPPEDWINDOW, 0);
    check_window_style(WS_CHILD, 0, WS_CHILD, 0);
    check_window_style(WS_POPUP, 0, WS_POPUP|WS_CLIPSIBLINGS, 0);
    check_window_style(WS_CHILD, WS_EX_DLGMODALFRAME, WS_CHILD, WS_EX_DLGMODALFRAME);
}

static void test_AWRwindow(LPCSTR class, LONG style, LONG exStyle, BOOL menu)
{
    HWND hwnd = 0;

    hwnd = CreateWindowEx(exStyle, class, class, style,
			  110, 100,
			  225, 200,
			  0,
			  menu ? hmenu : 0,
			  main_instance, 0);
    if (!hwnd) {
	trace("Failed to create window class=%s, style=0x%08lx, exStyle=0x%08lx\n", class, style, exStyle);
        return;
    }
    ShowWindow(hwnd, SW_SHOW);

    test_nonclient_area(hwnd);

    SetMenu(hwnd, 0);
    DestroyWindow(hwnd);
}

static BOOL AWR_init(void)
{
    WNDCLASS class;

    class.style         = CS_HREDRAW | CS_VREDRAW;
    class.lpfnWndProc     = (WNDPROC)DefWindowProc;
    class.cbClsExtra    = 0;
    class.cbWndExtra    = 0;
    class.hInstance     = main_instance;
    class.hIcon         = LoadIcon (0, IDI_APPLICATION);
    class.hCursor       = LoadCursor (0, IDC_ARROW);
    class.hbrBackground = 0;
    class.lpszMenuName  = 0;
    class.lpszClassName = szAWRClass;

    if (!RegisterClass (&class)) {
	ok(FALSE, "RegisterClass failed\n");
	return FALSE;
    }

    hmenu = CreateMenu();
    if (!hmenu)
	return FALSE;
    ok(hmenu != 0, "Failed to create menu\n");
    ok(AppendMenu(hmenu, MF_STRING, 1, "Test!"), "Failed to create menu item\n");

    return TRUE;
}

static void test_AWR_window_size(BOOL menu)
{
    LONG styles[] = {
	WS_POPUP,
	WS_MAXIMIZE, WS_BORDER, WS_DLGFRAME,
	WS_SYSMENU,
	WS_THICKFRAME,
	WS_MINIMIZEBOX, WS_MAXIMIZEBOX,
	WS_HSCROLL, WS_VSCROLL
    };

    int i;

    /* A exhaustive check of all the styles takes too long
     * so just do a (hopefully representative) sample
     */
    for (i = 0; i < COUNTOF(styles); ++i)
        test_AWRwindow(szAWRClass, styles[i], 0, menu);
}

#define SHOWSYSMETRIC(SM) trace(#SM "=%d\n", GetSystemMetrics(SM))

static void test_AdjustWindowRect(void)
{
    if (!AWR_init())
	return;

    SHOWSYSMETRIC(SM_CYCAPTION);
    SHOWSYSMETRIC(SM_CYMENU);
    SHOWSYSMETRIC(SM_CXVSCROLL);
    SHOWSYSMETRIC(SM_CYHSCROLL);
    SHOWSYSMETRIC(SM_CXFRAME);
    SHOWSYSMETRIC(SM_CYFRAME);
    SHOWSYSMETRIC(SM_CXDLGFRAME);
    SHOWSYSMETRIC(SM_CYDLGFRAME);
    SHOWSYSMETRIC(SM_CXBORDER);
    SHOWSYSMETRIC(SM_CYBORDER);

    test_AWR_window_size(FALSE);
    test_AWR_window_size(TRUE);

    DestroyMenu(hmenu);
}
#undef SHOWSYSMETRIC

static void test_CreateWindow(void)
{
    HWND hwnd, parent;
    HMENU hmenu;

#define expect_menu(window, menu) \
    ok(GetMenu(window) == (HMENU)menu, "GetMenu returned %04x\n", GetMenu(window))

#define expect_style(window, style)\
    ok((DWORD)GetWindowLong(window, GWL_STYLE) == (style), "expected style %lx != %lx\n", (LONG)(style), GetWindowLong(window, GWL_STYLE))

#define expect_ex_style(window, ex_style)\
    ok((DWORD)GetWindowLong(window, GWL_EXSTYLE) == (ex_style), "expected ex_style %lx != %lx\n", (LONG)(ex_style), GetWindowLong(window, GWL_EXSTYLE))

    hmenu = CreateMenu();
    ok(hmenu != 0, "CreateMenu failed\n");
    parent = GetDesktopWindow();
    ok(parent != 0, "GetDesktopWindow failed\n");

    ok(IsMenu(hmenu), "IsMenu failed\n");

    /* WS_CHILD */
    hwnd = CreateWindowEx(0, "static", NULL, WS_CHILD,
                           0, 0, 100, 100, parent, (HMENU)1, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, 1);
    expect_style(hwnd, WS_CHILD);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);

    hwnd = CreateWindowEx(0, "static", NULL, WS_CHILD | WS_CAPTION,
                           0, 0, 100, 100, parent, (HMENU)1, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, 1);
    expect_style(hwnd, WS_CHILD | WS_CAPTION);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);

    /* WS_POPUP */
    hwnd = CreateWindowEx(0, "static", NULL, WS_POPUP,
                           0, 0, 100, 100, parent, hmenu, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, hmenu);
    expect_style(hwnd, WS_POPUP | WS_CLIPSIBLINGS);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);
    ok(!IsMenu(hmenu), "IsMenu should fail\n");

    hmenu = CreateMenu();
    ok(hmenu != 0, "CreateMenu failed\n");
    hwnd = CreateWindowEx(0, "static", NULL, WS_POPUP | WS_CAPTION,
                           0, 0, 100, 100, parent, hmenu, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, hmenu);
    expect_style(hwnd, WS_POPUP | WS_CAPTION | WS_CLIPSIBLINGS);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);
    ok(!IsMenu(hmenu), "IsMenu should fail\n");

    /* WS_CHILD | WS_POPUP */
    hmenu = CreateMenu();
    ok(hmenu != 0, "CreateMenu failed\n");
    hwnd = CreateWindowEx(0, "static", NULL, WS_CHILD | WS_POPUP,
                           0, 0, 100, 100, parent, hmenu, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, hmenu);
    expect_style(hwnd, WS_CHILD | WS_POPUP | WS_CLIPSIBLINGS);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);
    ok(!IsMenu(hmenu), "IsMenu should fail\n");

    hmenu = CreateMenu();
    ok(hmenu != 0, "CreateMenu failed\n");
    hwnd = CreateWindowEx(0, "static", NULL, WS_CHILD | WS_POPUP | WS_CAPTION,
                           0, 0, 100, 100, parent, hmenu, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    expect_menu(hwnd, hmenu);
    expect_style(hwnd, WS_CHILD | WS_POPUP | WS_CAPTION | WS_CLIPSIBLINGS);
    expect_ex_style(hwnd, 0);
    DestroyWindow(hwnd);
    ok(!IsMenu(hmenu), "IsMenu should fail\n");

#undef expect_menu
#undef expect_style
#undef expect_ex_style
}

static void test_ShowWindow(void)
{
    HWND hwnd;
    DWORD style;
    RECT rcMain, rc;
    LRESULT ret;

    SetRect(&rcMain, 120, 120, 210, 210);

    hwnd = CreateWindowEx(0, "MainWindowClass", NULL,
                          WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX |
                          WS_MAXIMIZEBOX | WS_POPUP,
                          rcMain.left, rcMain.top,
                          rcMain.right - rcMain.left, rcMain.bottom - rcMain.top,
                          0, 0, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");

    style = GetWindowLong(hwnd, GWL_STYLE);
    ok(!(style & WS_DISABLED), "window should not be disabled\n");
    ok(!(style & WS_VISIBLE), "window should not be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = ShowWindow(hwnd, SW_SHOW);
    ok(!ret, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok(!(style & WS_DISABLED), "window should not be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = ShowWindow(hwnd, SW_MINIMIZE);
    ok(ret != 0, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok(!(style & WS_DISABLED), "window should not be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok((style & WS_MINIMIZE) != 0, "window should be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    ok(IsIconic(hwnd), "IsIconic should be TRUE\n");
    GetWindowRect(hwnd, &rc);
    ok(!EqualRect(&rcMain, &rc), "rects shouldn't match\n");

    ShowWindow(hwnd, SW_RESTORE);
    ok(ret != 0, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok(!(style & WS_DISABLED), "window should not be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = EnableWindow(hwnd, FALSE);
    ok(!ret, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok(!IsWindowEnabled(hwnd), "IsWindowEnabled should be FALSE\n");

    ret = DefWindowProc(hwnd, WM_SYSCOMMAND, SC_MINIMIZE, 0);
    ok(!ret, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = DefWindowProc(hwnd, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
    ok(!ret, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = ShowWindow(hwnd, SW_MINIMIZE);
    ok(ret != 0, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok((style & WS_MINIMIZE) != 0, "window should be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(!EqualRect(&rcMain, &rc), "rects shouldn't match\n");

    ret = DefWindowProc(hwnd, WM_SYSCOMMAND, SC_RESTORE, 0);
    ok(!ret, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok((style & WS_MINIMIZE) != 0, "window should be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(!EqualRect(&rcMain, &rc), "rects shouldn't match\n");

    ret = ShowWindow(hwnd, SW_RESTORE);
    ok(ret != 0, "not expected ret: %lu\n", ret);
    style = GetWindowLong(hwnd, GWL_STYLE);
    ok((style & WS_DISABLED) != 0, "window should be disabled\n");
    ok((style & WS_VISIBLE) != 0, "window should be visible\n");
    ok(!(style & WS_MINIMIZE), "window should not be minimized\n");
    ok(!(style & WS_MAXIMIZE), "window should not be maximized\n");
    GetWindowRect(hwnd, &rc);
    ok(EqualRect(&rcMain, &rc), "expected (%d,%d)-(%d,%d), got (%d,%d)-(%d,%d)\n",
       rcMain.left, rcMain.top, rcMain.right, rcMain.bottom,
       rc.left, rc.top, rc.right, rc.bottom);

    ret = DefWindowProc(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
    ok(!ret, "not expected ret: %lu\n", ret);
    ok(IsWindow(hwnd), "window should exist\n");

    ret = EnableWindow(hwnd, TRUE);
    ok(ret != 0, "not expected ret: %lu\n", ret);

    ret = DefWindowProc(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
    ok(!ret, "not expected ret: %lu\n", ret);
    ok(!IsWindow(hwnd), "window should not exist\n");
}

/* Subclassing through GWL_WNDPROC and the window words (added for
 * Win16). */
static WNDPROC old_subclass_proc;
static int subclass_calls;

LRESULT CALLBACK __export base_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    if (msg == WM_USER + 1) return 42;
    if (msg == WM_USER + 2) return MAKELRESULT(wparam, 43);
    return DefWindowProc(hwnd, msg, wparam, lparam);
}

LRESULT CALLBACK __export subclass_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    if (msg == WM_USER + 1)
    {
        subclass_calls++;
        return 0x77;
    }
    return CallWindowProc((FARPROC)old_subclass_proc, hwnd, msg, wparam + 1, lparam);
}

static void test_subclass(void)
{
    WNDCLASS cls;
    HWND hwnd;
    LONG ret;

    memset(&cls, 0, sizeof(cls));
    cls.lpfnWndProc = base_proc;
    cls.cbWndExtra = 6;
    cls.hInstance = main_instance;
    cls.lpszClassName = "SubclassTest";
    ok(RegisterClass(&cls), "RegisterClass failed\n");

    hwnd = CreateWindow("SubclassTest", "sub", WS_OVERLAPPED, 0, 0, 50, 50,
                        0, 0, main_instance, NULL);
    ok(hwnd != 0, "CreateWindow failed\n");

    ok(GetWindowLong(hwnd, GWL_WNDPROC) == (LONG)base_proc,
       "GWL_WNDPROC is %08lx, not %08lx\n", GetWindowLong(hwnd, GWL_WNDPROC), (LONG)base_proc);
    ok(SendMessage(hwnd, WM_USER + 1, 0, 0) == 42, "WM_USER+1 didn't reach the class procedure\n");

    old_subclass_proc = (WNDPROC)SetWindowLong(hwnd, GWL_WNDPROC, (LONG)subclass_proc);
    ok((LONG)old_subclass_proc == (LONG)base_proc,
       "SetWindowLong returned %08lx, not %08lx\n", (LONG)old_subclass_proc, (LONG)base_proc);
    ok(GetWindowLong(hwnd, GWL_WNDPROC) == (LONG)subclass_proc,
       "GWL_WNDPROC is %08lx, not %08lx\n", GetWindowLong(hwnd, GWL_WNDPROC), (LONG)subclass_proc);
    ok(GetClassLong(hwnd, GCL_WNDPROC) == (LONG)base_proc,
       "GCL_WNDPROC changed to %08lx\n", GetClassLong(hwnd, GCL_WNDPROC));

    ret = SendMessage(hwnd, WM_USER + 1, 0, 0);
    ok(ret == 0x77, "subclass procedure returned %lx\n", ret);
    ok(subclass_calls == 1, "subclass procedure called %d times\n", subclass_calls);
    ret = SendMessage(hwnd, WM_USER + 2, 5, 0);
    ok(ret == MAKELRESULT(6, 43), "CallWindowProc to the old procedure returned %08lx\n", ret);
    ret = CallWindowProc((FARPROC)old_subclass_proc, hwnd, WM_USER + 1, 0, 0);
    ok(ret == 42, "CallWindowProc returned %ld\n", ret);

    ok(SetWindowLong(hwnd, GWL_WNDPROC, (LONG)old_subclass_proc) == (LONG)subclass_proc,
       "SetWindowLong didn't return the subclass procedure\n");
    ok(SendMessage(hwnd, WM_USER + 1, 0, 0) == 42, "WM_USER+1 didn't reach the class procedure\n");
    ok(subclass_calls == 1, "subclass procedure called %d times\n", subclass_calls);

    /* the window's own bytes, by word and by long */
    ok(GetWindowWord(hwnd, 0) == 0, "initial window word %x\n", GetWindowWord(hwnd, 0));
    ok(SetWindowWord(hwnd, 0, 0x1234) == 0, "SetWindowWord didn't return 0\n");
    ok(SetWindowWord(hwnd, 2, 0x5678) == 0, "SetWindowWord didn't return 0\n");
    ok(GetWindowLong(hwnd, 0) == 0x56781234L, "window long %08lx\n", GetWindowLong(hwnd, 0));
    ok(SetWindowLong(hwnd, 2, 0x9abcdef0L) == 0x5678L, "SetWindowLong didn't return 0x5678\n");
    ok(GetWindowWord(hwnd, 4) == 0x9abc, "window word 4 is %x\n", GetWindowWord(hwnd, 4));

    ok((HINSTANCE)GetWindowWord(hwnd, GWW_HINSTANCE) == main_instance,
       "GWW_HINSTANCE is %04x, not %04x\n", GetWindowWord(hwnd, GWW_HINSTANCE), main_instance);
    ok((GetWindowLong(hwnd, GWL_STYLE) & ~WS_CLIPSIBLINGS) == WS_CAPTION,
       "GWL_STYLE is %08lx\n", GetWindowLong(hwnd, GWL_STYLE));
    SetWindowLong(hwnd, GWL_STYLE, GetWindowLong(hwnd, GWL_STYLE) | WS_DISABLED);
    ok(!IsWindowEnabled(hwnd), "WS_DISABLED through SetWindowLong didn't disable\n");

    DestroyWindow(hwnd);
    ok(UnregisterClass("SubclassTest", main_instance), "UnregisterClass failed\n");
}

/* Window text and FindWindow (added for Win16) */
static void test_text(void)
{
    char buf[32];
    HWND hwnd;
    int len;
    LRESULT r;

    hwnd = CreateWindow("ToolWindowClass", "First title", WS_POPUP, 0, 0, 50, 50,
                        0, 0, main_instance, NULL);
    ok(hwnd != 0, "CreateWindow failed\n");

    len = GetWindowText(hwnd, buf, sizeof(buf));
    ok(len == 11 && !lstrcmp(buf, "First title"), "GetWindowText gave %d [%s]\n", len, (LPSTR)buf);
    ok(GetWindowTextLength(hwnd) == 11, "GetWindowTextLength is %d\n", GetWindowTextLength(hwnd));

    SetWindowText(hwnd, "Second");
    len = GetWindowText(hwnd, buf, sizeof(buf));
    ok(len == 6 && !lstrcmp(buf, "Second"), "GetWindowText gave %d [%s]\n", len, (LPSTR)buf);

    /* truncated to the buffer, with its terminator */
    len = GetWindowText(hwnd, buf, 4);
    ok(len == 3 && !lstrcmp(buf, "Sec"), "GetWindowText gave %d [%s]\n", len, (LPSTR)buf);

    r = SendMessage(hwnd, WM_SETTEXT, 0, (LPARAM)(LPSTR)"Third one");
    ok(r != 0, "WM_SETTEXT returned 0\n");
    r = SendMessage(hwnd, WM_GETTEXTLENGTH, 0, 0);
    ok(r == 9, "WM_GETTEXTLENGTH returned %ld\n", r);
    r = SendMessage(hwnd, WM_GETTEXT, sizeof(buf), (LPARAM)(LPSTR)buf);
    ok(r == 9 && !lstrcmp(buf, "Third one"), "WM_GETTEXT gave %ld [%s]\n", r, (LPSTR)buf);

    ok(FindWindow("ToolWindowClass", "Third one") == hwnd, "FindWindow by class and title failed\n");
    ok(FindWindow(NULL, "Third one") == hwnd, "FindWindow by title failed\n");
    ok(FindWindow("MainWindowClass", "Third one") == 0, "FindWindow found the wrong class\n");
    ok(FindWindow(NULL, "No such window title") == 0, "FindWindow found a missing title\n");
    ok(FindWindow("MainWindowClass", "Main window 2") == hwndMain2, "FindWindow didn't find main window 2\n");

    SetWindowText(hwnd, "");
    ok(GetWindowTextLength(hwnd) == 0, "GetWindowTextLength is %d\n", GetWindowTextLength(hwnd));
    buf[0] = 'x';
    len = GetWindowText(hwnd, buf, sizeof(buf));
    ok(len == 0 && !buf[0], "GetWindowText gave %d [%s]\n", len, (LPSTR)buf);

    DestroyWindow(hwnd);
    ok(FindWindow("ToolWindowClass", "") == 0, "FindWindow found a destroyed window\n");
}

START_TEST(win)
{
    /* A program's instance handle is its data segment's selector. */
    main_instance = (HINSTANCE)get_ds();
    enum_child_proc = MakeProcInstance((FARPROC)EnumChildProc, main_instance);
    enum_child_proc1 = MakeProcInstance((FARPROC)EnumChildProc1, main_instance);

    if (!RegisterWindowClasses())
    {
        ok(FALSE, "RegisterWindowClasses failed\n");
        return;
    }

    hwndMain = CreateWindowEx(0, "MainWindowClass", "Main window",
                               WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX |
                               WS_MAXIMIZEBOX | WS_POPUP,
                               100, 100, 200, 200,
                               0, 0, main_instance, NULL);
    hwndMain2 = CreateWindowEx(0, "MainWindowClass", "Main window 2",
                                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX |
                                WS_MAXIMIZEBOX | WS_POPUP,
                                100, 100, 200, 200,
                                0, 0, main_instance, NULL);
    ok(hwndMain != 0 && hwndMain2 != 0, "main windows not created\n");
    if (!hwndMain || !hwndMain2) return;

    test_CreateWindow();
    test_parent_owner();
    test_SetParent();
    test_enum_task_windows();

    test_SetWindowPos(hwndMain);
    test_MoveWindow(hwndMain);
    test_children_zorder(hwndMain);

    test_AdjustWindowRect();
    test_window_styles();
    test_ShowWindow();
    test_subclass();
    test_text();

    DestroyWindow(hwndMain2);
    DestroyWindow(hwndMain);
    FreeProcInstance(enum_child_proc);
    FreeProcInstance(enum_child_proc1);
}
