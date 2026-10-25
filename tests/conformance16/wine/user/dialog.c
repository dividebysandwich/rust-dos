/* Unit test suite for the dialog functions.
 *
 * Copyright 2004 Bill Medland
 *
 * Ported to Win16 from Wine's dlls/user32/tests/dialog.c. Windows 3.1
 * has no WS_EX_CONTROLPARENT or DS_CONTROL, so GetNextDlgTabItem and
 * GetNextDlgGroupItem are checked in a flat dialog only, and the checks
 * that descend into nested dialogs are left out, as are the ones that
 * pass a null control, which Windows 3.1's parameter validation turns
 * away. The test program has no resources: the dialog templates Wine
 * keeps in resource.rc are built in memory in the Windows 3.1 format
 * and passed to DialogBoxIndirect and CreateDialogIndirect. The
 * message box font test (CreateDialogIndirectParamW) and the
 * SetLastError checks are left out. Checks of SetDlgItemInt,
 * GetDlgItemInt, CheckDlgButton and CheckRadioButton are added.
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

#define MAXHWNDS 100
static HWND hwnd [MAXHWNDS];
static unsigned int numwnds=1; /* 0 is reserved for null */

/* Global handles */
static HINSTANCE g_hinst;                          /* This application's HINSTANCE */
static HWND g_hwndMain, g_hwndButton1, g_hwndButton2, g_hwndButtonCancel;
static HWND g_hwndTestDlg, g_hwndTestDlgBut1, g_hwndTestDlgBut2, g_hwndTestDlgEdit;
static HWND g_hwndInitialFocusT1, g_hwndInitialFocusT2, g_hwndInitialFocusGroupBox;

static LONG g_styleInitialFocusT1, g_styleInitialFocusT2;
static BOOL g_bInitialFocusInitDlgResult;

static int g_terminated;

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

/***********************************************************************
 *
 * Dialog templates in the Windows 3.1 format: a DWORD style, a BYTE
 * count of items, the four coordinates, the menu, class and caption
 * strings and, with DS_SETFONT, a WORD point size and a face name. Each
 * item has its coordinates, a WORD id, a DWORD style, a class byte (0x80
 * button, 0x81 edit, 0x82 static), its text and a BYTE count of extra
 * data.
 */

#define CLS_BUTTON 0x80
#define CLS_EDIT   0x81
#define CLS_STATIC 0x82

static BYTE tmpl[512];
static int tlen;

static void put_byte(BYTE b)
{
    tmpl[tlen++] = b;
}

static void put_word(WORD w)
{
    put_byte(LOBYTE(w));
    put_byte(HIBYTE(w));
}

static void put_dword(DWORD d)
{
    put_word(LOWORD(d));
    put_word(HIWORD(d));
}

static void put_str(const char *s)
{
    do put_byte(*s); while (*s++);
}

static void begin_dialog(DWORD style, int count, int x, int y, int cx, int cy,
                         const char *caption)
{
    tlen = 0;
    put_dword(style);
    put_byte(count);
    put_word(x);
    put_word(y);
    put_word(cx);
    put_word(cy);
    put_str("");                    /* no menu */
    put_str("");                    /* the dialog class */
    put_str(caption);
    if (style & DS_SETFONT)
    {
        put_word(8);
        put_str("MS Sans Serif");
    }
}

static void add_item(int x, int y, int cx, int cy, int id, DWORD style,
                     BYTE cls, const char *text)
{
    put_word(x);
    put_word(y);
    put_word(cx);
    put_word(cy);
    put_word(id);
    put_dword(style);
    put_byte(cls);
    put_str(text);
    put_byte(0);
}

/* DialogBoxIndirect takes the template in a global memory object. */
static HGLOBAL template_handle(void)
{
    HGLOBAL h = GlobalAlloc(GMEM_MOVEABLE, tlen);
    BYTE FAR *p = (BYTE FAR *)GlobalLock(h);

    _fmemcpy(p, tmpl, tlen);
    GlobalUnlock(h);
    return h;
}

/* IDD_DIALOG and TEST_EMPTY_DIALOG */
static HGLOBAL idd_dialog(BOOL empty)
{
    begin_dialog(DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_SETFONT,
                 empty ? 0 : 2, 0, 0, 186, 95, "Dialog");
    if (!empty)
    {
        add_item(129, 7, 50, 14, IDOK, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                 CLS_BUTTON, "OK");
        add_item(129, 24, 50, 14, IDCANCEL, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                 CLS_BUTTON, "Cancel");
    }
    return template_handle();
}

/* RADIO_TEST_DIALOG */
static HGLOBAL radio_test_dialog(void)
{
    begin_dialog(DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_SETFONT,
                 4, 0, 0, 160, 80, "Radio Button Test Dialog");
    add_item(6, 5, 92, 70, 100, WS_CHILD | WS_VISIBLE | BS_GROUPBOX, CLS_BUTTON, "Static");
    add_item(17, 27, 39, 10, 200,
             WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON | WS_GROUP | WS_TABSTOP,
             CLS_BUTTON, "Radio1");
    add_item(17, 40, 39, 10, 201, WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON,
             CLS_BUTTON, "Radio2");
    add_item(109, 20, 50, 14, IDCANCEL,
             WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON | WS_TABSTOP | WS_GROUP,
             CLS_BUTTON, "Cancel");
    return template_handle();
}

/* MULTI_EDIT_DIALOG, without DS_CENTER, which came with Windows 95 */
static const void FAR *multi_edit_dialog(void)
{
    begin_dialog(WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | DS_SETFONT,
                 3, 0, 0, 160, 75, "Multiple Edit Test");
    add_item(5, 5, 150, 14, 1000,
             WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_MULTILINE | ES_WANTRETURN,
             CLS_EDIT, "");
    add_item(5, 25, 150, 14, 1001, WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
             CLS_EDIT, "");
    add_item(5, 45, 150, 14, 1002, WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
             CLS_EDIT, "");
    return tmpl;
}

/* Run a modal dialog from an in-memory template and free the template. */
static int dialog_box(HGLOBAL h, DLGPROC proc, LPARAM param)
{
    int ret = DialogBoxIndirectParam(g_hinst, h, 0, proc, param);
    GlobalFree(h);
    return ret;
}

/***********************************************************************
 *
 * The flat part of Wine's window hierarchy for the GetNextDlgItem
 * tests: dialog 6 and its controls.
 */

typedef struct {
    int id;
    int parent;
    DWORD style;
} h_entry;

static const h_entry hierarchy [] = {
    /* 0 is reserved for the null window */
    /* The main dialog with lots of controls */
    {  6,  0,  WS_OVERLAPPED},
        /* At the start of a dialog */
        /* Disabled controls are skipped */
    { 63,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_TABSTOP},
        /* Invisible controls are skipped */
    { 64,  6,  WS_CHILD | WS_TABSTOP},
        /* Invisible disabled controls are skipped */
    { 65,  6,  WS_CHILD | WS_DISABLED | WS_TABSTOP},
        /* Non-tabstop controls are skipped for tabs but not for groups */
    { 66,  6,  WS_CHILD | WS_VISIBLE},
        /* End of first group, with no tabstops in it */
    { 23,  6,  WS_CHILD | WS_VISIBLE | WS_GROUP},
        /* At last a tabstop */
    { 67,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    /* A group that is totally disabled or invisible */
    { 24,  6,  WS_CHILD | WS_DISABLED | WS_GROUP},
    { 68,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_TABSTOP},
    { 69,  6,  WS_CHILD | WS_TABSTOP},
    /* A valid group in the middle of the dialog (not the first nor last group*/
    { 25,  6,  WS_CHILD | WS_VISIBLE | WS_GROUP},
        /* A non-tabstop item will be skipped for tabs */
    { 70,  6,  WS_CHILD | WS_VISIBLE},
        /* A disabled item will be skipped for tabs and groups */
    { 71,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_TABSTOP},
        /* A valid item will be found for tabs and groups */
    { 72,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
        /* A disabled item to skip when looking for the next group item */
    { 73,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_TABSTOP},
    /* The next group begins with an enabled visible label */
    { 26,  6,  WS_CHILD | WS_VISIBLE | WS_GROUP},
    { 74,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    { 75,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    /* That group is terminated by a disabled label */
    { 27,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_GROUP},
    { 76,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    { 77,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    /* That group is terminated by an invisible label */
    { 28,  6,  WS_CHILD | WS_GROUP},
    /* The end of the dialog with item for loop and recursion testing */
    { 78,  6,  WS_CHILD | WS_VISIBLE | WS_TABSTOP},
    /* No tabstop so skipped for prev tab, but found for prev group */
    { 79,  6,  WS_CHILD | WS_VISIBLE},
    { 80,  6,  WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_TABSTOP},
    {0, 0, 0}
};

static BOOL CreateWindows (HINSTANCE hinst)
{
    const h_entry *p = hierarchy;

    while (p->id != 0)
    {
        DWORD style;
        char ctrlname[9];

        /* Basically assert that the hierarchy is valid and track the
         * maximum control number
         */
        if (p->id >= numwnds)
        {
            if (p->id >=  sizeof(hwnd)/sizeof(hwnd[0]))
            {
                trace ("Control %d is out of range\n", p->id);
                return FALSE;
            }
            else
                numwnds = p->id+1;
        }
        if (p->id <= 0)
        {
            trace ("Control %d is out of range\n", p->id);
            return FALSE;
        }
        if (hwnd[p->id] != 0)
        {
            trace ("Control %d is used more than once\n", p->id);
            return FALSE;
        }

        /* Create the control */
        wsprintf (ctrlname, "ctrl%04d", p->id);
        hwnd[p->id] = CreateWindow (p->parent ? "static" : "GetNextDlgItemWindowClass", ctrlname, p->style, 10, 10, 10, 10, hwnd[p->parent], p->parent ? (HMENU) (2000 + p->id) : 0, hinst, 0);
        if (!hwnd[p->id])
        {
            trace ("Failed to create control %d\n", p->id);
            return FALSE;
        }

        /* Check that the styles are as we specified (except the main one
         * which is quite frequently messed up).  If this keeps breaking then
         * we could mask out the bits that don't concern us.
         */
        if (p->parent)
        {
            style = GetWindowLong (hwnd[p->id], GWL_STYLE);
            if (style != p->style)
            {
                trace ("Style mismatch at %d: %08lx cf %08lx\n", p->id, style, p->style);
            }
        }
        p++;
    }

    return TRUE;
}

/* Form the lParam of a WM_KEYDOWN message */
static DWORD KeyDownData (int repeat, int scancode, int extended, int wasdown)
{
    return ((repeat & 0x0000FFFF) | ((DWORD)(scancode & 0x00FF) >> 16) |
            (extended ? 0x01000000L : 0) | (wasdown ? 0x40000000L : 0));
}

/* Form a WM_KEYDOWN VK_TAB message to the specified window */
static void FormTabMsg (MSG *pMsg, HWND hwnd)
{
    pMsg->hwnd = hwnd;
    pMsg->message = WM_KEYDOWN;
    pMsg->wParam = VK_TAB;
    pMsg->lParam = KeyDownData (1, 0x0F, 0, 0);
    /* pMsg->time is not set.  It shouldn't be needed */
    /* pMsg->pt is ignored */
}

/* Form a WM_KEYDOWN VK_RETURN message to the specified window */
static void FormEnterMsg (MSG *pMsg, HWND hwnd)
{
    pMsg->hwnd = hwnd;
    pMsg->message = WM_KEYDOWN;
    pMsg->wParam = VK_RETURN;
    pMsg->lParam = KeyDownData (1, 0x1C, 0, 0);
    /* pMsg->time is not set.  It shouldn't be needed */
    /* pMsg->pt is ignored */
}

/***********************************************************************
 *
 * The actual tests
 */

typedef struct
{
    int isok; /* or is it todo */
    int test;
    int dlg;
    int ctl;
    int tab;
    int prev;
    int res;
} test_record;

static int id (HWND h)
{
    unsigned int i;
    for (i = 0; i < numwnds; i++)
        if (hwnd[i] == h)
            return i;
    return -1;
}

/* Tests (numbered as in Wine's test, which describes the ones left out)
 *
 *   3. Prev Group of hDlg in hDlg is null
 *   4. Prev Tab of hDlg in hDlg is null
 *   7. Next Group of hDlg in hDlg is the first visible enabled child
 *   8. Next Tab of hDlg in hDlg is the first visible enabled tabstop
 *
 * Tests 15-24 are the basic Prev/Next Group tests
 *
 *  15. Next Group of a visible enabled non-group control is the next visible
 *      enabled non-group control, if there is one before the next group
 *  16. Next Group of a visible enabled non-group control wraps around to the
 *      beginning of the group on finding a control that starts another group.
 *      Note that the group is in the middle of the dialog.
 *  17. As 16 except note that the next group is started with a disabled
 *      visible control.
 *  18. As 16 except note that the next group is started with an invisible
 *      enabled control.
 *  19. Next Group wraps around the controls of the dialog
 *  20. Next Group is the same even if the initial control is disabled.
 *  21. Next Group is the same even if the initial control is invisible.
 *  22. Next Group is the same even if the initial control has the group style
 *  23. Next Group returns the initial control if there is no visible enabled
 *      control in the group. (Initial control disabled and not group style).
 *  24. Prev version of test 16.
 *      Prev Group of a visible enabled non-group control wraps around to the
 *      beginning of the group on finding a control that starts the group.
 *      Note that the group is in the middle of the dialog.
 *
 *  29. Next Tab of a control is the next visible enabled control with the
 *      Tabstop style (N.B. skips disabled, invisible and non-tabstop)
 *  30. Prev Tab of a control is the previous visible enabled control with the
 *      Tabstop style (N.B. skips disabled, invisible and non-tabstop)
 */

static void test_GetNextDlgItem(void)
{
    static test_record test [] =
    {
        /* isok test dlg  ctl  tab  prev res  */

        {   1,   3,    6,   6,   0,   1,   0},
        {   1,   4,    6,   6,   1,   1,   0},
        {   1,   7,    6,   6,   0,   0,  66},
        {   1,   8,    6,   6,   1,   0,  67},

        {   1,  15,    6,  70,   0,   0,  72},
        {   1,  16,    6,  72,   0,   0,  25},
        {   1,  17,    6,  75,   0,   0,  26},
        {   1,  18,    6,  77,   0,   0,  76},
        {   1,  19,    6,  79,   0,   0,  66},
        {   1,  20,    6,  71,   0,   0,  72},
        {   1,  21,    6,  64,   0,   0,  66},

        {   1,  22,    6,  25,   0,   0,  70},
        {   1,  23,    6,  68,   0,   0,  68},
        {   1,  24,    6,  25,   0,   1,  72},

        {   1,  29,    6,  67,   1,   0,  72},
        {   1,  30,    6,  72,   1,   1,  67},

        {   0,   0,    0,   0,   0,   0,   0}  /* End of test */
    };
    const test_record *p = test;

    ok (CreateWindows (g_hinst), "Could not create test windows\n");

    while (p->dlg)
    {
        HWND a;
        a = (p->tab ? GetNextDlgTabItem : GetNextDlgGroupItem) (hwnd[p->dlg], hwnd[p->ctl], p->prev);
        if (p->isok)
        {
            ok (a == hwnd[p->res], "Test %d: %s %s item of %d in %d was %d instead of %d\n", p->test, p->prev ? (LPSTR)"Prev" : (LPSTR)"Next", p->tab ? (LPSTR)"Tab" : (LPSTR)"Group", p->ctl, p->dlg, id(a), p->res);
        }
        else
        {
            todo_wine
            {
                ok (a == hwnd[p->res], "Test %d: %s %s item of %d in %d was actually  %d matching expected %d\n", p->test, p->prev ? (LPSTR)"Prev" : (LPSTR)"Next", p->tab ? (LPSTR)"Tab" : (LPSTR)"Group", p->ctl, p->dlg, id(a), p->res);
            }
        }
        p++;
    }
    DestroyWindow(hwnd[6]);
}

/*
 *  OnMainWindowCreate
 */
static BOOL OnMainWindowCreate (HWND hwnd, LPCREATESTRUCT lpcs)
{
    g_hwndButton1 = CreateWindow ("button", "Button &1",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
            10, 10, 80, 80, hwnd, (HMENU)100, g_hinst, 0);
    if (!g_hwndButton1) return FALSE;

    g_hwndButton2 = CreateWindow ("button", "Button &2",
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
            110, 10, 80, 80, hwnd, (HMENU)200, g_hinst, 0);
    if (!g_hwndButton2) return FALSE;

    g_hwndButtonCancel = CreateWindow ("button", "Cancel",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
            210, 10, 80, 80, hwnd, (HMENU)IDCANCEL, g_hinst, 0);
    if (!g_hwndButtonCancel) return FALSE;

    return TRUE;
}


/*
 *  OnTestDlgCreate
 */

static BOOL OnTestDlgCreate (HWND hwnd, LPCREATESTRUCT lpcs)
{
    g_hwndTestDlgEdit = CreateWindowEx ( WS_EX_NOPARENTNOTIFY,
            "Edit", "Edit",
            WS_CHILDWINDOW | WS_VISIBLE | WS_TABSTOP | ES_LEFT | ES_AUTOHSCROLL,
            16,33,184,24, hwnd, (HMENU)101, g_hinst, 0);
    if (!g_hwndTestDlgEdit) return FALSE;

    g_hwndTestDlgBut1 = CreateWindowEx ( WS_EX_NOPARENTNOTIFY,
            "button", "Button &1",
            WS_CHILDWINDOW | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
            204,33,30,24, hwnd, (HMENU)201, g_hinst, 0);
    if (!g_hwndTestDlgBut1) return FALSE;

    g_hwndTestDlgBut2 = CreateWindowEx ( WS_EX_NOPARENTNOTIFY, "button",
            "Button &2",
            WS_CHILDWINDOW | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
            90,102,80,24, hwnd, (HMENU)IDCANCEL, g_hinst, 0);
    if (!g_hwndTestDlgBut2) return FALSE;

    return TRUE;
}

LRESULT CALLBACK __export main_window_procA (HWND hwnd, UINT uiMsg, WPARAM wParam,
        LPARAM lParam)
{
    switch (uiMsg)
    {
        /* Add blank case statements for these to ensure we don't use them
         * by mistake.
         */
        case DM_GETDEFID: break;
        case DM_SETDEFID: break;

        case WM_CREATE:
            return (OnMainWindowCreate (hwnd,
                    (LPCREATESTRUCT) lParam) ? 0 : (LRESULT) -1);
        case WM_COMMAND:
            if (wParam == IDCANCEL)
            {
                g_terminated = TRUE;
                return 0;
            }
            break;
    }

    return DefWindowProc (hwnd, uiMsg, wParam, lParam);
}

BOOL CALLBACK __export disabled_test_proc (HWND hwnd, UINT uiMsg,
        WPARAM wParam, LPARAM lParam)
{
    switch (uiMsg)
    {
        case WM_INITDIALOG:
        {
            DWORD dw;
            HWND hwndOk;

            dw = SendMessage(hwnd, DM_GETDEFID, 0, 0);
            ok(DC_HASDEFID == HIWORD(dw), "DM_GETDEFID returned %08lx\n", dw);
            hwndOk = GetDlgItem(hwnd, LOWORD(dw));
            ok(hwndOk != 0, "no default button\n");
            EnableWindow(hwndOk, FALSE);

            PostMessage(hwnd, WM_KEYDOWN, VK_RETURN, 0);
            PostMessage(hwnd, WM_COMMAND, IDCANCEL, 0);
            break;
        }
        case WM_COMMAND:
            if (wParam == IDOK)
            {
                g_terminated = TRUE;
                EndDialog(hwnd, 0);
                return 0;
            }
            else if (wParam == IDCANCEL)
            {
                EndDialog(hwnd, 0);
                return 0;
            }
            break;
    }

    return (BOOL)DefWindowProc (hwnd, uiMsg, wParam, lParam);
}

LRESULT CALLBACK __export testDlgWinProc (HWND hwnd, UINT uiMsg, WPARAM wParam,
        LPARAM lParam)
{
    switch (uiMsg)
    {
        /* Add blank case statements for these to ensure we don't use them
         * by mistake.
         */
        case DM_GETDEFID: break;
        case DM_SETDEFID: break;

        case WM_CREATE:
            return (OnTestDlgCreate (hwnd,
                    (LPCREATESTRUCT) lParam) ? 0 : (LRESULT) -1);
    }

    return DefDlgProc (hwnd, uiMsg, wParam, lParam);
}

static BOOL RegisterWindowClasses (void)
{
    WNDCLASS cls;

    cls.style = 0;
    cls.lpfnWndProc = (WNDPROC)DefWindowProc;
    cls.cbClsExtra = 0;
    cls.cbWndExtra = 0;
    cls.hInstance = g_hinst;
    cls.hIcon = NULL;
    cls.hCursor = LoadCursor (NULL, IDC_ARROW);
    cls.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    cls.lpszMenuName = NULL;
    cls.lpszClassName = "GetNextDlgItemWindowClass";

    if (!RegisterClass (&cls)) return FALSE;

    cls.lpfnWndProc = (WNDPROC)main_window_procA;
    cls.lpszClassName = "IsDialogMessageWindowClass";

    if (!RegisterClass (&cls)) return FALSE;

    memset(&cls, 0, sizeof(cls));
    GetClassInfo(0, "#32770", &cls);
    cls.lpfnWndProc = (WNDPROC)testDlgWinProc;
    cls.hInstance = g_hinst;
    cls.lpszClassName = "WM_NEXTDLGCTLWndClass";
    if (!RegisterClass (&cls)) return FALSE;

    return TRUE;
}

static void test_WM_NEXTDLGCTL(void)
{
    DWORD dwVal;

    g_hwndTestDlg = CreateWindowEx( WS_EX_DLGMODALFRAME,
              "WM_NEXTDLGCTLWndClass",
              "WM_NEXTDLGCTL Message test window",
              WS_POPUPWINDOW | WS_CLIPSIBLINGS | WS_DLGFRAME | WS_OVERLAPPED |
              WS_MINIMIZEBOX | WS_MAXIMIZEBOX | DS_SETFONT | DS_MODALFRAME,
              0, 0, 235, 135,
              NULL, NULL, g_hinst, 0);

    ok (g_hwndTestDlg != 0, "failed to create the test window\n");
    ok (g_hwndTestDlgBut1 != 0, "failed to create the first button\n");
    ok (g_hwndTestDlgBut2 != 0, "failed to create the second button\n");
    ok (g_hwndTestDlgEdit != 0, "failed to create the edit control\n");
    if (!g_hwndTestDlg || !g_hwndTestDlgBut1 || !g_hwndTestDlgBut2 || !g_hwndTestDlgEdit)
        return;

    /*
     * Test message DM_SETDEFID
     */

    DefDlgProc( g_hwndTestDlg, DM_SETDEFID, IDCANCEL, 0 );
    DefDlgProc( g_hwndTestDlgBut1, BM_SETSTYLE, BS_DEFPUSHBUTTON, FALSE );
    dwVal = DefDlgProc(g_hwndTestDlg, DM_GETDEFID, 0, 0);

    ok ( IDCANCEL == (LOWORD(dwVal)), "Did not set default ID\n" );

    /*
     * Check whether message WM_NEXTDLGCTL is changing the focus to next control and if
     * the destination control is a button, style of the button should be changed to
     * BS_DEFPUSHBUTTON with out making it default.
     */

    /*
     * Keep the focus on Edit control.
     */

    if ( SetFocus( g_hwndTestDlgEdit ) )
    {
         ok ((GetFocus() == g_hwndTestDlgEdit), "Focus didn't set on Edit control\n");

        /*
         * Test message WM_NEXTDLGCTL
         */
        DefDlgProc( g_hwndTestDlg, WM_NEXTDLGCTL, 0, 0 );
        ok ((GetFocus() == g_hwndTestDlgBut1), "Focus didn't move to first button\n");

        /*
         * Check whether the default button ID got changed by sending message "WM_NEXTDLGCTL"
         */
        dwVal = DefDlgProc(g_hwndTestDlg, DM_GETDEFID, 0, 0);
        ok ( IDCANCEL == (LOWORD(dwVal)), "WM_NEXTDLGCTL changed default button\n");

        /*
         * Check whether the style of the button which got the focus, changed to BS_DEFPUSHBUTTON and
         * the style of default button changed to BS_PUSHBUTTON.
         */
        if ( IDCANCEL == (LOWORD(dwVal)) )
        {
                ok ( ((GetWindowLong( g_hwndTestDlgBut1, GWL_STYLE)) & BS_DEFPUSHBUTTON) != 0,
                        "Button1 style not set to BS_DEFPUSHBUTTON\n" );

                ok ( !((GetWindowLong( g_hwndTestDlgBut2, GWL_STYLE)) & BS_DEFPUSHBUTTON),
                        "Button2's style not chaged to BS_PUSHBUTTON\n" );
        }

        /*
         * Move focus to Button2 using "WM_NEXTDLGCTL"
         */
        DefDlgProc( g_hwndTestDlg, WM_NEXTDLGCTL, 0, 0 );
        ok ((GetFocus() == g_hwndTestDlgBut2), "Focus didn't move to second button\n");

        /*
         * Check whether the default button ID got changed by sending message "WM_NEXTDLGCTL"
         */
        dwVal = DefDlgProc(g_hwndTestDlg, DM_GETDEFID, 0, 0);
        ok ( IDCANCEL == (LOWORD(dwVal)), "WM_NEXTDLGCTL changed default button\n");

        /*
         * Check whether the style of the button which got the focus, changed to BS_DEFPUSHBUTTON and
         * the style of button which lost the focus changed to BS_PUSHBUTTON.
         */
        if ( IDCANCEL == (LOWORD(dwVal)) )
        {
                ok ( ((GetWindowLong( g_hwndTestDlgBut2, GWL_STYLE)) & BS_DEFPUSHBUTTON) != 0,
                        "Button2 style not set to BS_DEFPUSHBUTTON\n" );

                ok ( !((GetWindowLong( g_hwndTestDlgBut1, GWL_STYLE)) & BS_DEFPUSHBUTTON),
                        "Button1's style not chaged to BS_PUSHBUTTON\n" );
        }

        /*
         * Move focus to Edit control using "WM_NEXTDLGCTL"
         */
        DefDlgProc( g_hwndTestDlg, WM_NEXTDLGCTL, 0, 0 );
        ok ((GetFocus() == g_hwndTestDlgEdit), "Focus didn't move to Edit control\n");

        /*
         * Check whether the default button ID got changed by sending message "WM_NEXTDLGCTL"
         */
        dwVal = DefDlgProc(g_hwndTestDlg, DM_GETDEFID, 0, 0);
        ok ( IDCANCEL == (LOWORD(dwVal)), "WM_NEXTDLGCTL changed default button\n");
    }
    else
        trace("there was no focus before SetFocus\n");
    DestroyWindow(g_hwndTestDlg);
}

static void test_IsDialogMessage(void)
{
    MSG msg;

    g_hwndMain = CreateWindow ("IsDialogMessageWindowClass", "IsDialogMessageWindowClass",
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
            NULL, NULL, g_hinst, 0);

    ok (g_hwndMain != 0, "failed to create the main window\n");
    ok (g_hwndButton1 != 0, "failed to create the first button\n");
    ok (g_hwndButtonCancel != 0, "failed to create the cancel button\n");
    if (!g_hwndMain || !g_hwndButton1 || !g_hwndButtonCancel) return;

    /* The focus should initially be nowhere.  The first TAB should take it
     * to the first button.  The second TAB should take it to the Cancel
     * button.
     */
    FormTabMsg (&msg, g_hwndMain);
    ok (IsDialogMessage (g_hwndMain, &msg), "Did not handle first TAB\n");
    ok ((GetFocus() == g_hwndButton1), "Focus did not move to first button\n");
    FormTabMsg (&msg, g_hwndButton1);
    ok (IsDialogMessage (g_hwndMain, &msg), "Did not handle second TAB\n");
    ok ((GetFocus() == g_hwndButtonCancel),
            "Focus did not move to cancel button\n");
    FormEnterMsg (&msg, g_hwndButtonCancel);
    ok (IsDialogMessage (g_hwndMain, &msg), "Did not handle the ENTER\n");
    ok (g_terminated, "ENTER did not terminate\n");
}

BOOL CALLBACK __export delayFocusDlgWinProc (HWND hDlg, UINT uiMsg, WPARAM wParam,
        LPARAM lParam)
{
    switch (uiMsg)
    {
    case WM_INITDIALOG:
        g_hwndMain = hDlg;
       g_hwndInitialFocusGroupBox = GetDlgItem(hDlg,100);
       g_hwndButton1 = GetDlgItem(hDlg,200);
       g_hwndButton2 = GetDlgItem(hDlg,201);
       g_hwndButtonCancel = GetDlgItem(hDlg,IDCANCEL);
       g_styleInitialFocusT1 = GetWindowLong(g_hwndInitialFocusGroupBox, GWL_STYLE);

       /* Initially check the second radio button */
       SendMessage(g_hwndButton1, BM_SETCHECK, 0, 0);
       SendMessage(g_hwndButton2, BM_SETCHECK, 1, 0);
       /* Continue testing after dialog initialization */
       PostMessage(hDlg, WM_USER, 0, 0);
       return g_bInitialFocusInitDlgResult;

    case WM_COMMAND:
        if (LOWORD(wParam) == IDCANCEL)
       {
           EndDialog(hDlg, LOWORD(wParam));
           return TRUE;
       }
       return FALSE;

    case WM_USER:
       g_styleInitialFocusT2 = GetWindowLong(hDlg, GWL_STYLE);
        g_hwndInitialFocusT1 = GetFocus();
       SetFocus(hDlg);
        g_hwndInitialFocusT2 = GetFocus();
       PostMessage(hDlg, WM_COMMAND, IDCANCEL, 0);
       return TRUE;
    }

    return FALSE;
}

/* Helper for InitialFocusTest */
static LPSTR GetHwndString(HWND hw)
{
  if (hw == NULL)
    return "a null handle";
  if (hw == g_hwndMain)
    return "the dialog handle";
  if (hw == g_hwndInitialFocusGroupBox)
    return "the group box control";
  if (hw == g_hwndButton1)
    return "the first button";
  if (hw == g_hwndButton2)
    return "the second button";
  if (hw == g_hwndButtonCancel)
    return "the cancel button";

  return "unknown handle";
}

static void test_initial_focus(void)
{
    DLGPROC proc = (DLGPROC)MakeProcInstance((FARPROC)delayFocusDlgWinProc, g_hinst);

    /* Test 1:
     * This test intentionally returns FALSE in response to WM_INITDIALOG
     * without setting focus to a control. This is not allowed according to
     * MSDN, but it is exactly what MFC's CFormView does.
     *
     * Since the WM_INITDIALOG handler returns FALSE without setting the focus,
     * the focus should initially be NULL. Later, when we manually set focus to
     * the dialog, the default handler should set focus to the first control that
     * is "visible, not disabled, and has the WS_TABSTOP style" (MSDN). Because the
     * second radio button has been checked, it should be the first control
     * that meets these criteria and should receive the focus.
     */

    /* Wine's test runs this with no window of its own left, so the
     * focus is nowhere when the dialog starts. */
    DestroyWindow(g_hwndMain);
    g_hwndMain = 0;

    g_bInitialFocusInitDlgResult = FALSE;
    g_hwndInitialFocusT1 = (HWND) -1;
    g_hwndInitialFocusT2 = (HWND) -1;
    g_styleInitialFocusT1 = -1;
    g_styleInitialFocusT2 = -1;

    dialog_box(radio_test_dialog(), proc, 0);

    ok (((g_styleInitialFocusT1 & WS_TABSTOP) == 0),
       "Error in wrc - Detected WS_TABSTOP as default style for GROUPBOX\n");

    ok (((g_styleInitialFocusT2 & WS_VISIBLE) == 0),
       "Modal dialogs should not be shown until the message queue first goes empty\n");

    ok ((g_hwndInitialFocusT1 == NULL),
        "Error in initial focus when WM_INITDIALOG returned FALSE: "
        "Expected NULL focus, got %s (%04x).\n",
        GetHwndString(g_hwndInitialFocusT1), g_hwndInitialFocusT1);

    ok ((g_hwndInitialFocusT2 == g_hwndButton2),
        "Error after first SetFocus() when WM_INITDIALOG returned FALSE: "
        "Expected the second button (%04x), got %s (%04x).\n",
        g_hwndButton2, GetHwndString(g_hwndInitialFocusT2),
        g_hwndInitialFocusT2);

    /* Test 2:
     * This is the same as above, except WM_INITDIALOG is made to return TRUE.
     * This should cause the focus to go to the second radio button right away
     * and stay there (until the user indicates otherwise).
     */

    g_bInitialFocusInitDlgResult = TRUE;
    g_hwndInitialFocusT1 = (HWND) -1;
    g_hwndInitialFocusT2 = (HWND) -1;
    g_styleInitialFocusT1 = -1;
    g_styleInitialFocusT2 = -1;

    dialog_box(radio_test_dialog(), proc, 0);

    ok ((g_hwndInitialFocusT1 == g_hwndButton2),
       "Error in initial focus when WM_INITDIALOG returned TRUE: "
       "Expected the second button (%04x), got %s (%04x).\n",
       g_hwndButton2, GetHwndString(g_hwndInitialFocusT1),
       g_hwndInitialFocusT1);

    ok ((g_hwndInitialFocusT2 == g_hwndButton2),
       "Error after first SetFocus() when WM_INITDIALOG returned TRUE: "
       "Expected the second button (%04x), got %s (%04x).\n",
       g_hwndButton2, GetHwndString(g_hwndInitialFocusT2),
       g_hwndInitialFocusT2);

    FreeProcInstance((FARPROC)proc);
}

static void test_GetDlgItemText(void)
{
    char string[64];
    BOOL ret;

    lstrcpy(string, "Overwrite Me");
    ret = GetDlgItemText(NULL, 0, string, sizeof(string)/sizeof(string[0]));
    ok(!ret, "GetDlgItemText(NULL) shouldn't have succeeded\n");

    ok(string[0] == '\0' || broken(!lstrcmp(string, "Overwrite Me")),
       "string retrieved using GetDlgItemText should have been NULL terminated\n");
}

static void test_GetDlgItem(void)
{
    HWND hwnd, child1, child2, hwnd2;
    BOOL ret;

    hwnd = CreateWindow("button", "parent", WS_VISIBLE, 0, 0, 100, 100, NULL, 0, g_hinst, NULL);
    ok(hwnd != NULL, "failed to created window\n");

    /* created with the same ID */
    child1 = CreateWindow("button", "child1", WS_VISIBLE|WS_CHILD, 0, 0, 10, 10, hwnd, 0, g_hinst, NULL);
    ok(child1 != NULL, "failed to create first child\n");
    child2 = CreateWindow("button", "child2", WS_VISIBLE|WS_CHILD, 0, 0, 10, 10, hwnd, 0, g_hinst, NULL);
    ok(child2 != NULL, "failed to create second child\n");

    hwnd2 = GetDlgItem(hwnd, 0);
    ok(hwnd2 == child1, "expected first child, got %04x\n", hwnd2);

    hwnd2 = GetTopWindow(hwnd);
    ok(hwnd2 == child1, "expected first child to be top, got %04x\n", hwnd2);

    ret = SetWindowPos(child1, child2, 0, 0, 0, 0, SWP_NOMOVE);
    ok(ret, "got %d\n", ret);
    hwnd2 = GetTopWindow(hwnd);
    ok(hwnd2 == child2, "expected second child to be top, got %04x\n", hwnd2);

    /* top window from child list is picked */
    hwnd2 = GetDlgItem(hwnd, 0);
    ok(hwnd2 == child2, "expected second child, got %04x\n", hwnd2);

    /* Now test how GetDlgItem searches */
    DestroyWindow(child2);
    child2 = CreateWindow("button", "child2", WS_VISIBLE|WS_CHILD, 0, 0, 10, 10, child1, 0, g_hinst, NULL);
    ok(child2 != NULL, "failed to create second child\n");

    /* give child2 an ID */
    SetWindowWord(child2, GWW_ID, 100);

    hwnd2 = GetDlgItem(hwnd, 100);
    ok(!hwnd2, "expected child to not be found, got %04x\n", hwnd2);

    hwnd2 = GetDlgItem(child1, 100);
    ok(hwnd2 == child2, "expected second child, got %04x\n", hwnd2);

    DestroyWindow(child1);
    DestroyWindow(child2);
    DestroyWindow(hwnd);
}

BOOL CALLBACK __export DestroyDlgWinProc (HWND hDlg, UINT uiMsg,
        WPARAM wParam, LPARAM lParam)
{
    if (uiMsg == WM_INITDIALOG)
    {
        DestroyWindow(hDlg);
        return TRUE;
    }
    return FALSE;
}

BOOL CALLBACK __export DestroyOnCloseDlgWinProc (HWND hDlg, UINT uiMsg,
        WPARAM wParam, LPARAM lParam)
{
    switch (uiMsg)
    {
    case WM_INITDIALOG:
        PostMessage(hDlg, WM_CLOSE, 0, 0);
        return TRUE;
    case WM_CLOSE:
        DestroyWindow(hDlg);
        return TRUE;
    case WM_DESTROY:
        PostQuitMessage(0);
        return TRUE;
    }
    return FALSE;
}

BOOL CALLBACK __export TestInitDialogHandleProc (HWND hDlg, UINT uiMsg,
        WPARAM wParam, LPARAM lParam)
{
    if (uiMsg == WM_INITDIALOG)
    {
        HWND expected = GetNextDlgTabItem(hDlg, NULL, FALSE);
        ok(expected == (HWND)wParam,
           "Expected wParam to be the handle to the first tabstop control (%04x), got %04x\n",
           expected, (HWND)wParam);

        EndDialog(hDlg, LOWORD(SendMessage(hDlg, DM_GETDEFID, 0, 0)));
        return TRUE;
    }
    return FALSE;
}

BOOL CALLBACK __export TestDefButtonDlgProc (HWND hDlg, UINT uiMsg,
                                              WPARAM wParam, LPARAM lParam)
{
    switch (uiMsg)
    {
    case WM_INITDIALOG:
        EndDialog(hDlg, LOWORD(SendMessage(hDlg, DM_GETDEFID, 0, 0)));
        return TRUE;
    }
    return FALSE;
}

static int received_idok = 0;

BOOL CALLBACK __export TestReturnKeyDlgProc (HWND hDlg, UINT uiMsg,
        WPARAM wParam, LPARAM lParam)
{
    switch (uiMsg)
    {
    case WM_INITDIALOG:
        {
            MSG msg;
            memset(&msg, 0, sizeof(msg));
            msg.hwnd = hDlg;
            msg.message = WM_KEYDOWN;
            msg.wParam = VK_RETURN;
            msg.lParam = 0x011c0001L;
            IsDialogMessage(hDlg, &msg);
        }
        ok(received_idok, "WM_COMMAND not received\n");
        EndDialog(hDlg, 0);
        return TRUE;
    case WM_COMMAND:
        ok(wParam==IDOK, "Expected IDOK\n");
        received_idok = 1;
        return TRUE;
    }
    return FALSE;
}

static void test_DialogBoxParamA(void)
{
    int ret;
    HWND hwnd_invalid = (HWND)0x4444;
    MSG msg;

    ret = DialogBoxIndirectParam(g_hinst, idd_dialog(FALSE), hwnd_invalid, 0, 0);
    ok(0 == ret || broken(ret == -1), "DialogBoxParamA returned %d, expected 0\n", ret);

    /* Test a dialog which destroys itself on WM_INITDIALOG. The
     * dialog procedure is called through its exported entry point,
     * without MakeProcInstance. */
    ret = dialog_box(idd_dialog(FALSE), (DLGPROC)DestroyDlgWinProc, 0);
    ok(-1 == ret, "DialogBoxParamA returned %d, expected -1\n", ret);

    ret = DialogBox(g_hinst, "RESOURCE_INVALID", 0, (DLGPROC)DestroyDlgWinProc);
    ok(-1 == ret, "DialogBoxParamA returned %d, expected -1\n", ret);

    ret = (int)DefDlgProc(0, WM_ERASEBKGND, 0, 0);
    ok(ret == 0, "DefDlgProcA returned %d, expected 0\n", ret);

    ret = dialog_box(idd_dialog(TRUE), (DLGPROC)TestInitDialogHandleProc, 0);
    ok(ret == IDOK, "Expected IDOK\n");

    ret = dialog_box(idd_dialog(TRUE), (DLGPROC)TestDefButtonDlgProc, 0);
    ok(ret == IDOK, "Expected IDOK\n");

    dialog_box(idd_dialog(TRUE), (DLGPROC)TestReturnKeyDlgProc, 0);

    /* Test a dialog which destroys itself on WM_CLOSE. Its WM_QUIT is
     * taken out of the queue afterwards, so that it doesn't end the
     * dialogs of the next tests. */
    ret = dialog_box(idd_dialog(FALSE), (DLGPROC)DestroyOnCloseDlgWinProc, 0);
    ok(0 == ret, "DialogBoxParamA returned %d, expected 0\n", ret);
    while (PeekMessage(&msg, 0, WM_QUIT, WM_QUIT, PM_REMOVE))
        ;
}

static void test_DisabledDialogTest(void)
{
    DLGPROC proc = (DLGPROC)MakeProcInstance((FARPROC)disabled_test_proc, g_hinst);

    g_terminated = FALSE;
    dialog_box(idd_dialog(FALSE), proc, 0);
    ok(FALSE == g_terminated, "dialog with disabled ok button has been terminated\n");
    FreeProcInstance((FARPROC)proc);
}

BOOL CALLBACK __export messageBoxFontDlgWinProc (HWND hDlg, UINT uiMsg, WPARAM wParam,
        LPARAM lParam)
{
    return (uiMsg == WM_INITDIALOG);
}

static void test_SaveRestoreFocus(void)
{
    HWND hDlg;
    int foundId;
    HWND foundHwnd;

    /* create the dialog */
    hDlg = CreateDialogIndirect(g_hinst, multi_edit_dialog(), NULL,
                                (DLGPROC)messageBoxFontDlgWinProc);
    ok (hDlg != 0, "Failed to create test dialog.\n");
    if (!hDlg) return;

    foundId = GetDlgCtrlID(GetFocus());
    ok (foundId == 1000, "First edit box should have gained focus on dialog creation. Expected: %d, Found: %d\n", 1000, foundId);

    /* de- then reactivate the dialog */
    SendMessage(hDlg, WM_ACTIVATE, WA_INACTIVE, 0);
    SendMessage(hDlg, WM_ACTIVATE, WA_ACTIVE, 0);

    foundId = GetDlgCtrlID(GetFocus());
    ok (foundId == 1000, "First edit box should have regained focus after dialog reactivation. Expected: %d, Found: %d\n", 1000, foundId);

    /* select the next tabbable item */
    SetFocus(GetNextDlgTabItem(hDlg, GetFocus(), FALSE));

    foundId = GetDlgCtrlID(GetFocus());
    ok (foundId == 1001, "Second edit box should have gained focus. Expected: %d, Found: %d\n", 1001, foundId);

    /* de- then reactivate the dialog */
    SendMessage(hDlg, WM_ACTIVATE, WA_INACTIVE, 0);
    SendMessage(hDlg, WM_ACTIVATE, WA_ACTIVE, 0);

    foundId = GetDlgCtrlID(GetFocus());
    ok (foundId == 1001, "Second edit box should have gained focus after dialog reactivation. Expected: %d, Found: %d\n", 1001, foundId);

    /* disable the 2nd box */
    EnableWindow(GetFocus(), FALSE);

    foundHwnd = GetFocus();
    ok (foundHwnd == NULL, "Second edit box should have lost focus after being disabled. Expected: 0, Found: %04x\n", foundHwnd);

    /* de- then reactivate the dialog */
    SendMessage(hDlg, WM_ACTIVATE, WA_INACTIVE, 0);
    SendMessage(hDlg, WM_ACTIVATE, WA_ACTIVE, 0);

    foundHwnd = GetFocus();
    ok (foundHwnd == NULL, "No controls should have gained focus after dialog reactivation. Expected: 0, Found: %04x\n", foundHwnd);

    /* clean up */
    DestroyWindow(hDlg);
}

static int timer_count;

BOOL CALLBACK __export timer_message_dlg_proc(HWND wnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    BOOL visible;
    DWORD start;

    switch (msg)
    {
        case WM_INITDIALOG:
            visible = (GetWindowLong(wnd, GWL_STYLE) & WS_VISIBLE) != 0;
            ok(!visible, "Dialog should not be visible.\n");
            SetTimer(wnd, 1, 100, NULL);
            /* Windows 3.1 has no Sleep: wait without yielding. */
            start = GetTickCount();
            while (GetTickCount() - start < 200)
                ;
            return FALSE;

        case WM_COMMAND:
            if (LOWORD(wparam) != IDCANCEL) return FALSE;
            EndDialog(wnd, LOWORD(wparam));
            return TRUE;

        case WM_TIMER:
            if (wparam != 1) return FALSE;
            visible = (GetWindowLong(wnd, GWL_STYLE) & WS_VISIBLE) != 0;
            if (!timer_count++)
            {
                ok(!visible, "Dialog should not be visible.\n");
                PostMessage(wnd, WM_USER, 0, 0);
            }
            else
            {
                ok(visible, "Dialog should be visible.\n");
                PostMessage(wnd, WM_COMMAND, IDCANCEL, 0);
            }
            return TRUE;

        case WM_USER:
            visible = (GetWindowLong(wnd, GWL_STYLE) & WS_VISIBLE) != 0;
            ok(visible, "Dialog should be visible.\n");
            return TRUE;

        default:
            return FALSE;
    }
}

static void test_timer_message(void)
{
    DLGPROC proc = (DLGPROC)MakeProcInstance((FARPROC)timer_message_dlg_proc, g_hinst);

    dialog_box(radio_test_dialog(), proc, 0);
    ok(timer_count == 2, "got %d WM_TIMER messages\n", timer_count);
    FreeProcInstance((FARPROC)proc);
}

/* SetDlgItemInt, GetDlgItemInt, CheckDlgButton and CheckRadioButton in
 * a modeless dialog. */
static void test_DlgItemInt(void)
{
    static const struct
    {
        const char *text;
        BOOL is_signed;
        UINT value;
        BOOL translated;
    } tests[] =
    {
        { "1234", FALSE, 1234, TRUE },
        { "  42", FALSE, 42, TRUE },
        { "-5", TRUE, (UINT)-5, TRUE },
        { "65535", FALSE, 65535u, TRUE },
        { "32767", TRUE, 32767, TRUE },
        { "abc", FALSE, 0, FALSE },
        { "12x", FALSE, 0, FALSE },
        { "", FALSE, 0, FALSE },
        { "70000", FALSE, 0, FALSE },
        { "-40000", TRUE, 0, FALSE },
    };
    DLGPROC proc = (DLGPROC)MakeProcInstance((FARPROC)messageBoxFontDlgWinProc, g_hinst);
    HWND hDlg;
    char buf[32];
    UINT value;
    BOOL translated;
    int i;

    begin_dialog(WS_POPUP | WS_CAPTION | DS_SETFONT, 5, 0, 0, 160, 75, "Dialog items");
    add_item(5, 5, 60, 14, 1000, WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
             CLS_EDIT, "");
    add_item(5, 25, 60, 10, 1001, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_CHECKBOX,
             CLS_BUTTON, "Check");
    add_item(5, 40, 60, 10, 1002, WS_CHILD | WS_VISIBLE | WS_GROUP | BS_RADIOBUTTON,
             CLS_BUTTON, "One");
    add_item(5, 50, 60, 10, 1003, WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON,
             CLS_BUTTON, "Two");
    add_item(5, 60, 60, 10, 1004, WS_CHILD | WS_VISIBLE | BS_RADIOBUTTON,
             CLS_BUTTON, "Three");

    hDlg = CreateDialogIndirect(g_hinst, tmpl, NULL, proc);
    ok(hDlg != 0, "Failed to create test dialog.\n");
    if (!hDlg)
    {
        FreeProcInstance((FARPROC)proc);
        return;
    }
    ok(GetDlgItem(hDlg, 1000) != 0, "no edit control\n");
    ok(GetDlgCtrlID(GetDlgItem(hDlg, 1003)) == 1003, "wrong control id\n");

    SetDlgItemInt(hDlg, 1000, 1234, FALSE);
    GetDlgItemText(hDlg, 1000, buf, sizeof(buf));
    ok(!lstrcmp(buf, "1234"), "SetDlgItemInt unsigned gave [%s]\n", (LPSTR)buf);
    SetDlgItemInt(hDlg, 1000, (UINT)-5, TRUE);
    GetDlgItemText(hDlg, 1000, buf, sizeof(buf));
    ok(!lstrcmp(buf, "-5"), "SetDlgItemInt signed gave [%s]\n", (LPSTR)buf);
    SetDlgItemInt(hDlg, 1000, (UINT)-5, FALSE);
    GetDlgItemText(hDlg, 1000, buf, sizeof(buf));
    ok(!lstrcmp(buf, "65531"), "SetDlgItemInt unsigned -5 gave [%s]\n", (LPSTR)buf);

    for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++)
    {
        SetDlgItemText(hDlg, 1000, tests[i].text);
        translated = 0xbeef;
        value = GetDlgItemInt(hDlg, 1000, &translated, tests[i].is_signed);
        ok(value == tests[i].value && translated == tests[i].translated,
           "%d: [%s] signed %d: got %u translated %d, expected %u %d\n", i,
           (LPSTR)tests[i].text, tests[i].is_signed, value, translated,
           tests[i].value, tests[i].translated);
    }

    /* lpTranslated may be null */
    SetDlgItemText(hDlg, 1000, "77");
    value = GetDlgItemInt(hDlg, 1000, NULL, FALSE);
    ok(value == 77, "got %u\n", value);

    ok(!IsDlgButtonChecked(hDlg, 1001), "check box is checked\n");
    CheckDlgButton(hDlg, 1001, 1);
    ok(IsDlgButtonChecked(hDlg, 1001) == 1, "got %u\n", IsDlgButtonChecked(hDlg, 1001));
    CheckDlgButton(hDlg, 1001, 0);
    ok(!IsDlgButtonChecked(hDlg, 1001), "check box is checked\n");

    CheckRadioButton(hDlg, 1002, 1004, 1003);
    ok(!IsDlgButtonChecked(hDlg, 1002), "first radio button checked\n");
    ok(IsDlgButtonChecked(hDlg, 1003) == 1, "second radio button not checked\n");
    ok(!IsDlgButtonChecked(hDlg, 1004), "third radio button checked\n");
    CheckRadioButton(hDlg, 1002, 1004, 1004);
    ok(!IsDlgButtonChecked(hDlg, 1003), "second radio button checked\n");
    ok(IsDlgButtonChecked(hDlg, 1004) == 1, "third radio button not checked\n");

    ok(SendDlgItemMessage(hDlg, 1004, BM_GETCHECK, 0, 0) == 1,
       "SendDlgItemMessage(BM_GETCHECK) didn't return 1\n");

    DestroyWindow(hDlg);
    FreeProcInstance((FARPROC)proc);
}

START_TEST(dialog)
{
    /* A program's instance handle is its data segment's selector. */
    g_hinst = (HINSTANCE)get_ds();

    if (!RegisterWindowClasses())
    {
        ok(0, "failed to register the window classes\n");
        return;
    }

    test_GetNextDlgItem();
    test_IsDialogMessage();
    test_WM_NEXTDLGCTL();
    test_initial_focus();
    test_GetDlgItem();
    test_GetDlgItemText();
    test_DialogBoxParamA();
    test_DisabledDialogTest();
    test_SaveRestoreFocus();
    test_timer_message();
    test_DlgItemInt();
}
