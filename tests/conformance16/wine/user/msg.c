/*
 * Unit tests for window message handling
 *
 * Copyright 1999 Ove Kaaven
 * Copyright 2003 Dimitrie O. Paun
 * Copyright 2004, 2005 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/user32/tests/msg.c, in part: its
 * checker of message sequences, with the messages CreateWindow sends
 * an overlapped window and a child window, those ShowWindow sends a
 * child and those DestroyWindow sends a child; PostQuitMessage and
 * WM_QUIT (test_quit_message, with PostAppMessage for
 * PostThreadMessage); and timers (test_timers and test_timers_no_wnd,
 * without the thread). Windows 3.1 has no CBT hook messages in Wine's
 * sense and no WinEvents, so the checker leaves those out, and no
 * threads, Unicode windows or Sleep. In Windows 3.1, SetTimer ignores
 * the id a timer without a window is given, so Wine's check that it
 * replaces the timer is left out. Added for Win16 are checks of the
 * order PostMessage, PostAppMessage and SendMessage deliver messages in,
 * PeekMessage's filters and PM_NOREMOVE, and GetMessage.
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

/* undocumented SWP flags - from SDK 3.1 */
#define SWP_NOCLIENTSIZE	0x0800
#define SWP_NOCLIENTMOVE	0x1000

static HINSTANCE main_instance;

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

typedef enum {
    sent=0x1,
    posted=0x2,
    parent=0x4,
    wparam=0x8,
    lparam=0x10,
    defwinproc=0x20,
    beginpaint=0x40,
    optional=0x80
} msg_flags_t;

struct message {
    UINT message;          /* the WM_* code */
    int flags;             /* message props */
    WPARAM wParam;         /* expected value of wParam */
    LPARAM lParam;         /* expected value of lParam */
    WPARAM wp_mask;        /* mask for wParam checks */
    LPARAM lp_mask;        /* mask for lParam checks */
};

struct recvd_message {
    UINT message;          /* the WM_* code */
    int flags;             /* message props */
    HWND hwnd;             /* window that received the message */
    WPARAM wParam;         /* expected value of wParam */
    LPARAM lParam;         /* expected value of lParam */
};

/* Empty message sequence */
static const struct message WmEmptySeq[] =
{
    { 0 }
};
/* CreateWindow (for overlapped window, not initially visible) (16/32) */
static const struct message WmCreateOverlappedSeq[] = {
    { WM_GETMINMAXINFO, sent },
    { WM_NCCREATE, sent },
    { WM_NCCALCSIZE, sent|wparam, 0 },
    { 0x0093, sent|defwinproc|optional },
    { 0x0094, sent|defwinproc|optional },
    { WM_CREATE, sent },
    { 0 }
};
/* CreateWindow (for child window, not initially visible) */
static const struct message WmCreateChildSeq[] = {
    { WM_NCCREATE, sent },
    /* child is inserted into parent's child list after WM_NCCREATE returns */
    { WM_NCCALCSIZE, sent|wparam, 0 },
    { WM_CREATE, sent },
    { WM_SIZE, sent|wparam, SIZE_RESTORED },
    { WM_MOVE, sent },
    { WM_PARENTNOTIFY, sent|parent|wparam, WM_CREATE },
    { 0 }
};
/* CreateWindow (for a child window, initially visible) */
static const struct message WmCreateVisibleChildSeq[] = {
    { WM_NCCREATE, sent },
    /* child is inserted into parent's child list after WM_NCCREATE returns */
    { WM_NCCALCSIZE, sent|wparam, 0 },
    { WM_CREATE, sent },
    { WM_SIZE, sent|wparam, SIZE_RESTORED },
    { WM_MOVE, sent },
    { WM_PARENTNOTIFY, sent|parent|wparam, WM_CREATE },
    { WM_SHOWWINDOW, sent|wparam, 1 },
    { WM_WINDOWPOSCHANGING, sent|wparam, SWP_SHOWWINDOW|SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE },
    { WM_ERASEBKGND, sent|parent|optional },
    { WM_WINDOWPOSCHANGED, sent|wparam, SWP_SHOWWINDOW|SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE|SWP_NOCLIENTSIZE|SWP_NOCLIENTMOVE },
    { WM_NCCALCSIZE, sent|wparam|optional, 1 }, /* WinXP */
    { 0 }
};
/* ShowWindow(SW_SHOW) for a not visible child window */
static const struct message WmShowChildSeq[] = {
    { WM_SHOWWINDOW, sent|wparam, 1 },
    { WM_WINDOWPOSCHANGING, sent|wparam, SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOSIZE|SWP_NOMOVE },
    { WM_ERASEBKGND, sent|parent|optional },
    { WM_WINDOWPOSCHANGED, sent|wparam, SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOSIZE|SWP_NOMOVE|SWP_NOCLIENTSIZE|SWP_NOCLIENTMOVE },
    { 0 }
};
/* ShowWindow(SW_HIDE) for a visible child window */
static const struct message WmHideChildSeq[] = {
    { WM_SHOWWINDOW, sent|wparam, 0 },
    { WM_WINDOWPOSCHANGING, sent|wparam, SWP_HIDEWINDOW|SWP_NOACTIVATE|SWP_NOSIZE|SWP_NOMOVE },
    { WM_ERASEBKGND, sent|parent|optional },
    { WM_WINDOWPOSCHANGED, sent|wparam, SWP_HIDEWINDOW|SWP_NOACTIVATE|SWP_NOSIZE|SWP_NOMOVE|SWP_NOCLIENTSIZE|SWP_NOCLIENTMOVE },
    { 0 }
};
/* DestroyWindow for a child window (WS_VISIBLE, its parent not visible) */
static const struct message WmDestroyInvisibleChildSeq[] = {
    { 0x0090, sent|optional },
    { WM_PARENTNOTIFY, sent|parent|wparam, WM_DESTROY },
    { WM_SHOWWINDOW, sent|wparam, 0 },
    { WM_DESTROY, sent },
    { WM_NCDESTROY, sent },
    { 0 }
};

#define MAX_SEQUENCE 128
static struct recvd_message sequence[MAX_SEQUENCE];
static int sequence_cnt;
static int log_all_parent_messages;

static BOOL ignore_message( UINT message )
{
    /* these are always ignored */
    return (message >= 0xc000 ||
            message == 0x007f /* WM_GETICON */ ||
            message == 0x003d /* WM_GETOBJECT */ ||
            message == WM_TIMECHANGE);
}

static void add_message(const struct recvd_message *msg)
{
    struct recvd_message *seq;

    if (sequence_cnt == MAX_SEQUENCE - 1) return;
    seq = &sequence[sequence_cnt];
    seq->hwnd = msg->hwnd;
    seq->message = msg->message;
    seq->flags = msg->flags;
    seq->wParam = msg->wParam;
    seq->lParam = msg->lParam;

    switch (msg->message)
    {
    case WM_WINDOWPOSCHANGING:
    case WM_WINDOWPOSCHANGED:
        if (msg->lParam)
        {
            WINDOWPOS FAR *winpos = (WINDOWPOS FAR *)msg->lParam;

            seq->wParam = winpos->flags;
            /* We are not interested in the flags that don't match under XP and Win9x */
            seq->wParam &= ~SWP_NOZORDER;
        }
        break;
    }
    sequence_cnt++;
}

static void flush_sequence(void)
{
    sequence_cnt = 0;
}

/* dispatch what is queued, for a while */
static void flush_events(void)
{
    MSG msg;
    DWORD time = GetTickCount() + 100;

    while ((long)(time - GetTickCount()) > 0)
        while (PeekMessage( &msg, 0, 0, 0, PM_REMOVE )) DispatchMessage( &msg );
}

static void dump_sequence(const char *context, const char *file, int line)
{
    int i;

    trace_(file, line)("Failed sequence %s, the messages were:\n", context );
    for (i = 0; i < sequence_cnt; i++)
        trace_(file, line)("  %d: %04x hwnd %04x wp %04x lp %08lx flags %x\n", i,
                           sequence[i].message, sequence[i].hwnd, sequence[i].wParam,
                           sequence[i].lParam, sequence[i].flags);
}

#define ok_sequence( exp, contx, todo) \
        ok_sequence_( (exp), (contx), (todo), __FILE__, __LINE__)

static void ok_sequence_(const struct message *expected, const char *context, int todo,
                         const char *file, int line)
{
    static const struct recvd_message end_of_sequence;
    const struct recvd_message *actual;
    int dump = 0;
    unsigned int count = 0;

    add_message(&end_of_sequence);

    actual = sequence;

    while (expected->message && actual->message)
    {
	if (expected->message == actual->message)
	{
	    if (expected->flags & wparam)
	    {
                ok_( file, line)( ((expected->wParam ^ actual->wParam) & ~expected->wp_mask) == 0,
                                 "%s: %u: in msg 0x%04x expecting wParam 0x%x got 0x%x\n",
                                 (LPSTR)context, count, expected->message, expected->wParam, actual->wParam);
                if ((expected->wParam ^ actual->wParam) & ~expected->wp_mask) dump++;
	    }
	    if (expected->flags & lparam)
            {
                ok_( file, line)(((expected->lParam ^ actual->lParam) & ~expected->lp_mask) == 0,
                                 "%s: %u: in msg 0x%04x expecting lParam 0x%lx got 0x%lx\n",
                                 (LPSTR)context, count, expected->message, expected->lParam, actual->lParam);
                if ((expected->lParam ^ actual->lParam) & ~expected->lp_mask) dump++;
            }
	    if ((expected->flags & optional) &&
                ((expected->flags ^ actual->flags) & (defwinproc|parent)))
            {
                /* don't match optional messages if their defwinproc or parent status differs */
                expected++;
                count++;
                continue;
            }
	    ok_( file, line) ((expected->flags & defwinproc) == (actual->flags & defwinproc),
		"%s: %u: the msg 0x%04x should %shave been sent by DefWindowProc\n",
                (LPSTR)context, count, expected->message, (LPSTR)((expected->flags & defwinproc) ? "" : "NOT "));
            if ((expected->flags & defwinproc) != (actual->flags & defwinproc)) dump++;

	    ok_( file, line) ((expected->flags & beginpaint) == (actual->flags & beginpaint),
		"%s: %u: the msg 0x%04x should %shave been sent by BeginPaint\n",
                (LPSTR)context, count, expected->message, (LPSTR)((expected->flags & beginpaint) ? "" : "NOT "));
            if ((expected->flags & beginpaint) != (actual->flags & beginpaint)) dump++;

	    ok_( file, line) ((expected->flags & (sent|posted)) == (actual->flags & (sent|posted)),
		"%s: %u: the msg 0x%04x should have been %s\n",
                (LPSTR)context, count, expected->message, (LPSTR)((expected->flags & posted) ? "posted" : "sent"));
            if ((expected->flags & (sent|posted)) != (actual->flags & (sent|posted))) dump++;

	    ok_( file, line) ((expected->flags & parent) == (actual->flags & parent),
		"%s: %u: the msg 0x%04x was expected in %s\n",
                (LPSTR)context, count, expected->message, (LPSTR)((expected->flags & parent) ? "parent" : "child"));
            if ((expected->flags & parent) != (actual->flags & parent)) dump++;

	    expected++;
	    actual++;
	}
	else if (expected->flags & optional)
	    expected++;
        else
        {
            ok_( file, line) (FALSE, "%s: %u: the msg 0x%04x was expected, but got msg 0x%04x instead\n",
                              (LPSTR)context, count, expected->message, actual->message);
            dump++;
            expected++;
            actual++;
        }
        count++;
    }

    /* skip all optional trailing messages */
    while (expected->message && (expected->flags & optional))
	expected++;

    if (expected->message || actual->message)
    {
        ok_( file, line) (FALSE, "%s: %u: the msg sequence is not complete: expected %04x - actual %04x\n",
                          (LPSTR)context, count, expected->message, actual->message);
        dump++;
    }

    if (dump) dump_sequence(context, file, line);
    flush_sequence();
}

static LRESULT MsgCheckProc(HWND hwnd, UINT message, WPARAM wParam, LPARAM lParam)
{
    static int defwndproc_counter = 0;
    static int beginpaint_counter = 0;
    LRESULT ret;
    struct recvd_message msg;

    if (ignore_message( message )) return 0;

    switch (message)
    {
	case WM_ENABLE:
	{
	    LONG style = GetWindowLong(hwnd, GWL_STYLE);
	    ok((BOOL)wParam == !(style & WS_DISABLED),
		"wrong WS_DISABLED state: %d != %d\n", wParam, !(style & WS_DISABLED));
	    break;
	}

	case WM_NCDESTROY:
	    ok(!GetWindow(hwnd, GW_CHILD), "children should be unlinked at this point\n");
	    break;

	/* test_accelerators() depends on this */
	case WM_NCHITTEST:
	    return HTCLIENT;

	/* ignore */
	case WM_MOUSEMOVE:
	case WM_MOUSEACTIVATE:
	case WM_NCMOUSEMOVE:
	case WM_SETCURSOR:
	    return 0;
    }

    msg.hwnd = hwnd;
    msg.message = message;
    msg.flags = sent|wparam|lparam;
    if (defwndproc_counter) msg.flags |= defwinproc;
    if (beginpaint_counter) msg.flags |= beginpaint;
    msg.wParam = wParam;
    msg.lParam = lParam;
    add_message(&msg);

    if (message == WM_PAINT)
    {
        PAINTSTRUCT ps;
        beginpaint_counter++;
        BeginPaint( hwnd, &ps );
        beginpaint_counter--;
        EndPaint( hwnd, &ps );
        return 0;
    }

    defwndproc_counter++;
    ret = DefWindowProc(hwnd, message, wParam, lParam);
    defwndproc_counter--;

    return ret;
}

LRESULT CALLBACK __export MsgCheckProcA(HWND hwnd, UINT message, WPARAM wParam, LPARAM lParam)
{
    return MsgCheckProc(hwnd, message, wParam, lParam);
}

LRESULT CALLBACK __export ParentMsgCheckProcA(HWND hwnd, UINT message, WPARAM wParam, LPARAM lParam)
{
    static int defwndproc_counter = 0;
    static int beginpaint_counter = 0;
    LRESULT ret;
    struct recvd_message msg;

    if (ignore_message( message )) return 0;

    if (log_all_parent_messages ||
        message == WM_PARENTNOTIFY || message == WM_CANCELMODE ||
	message == WM_SETFOCUS || message == WM_KILLFOCUS ||
	message == WM_ENABLE ||	message == WM_ENTERIDLE ||
	message == WM_DRAWITEM || message == WM_COMMAND)
    {
        switch (message)
        {
            /* ignore */
            case WM_NCHITTEST:
                return HTCLIENT;
            case WM_SETCURSOR:
            case WM_MOUSEMOVE:
            case WM_NCMOUSEMOVE:
                return 0;
        }

        msg.hwnd = hwnd;
        msg.message = message;
        msg.flags = sent|parent|wparam|lparam;
        if (defwndproc_counter) msg.flags |= defwinproc;
        if (beginpaint_counter) msg.flags |= beginpaint;
        msg.wParam = wParam;
        msg.lParam = lParam;
        add_message(&msg);
    }

    if (message == WM_PAINT)
    {
        PAINTSTRUCT ps;
        beginpaint_counter++;
        BeginPaint( hwnd, &ps );
        beginpaint_counter--;
        EndPaint( hwnd, &ps );
        return 0;
    }

    defwndproc_counter++;
    ret = DefWindowProc(hwnd, message, wParam, lParam);
    defwndproc_counter--;

    return ret;
}

static BOOL RegisterWindowClasses(void)
{
    WNDCLASS cls;

    cls.style = 0;
    cls.lpfnWndProc = MsgCheckProcA;
    cls.cbClsExtra = 0;
    cls.cbWndExtra = 0;
    cls.hInstance = main_instance;
    cls.hIcon = LoadIcon(0, IDI_APPLICATION);
    cls.hCursor = LoadCursor(0, IDC_ARROW);
    cls.hbrBackground = GetStockObject(WHITE_BRUSH);
    cls.lpszMenuName = NULL;
    cls.lpszClassName = "TestWindowClass";
    if(!RegisterClass(&cls)) return FALSE;

    cls.lpfnWndProc = ParentMsgCheckProcA;
    cls.lpszClassName = "TestParentClass";
    if(!RegisterClass(&cls)) return FALSE;

    cls.lpfnWndProc = (WNDPROC)DefWindowProc;
    cls.lpszClassName = "SimpleWindowClass";
    if(!RegisterClass(&cls)) return FALSE;

    return TRUE;
}

static void test_messages(void)
{
    HWND hwnd, hparent, hchild;

    flush_sequence();

    hwnd = CreateWindowEx(0, "TestWindowClass", "Test overlapped", WS_OVERLAPPEDWINDOW,
                           100, 100, 200, 200, 0, 0, main_instance, NULL);
    ok (hwnd != 0, "Failed to create overlapped window\n");
    ok_sequence(WmCreateOverlappedSeq, "CreateWindow:overlapped", FALSE);

    /* test ShowWindow(SW_HIDE) on a newly created invisible window */
    ok( ShowWindow(hwnd, SW_HIDE) == FALSE, "ShowWindow: window was visible\n" );
    ok_sequence(WmEmptySeq, "ShowWindow(SW_HIDE):overlapped, invisible", FALSE);

    DestroyWindow(hwnd);
    flush_sequence();

    hparent = CreateWindowEx(0, "TestParentClass", "Test parent", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                              100, 100, 200, 200, 0, 0, main_instance, NULL);
    ok (hparent != 0, "Failed to create parent window\n");
    flush_events();
    flush_sequence();

    /* visible child window */
    hchild = CreateWindowEx(0, "TestWindowClass", "Test child",
                             WS_CHILD | WS_VISIBLE,
                             0, 0, 10, 10, hparent, 0, main_instance, NULL);
    ok (hchild != 0, "Failed to create child window\n");
    ok_sequence(WmCreateVisibleChildSeq, "CreateWindow:visible child", FALSE);
    DestroyWindow(hchild);
    flush_sequence();

    hchild = CreateWindowEx(0, "TestWindowClass", "Test child", WS_CHILD,
                             0, 0, 10, 10, hparent, 0, main_instance, NULL);
    ok (hchild != 0, "Failed to create child window\n");
    ok_sequence(WmCreateChildSeq, "CreateWindow:child", FALSE);

    ShowWindow(hchild, SW_SHOW);
    ok_sequence(WmShowChildSeq, "ShowWindow(SW_SHOW):child", FALSE);

    ShowWindow(hchild, SW_HIDE);
    ok_sequence(WmHideChildSeq, "ShowWindow(SW_HIDE):child", FALSE);

    ShowWindow(hchild, SW_SHOW);
    ok_sequence(WmShowChildSeq, "ShowWindow(SW_SHOW):child", FALSE);

    DestroyWindow(hchild);
    flush_sequence();
    DestroyWindow(hparent);
    flush_sequence();

    /* a child of a parent that isn't visible */
    hparent = CreateWindowEx(0, "TestParentClass", "Test parent", WS_OVERLAPPEDWINDOW,
                              100, 100, 200, 200, 0, 0, main_instance, NULL);
    ok (hparent != 0, "Failed to create parent window\n");
    hchild = CreateWindowEx(0, "TestWindowClass", "Test child", WS_CHILD,
                             0, 0, 10, 10, hparent, 0, main_instance, NULL);
    ok (hchild != 0, "Failed to create child window\n");
    SetWindowPos(hchild, 0,0,0,0,0, SWP_SHOWWINDOW|SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE|SWP_NOZORDER);
    ok((GetWindowLong(hchild, GWL_STYLE) & WS_VISIBLE) != 0, "WS_VISIBLE should be set\n");
    ok(!IsWindowVisible(hchild), "IsWindowVisible() should return FALSE\n");
    flush_sequence();
    DestroyWindow(hchild);
    ok_sequence(WmDestroyInvisibleChildSeq, "DestroyInvisibleChildSeq", FALSE);

    DestroyWindow(hparent);
    flush_sequence();
}

/* SendMessage, PostMessage and the queue's order (added for Win16) */
static void test_queue(void)
{
    HWND hwnd, hwnd2;
    MSG msg;
    LRESULT res;
    BOOL ret;

    hwnd = CreateWindowEx(0, "TestWindowClass", "Test popup", WS_POPUP,
                           0, 0, 50, 50, 0, 0, main_instance, NULL);
    ok(hwnd != 0, "Failed to create window\n");
    hwnd2 = CreateWindowEx(0, "TestWindowClass", "Test popup 2", WS_POPUP,
                           0, 0, 50, 50, 0, 0, main_instance, NULL);
    ok(hwnd2 != 0, "Failed to create window\n");
    flush_events();
    flush_sequence();

    /* a sent message goes to the window procedure at once */
    res = SendMessage(hwnd, WM_USER, 0x1234, 0x56789abcL);
    ok(res == 0, "SendMessage returned %ld\n", res);
    ok(sequence_cnt == 1 && sequence[0].message == WM_USER && sequence[0].hwnd == hwnd &&
       sequence[0].wParam == 0x1234 && sequence[0].lParam == 0x56789abcL,
       "SendMessage didn't call the window procedure: %d messages, the first %04x\n",
       sequence_cnt, sequence[0].message);
    ok(!PeekMessage(&msg, 0, 0, 0, PM_NOREMOVE), "a sent message was queued\n");
    flush_sequence();

    /* posted ones are queued in their order */
    ok(PostMessage(hwnd, WM_USER+1, 1, 11), "PostMessage failed\n");
    ok(PostMessage(hwnd2, WM_USER+2, 2, 22), "PostMessage failed\n");
    ok(PostAppMessage(GetCurrentTask(), WM_USER+3, 3, 33), "PostAppMessage failed\n");
    ok(PostMessage(hwnd, WM_USER+4, 4, 44), "PostMessage failed\n");
    ok(sequence_cnt == 0, "PostMessage called the window procedure\n");

    memset(&msg, 0, sizeof(msg));
    ret = PeekMessage(&msg, 0, 0, 0, PM_NOREMOVE);
    ok(ret && msg.message == WM_USER+1 && msg.hwnd == hwnd && msg.wParam == 1 && msg.lParam == 11,
       "PM_NOREMOVE got %d: %04x hwnd %04x wp %x lp %lx\n", ret, msg.message, msg.hwnd, msg.wParam, msg.lParam);
    ret = PeekMessage(&msg, 0, 0, 0, PM_NOREMOVE);
    ok(ret && msg.message == WM_USER+1, "PM_NOREMOVE didn't leave the message, got %04x\n", msg.message);

    /* a filter by window */
    ret = PeekMessage(&msg, hwnd2, 0, 0, PM_REMOVE);
    ok(ret && msg.message == WM_USER+2 && msg.hwnd == hwnd2 && msg.wParam == 2 && msg.lParam == 22,
       "filter by window got %d: %04x hwnd %04x\n", ret, msg.message, msg.hwnd);
    ret = PeekMessage(&msg, hwnd2, 0, 0, PM_REMOVE);
    ok(!ret, "second message for hwnd2: %04x\n", msg.message);

    /* a filter by message */
    ret = PeekMessage(&msg, 0, WM_USER+4, WM_USER+4, PM_REMOVE);
    ok(ret && msg.message == WM_USER+4 && msg.hwnd == hwnd && msg.wParam == 4,
       "filter by message got %d: %04x\n", ret, msg.message);

    /* what is left, in order */
    ret = GetMessage(&msg, 0, 0, 0);
    ok(ret && msg.message == WM_USER+1 && msg.hwnd == hwnd, "GetMessage got %d: %04x\n", ret, msg.message);
    ok(sequence_cnt == 0, "GetMessage called the window procedure\n");
    res = DispatchMessage(&msg);
    ok(sequence_cnt == 1 && sequence[0].message == WM_USER+1 && sequence[0].wParam == 1 &&
       sequence[0].lParam == 11, "DispatchMessage didn't call the window procedure\n");
    flush_sequence();
    ret = GetMessage(&msg, 0, 0, 0);
    ok(ret && msg.message == WM_USER+3 && msg.hwnd == 0 && msg.wParam == 3 && msg.lParam == 33,
       "GetMessage got %d: %04x hwnd %04x wp %x lp %lx\n", ret, msg.message, msg.hwnd, msg.wParam, msg.lParam);
    ok(!PeekMessage(&msg, 0, WM_USER, WM_USER+100, PM_REMOVE), "a message is left: %04x\n", msg.message);

    /* a message posted while one is being handled */
    PostMessage(hwnd, WM_USER+5, 0, 0);
    res = SendMessage(hwnd, WM_USER+6, 0, 0);
    ok(sequence_cnt == 1 && sequence[0].message == WM_USER+6, "SendMessage handled %d messages\n", sequence_cnt);
    flush_sequence();
    ret = PeekMessage(&msg, 0, 0, 0, PM_REMOVE);
    ok(ret && msg.message == WM_USER+5, "PeekMessage got %04x\n", msg.message);

    DestroyWindow(hwnd2);
    DestroyWindow(hwnd);
    flush_events();
    flush_sequence();
}

static void test_quit_message(void)
{
    MSG msg;
    BOOL ret;

    /* test using PostQuitMessage */
    flush_events();
    PostQuitMessage(0xbeef);

    ret = PeekMessage(&msg, NULL, 0, 0, PM_NOREMOVE);
    ok(ret, "PeekMessage failed\n");
    ok(msg.message == WM_QUIT, "Received message 0x%04x instead of WM_QUIT\n", msg.message);
    ok(msg.wParam == 0xbeef, "wParam was 0x%x instead of 0xbeef\n", msg.wParam);

    ret = PostAppMessage(GetCurrentTask(), WM_USER, 0, 0);
    ok(ret, "PostAppMessage failed\n");

    ret = GetMessage(&msg, NULL, 0, 0);
    ok(ret > 0, "GetMessage failed\n");
    ok(msg.message == WM_USER, "Received message 0x%04x instead of WM_USER\n", msg.message);

    /* note: WM_QUIT message received after WM_USER message */
    ret = GetMessage(&msg, NULL, 0, 0);
    ok(!ret, "GetMessage return %d instead of FALSE\n", ret);
    ok(msg.message == WM_QUIT, "Received message 0x%04x instead of WM_QUIT\n", msg.message);
    ok(msg.wParam == 0xbeef, "wParam was 0x%x instead of 0xbeef\n", msg.wParam);

    ret = PeekMessage(&msg, NULL, 0, 0, PM_REMOVE);
    ok( !ret || msg.message != WM_QUIT, "Received WM_QUIT again\n" );

    /* now test with PostAppMessage - different behaviour! */
    PostAppMessage(GetCurrentTask(), WM_QUIT, 0xdead, 0);

    ret = PeekMessage(&msg, NULL, 0, 0, PM_NOREMOVE);
    ok(ret, "PeekMessage failed\n");
    ok(msg.message == WM_QUIT, "Received message 0x%04x instead of WM_QUIT\n", msg.message);
    ok(msg.wParam == 0xdead, "wParam was 0x%x instead of 0xdead\n", msg.wParam);

    ret = PostAppMessage(GetCurrentTask(), WM_USER, 0, 0);
    ok(ret, "PostAppMessage failed\n");

    /* note: we receive the WM_QUIT message first this time */
    ret = GetMessage(&msg, NULL, 0, 0);
    ok(!ret, "GetMessage return %d instead of FALSE\n", ret);
    ok(msg.message == WM_QUIT, "Received message 0x%04x instead of WM_QUIT\n", msg.message);
    ok(msg.wParam == 0xdead, "wParam was 0x%x instead of 0xdead\n", msg.wParam);

    ret = GetMessage(&msg, NULL, 0, 0);
    ok(ret > 0, "GetMessage failed\n");
    ok(msg.message == WM_USER, "Received message 0x%04x instead of WM_USER\n", msg.message);
}

#define TIMER_ID  0x19

static int count = 0;
static HWND timer_hwnd;
static UINT timer_msg, timer_id;

void CALLBACK __export callback_count(HWND hwnd, UINT uMsg, UINT idEvent, DWORD dwTime)
{
    count++;
    timer_hwnd = hwnd;
    timer_msg = uMsg;
    timer_id = idEvent;
}

/* Sleep() for Windows 3.1: the messages that come meanwhile wait */
static void wait_ticks(DWORD ms)
{
    DWORD end = GetTickCount() + ms;
    while ((long)(end - GetTickCount()) > 0)
        Yield();
}

static void dispatch_all(void)
{
    MSG msg;
    while (PeekMessage(&msg, NULL, 0, 0, PM_REMOVE)) DispatchMessage(&msg);
}

static void test_timers(void)
{
    HWND hwnd;
    UINT id;
    MSG msg;
    BOOL got;
    DWORD start;
    TIMERPROC proc = (TIMERPROC)MakeProcInstance((FARPROC)callback_count, main_instance);

    hwnd = CreateWindow ("TestWindowClass", NULL,
       WS_OVERLAPPEDWINDOW ,
       CW_USEDEFAULT, CW_USEDEFAULT, 300, 300, 0,
       NULL, main_instance, 0);
    ok(hwnd != 0, "CreateWindow failed\n");

    id = SetTimer(hwnd,TIMER_ID,10000,proc);
    ok(id, "SetTimer failed\n");
    ok(id==TIMER_ID, "SetTimer timer ID different\n");
    ok( KillTimer(hwnd, TIMER_ID), "KillTimer failed\n");
    ok( !KillTimer(hwnd, TIMER_ID), "KillTimer succeeded a second time\n");

    /* a window's timer, by WM_TIMER in the queue (added for Win16) */
    flush_events();
    flush_sequence();
    id = SetTimer(hwnd, TIMER_ID, 50, NULL);
    ok(id == TIMER_ID, "SetTimer returned %x\n", id);
    got = FALSE;
    start = GetTickCount();
    while (!got && GetTickCount() - start < 2000)
        got = PeekMessage(&msg, hwnd, WM_TIMER, WM_TIMER, PM_REMOVE);
    ok(got, "no WM_TIMER in 2 seconds\n");
    if (got)
    {
        ok(msg.hwnd == hwnd && msg.wParam == TIMER_ID && msg.lParam == 0,
           "WM_TIMER for %04x id %x lParam %08lx\n", msg.hwnd, msg.wParam, msg.lParam);
        DispatchMessage(&msg);
        ok(sequence_cnt == 1 && sequence[0].message == WM_TIMER && sequence[0].wParam == TIMER_ID,
           "DispatchMessage of WM_TIMER: %d messages, the first %04x\n", sequence_cnt, sequence[0].message);
    }
    ok(KillTimer(hwnd, TIMER_ID), "KillTimer failed\n");
    /* a WM_TIMER already queued may still come; none after that */
    dispatch_all();
    wait_ticks(200);
    ok(!PeekMessage(&msg, hwnd, WM_TIMER, WM_TIMER, PM_REMOVE), "WM_TIMER after KillTimer\n");
    flush_sequence();

    /* a window's timer with a procedure */
    count = 0;
    id = SetTimer(hwnd, TIMER_ID + 1, 50, proc);
    ok(id == TIMER_ID + 1, "SetTimer returned %x\n", id);
    start = GetTickCount();
    while (!count && GetTickCount() - start < 2000)
        dispatch_all();
    ok(count > 0, "the timer procedure wasn't called in 2 seconds\n");
    ok(timer_hwnd == hwnd && timer_msg == WM_TIMER && timer_id == TIMER_ID + 1,
       "the timer procedure got %04x %04x %x\n", timer_hwnd, timer_msg, timer_id);
    ok(KillTimer(hwnd, TIMER_ID + 1), "KillTimer failed\n");
    ok(sequence_cnt == 0 || sequence[0].message != WM_TIMER,
       "WM_TIMER with a procedure went to the window procedure\n");

    ok(DestroyWindow(hwnd), "failed to destroy window\n");
    FreeProcInstance((FARPROC)proc);
}

static void test_timers_no_wnd(void)
{
    UINT id;
    TIMERPROC proc = (TIMERPROC)MakeProcInstance((FARPROC)callback_count, main_instance);

    count = 0;
    id = SetTimer(NULL, 0, 100, proc);
    ok(id != 0, "did not get id from SetTimer.\n");
    dispatch_all();
    ok(count == 0, "did not get zero count as expected (%i).\n", count);
    wait_ticks(300);
    dispatch_all();
    ok(count == 1, "did not get one count as expected (%i).\n", count);
    ok(timer_hwnd == 0 && timer_msg == WM_TIMER && timer_id == id,
       "the timer procedure got %04x %04x %x, not 0 WM_TIMER %x\n", timer_hwnd, timer_msg, timer_id, id);
    KillTimer(NULL, id);
    wait_ticks(250);
    dispatch_all();
    ok(count == 1, "killing timer did not work (%i).\n", count);
    FreeProcInstance((FARPROC)proc);
}

START_TEST(msg)
{
    /* A program's instance handle is its data segment's selector. */
    main_instance = (HINSTANCE)get_ds();

    if (!RegisterWindowClasses())
    {
        ok(FALSE, "RegisterWindowClasses failed\n");
        return;
    }

    test_messages();
    test_queue();
    test_timers();
    test_timers_no_wnd();
    test_quit_message();
}
