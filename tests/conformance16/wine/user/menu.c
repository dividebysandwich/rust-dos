/*
 * Unit tests for menus
 *
 * Copyright 2005 Robert Shearman
 * Copyright 2007 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/user32/tests/menu.c. Most of Wine's
 * test uses MENUITEMINFO, GetMenuInfo and the other menu functions
 * that came with Windows 95; the parts that use the Windows 3.1 calls
 * are kept: MF_HILITE in AppendMenu, InsertMenu and ModifyMenu (the
 * MF_DEFAULT checks are left out, as Windows 3.1 has no MF_DEFAULT),
 * HiliteMenuItem, the search by command (with GetMenuString and
 * GetMenuState in place of GetMenuItemInfo), the depth limit and the
 * circular reference, and the menu a window keeps after DestroyMenu.
 * Checks of CreateMenu, AppendMenu, InsertMenu, ModifyMenu,
 * GetMenuString, GetMenuItemCount, GetMenuItemID, GetMenuState,
 * GetSubMenu, CheckMenuItem, EnableMenuItem, DeleteMenu and RemoveMenu
 * are added. The menu loop that WM_SYSCOMMAND SC_KEYMENU enters is
 * ended by the window procedure on WM_ENTERIDLE, as in Wine's test,
 * and also by a timer, so that the test never waits for input. The
 * todo_wine marks are dropped: the checks expect what Windows does.
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

static HINSTANCE main_instance;
static ATOM atomMenuCheckClass;
static BOOL got_initmenu;

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

LRESULT CALLBACK __export menu_check_wnd_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam)
{
    switch (msg)
    {
    case WM_INITMENU:
        /* it sets a variable here, where Wine's test sets
         * GWLP_USERDATA */
        got_initmenu = TRUE;
        break;
    case WM_ENTERIDLE:
        PostMessage(hwnd, WM_CANCELMODE, 0, 0);
        return TRUE;
    case WM_TIMER:
        /* the fallback that ends the menu loop */
        KillTimer(hwnd, wparam);
        PostMessage(hwnd, WM_CANCELMODE, 0, 0);
        PostMessage(hwnd, WM_KEYDOWN, VK_ESCAPE, 0);
        return 0;
    }
    return DefWindowProc(hwnd, msg, wparam, lparam);
}

static void register_menu_check_class(void)
{
    WNDCLASS wc;

    memset(&wc, 0, sizeof(wc));
    wc.lpfnWndProc = (WNDPROC)menu_check_wnd_proc;
    wc.hInstance = main_instance;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_BTNFACE+1);
    wc.lpszClassName = "WineMenuCheck";

    atomMenuCheckClass = RegisterClass(&wc);
    ok(atomMenuCheckClass != 0, "RegisterClass failed\n");
}

/* demonstrates that windows locks the menu object so that it is still valid
 * even after a client calls DestroyMenu on it */
static void test_menu_locked_by_window(void)
{
    BOOL ret;
    HMENU hmenu;
    HWND hwnd = CreateWindow("WineMenuCheck", NULL,
                             WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 200, 200,
                             NULL, NULL, main_instance, NULL);
    ok(hwnd != NULL, "CreateWindowEx failed\n");
    hmenu = CreateMenu();
    ok(hmenu != NULL, "CreateMenu failed\n");
    ret = InsertMenu(hmenu, 0, MF_STRING, 0, "&Test");
    ok(ret, "InsertMenu failed\n");
    ret = SetMenu(hwnd, hmenu);
    ok(ret, "SetMenu failed\n");
    ret = DestroyMenu(hmenu);
    ok(ret, "DestroyMenu failed\n");

    /* Windows 3.1's DrawMenuBar returns nothing */
    DrawMenuBar(hwnd);

    SetTimer(hwnd, 1, 2000, NULL);
    got_initmenu = FALSE;
    SendMessage(hwnd, WM_SYSCOMMAND, SC_KEYMENU, 0);
    /* did we process the WM_INITMENU message? */
    ok(got_initmenu, "WM_INITMENU should have been sent\n");
    KillTimer(hwnd, 1);

    /* Last, as the emulator doesn't have USER.358 IsMenu yet. */
    ret = IsMenu(GetMenu(hwnd));
    ok(!ret || broken(ret) /* nt4 */, "Menu handle should have been destroyed\n");

    DestroyWindow(hwnd);
}

static void test_menu_flags( void )
{
    HMENU hMenu, hPopupMenu;

    hMenu = CreateMenu();
    hPopupMenu = CreatePopupMenu();

    AppendMenu(hMenu, MF_POPUP | MF_STRING, (UINT)hPopupMenu, "Popup");

    AppendMenu(hPopupMenu, MF_STRING | MF_HILITE, 101, "Item 1");
    InsertMenu(hPopupMenu, 1, MF_BYPOSITION | MF_STRING | MF_HILITE, 102, "Item 2");
    AppendMenu(hPopupMenu, MF_STRING, 103, "Item 3");
    ModifyMenu(hPopupMenu, 2, MF_BYPOSITION | MF_STRING | MF_HILITE, 103, "Item 3");

    ok((GetMenuState(hPopupMenu, 0, MF_BYPOSITION) & MF_HILITE) != 0,
      "AppendMenu should accept MF_HILITE\n");
    ok((GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE) != 0,
      "InsertMenu should accept MF_HILITE\n");
    ok((GetMenuState(hPopupMenu, 2, MF_BYPOSITION) & MF_HILITE) != 0,
      "ModifyMenu should accept MF_HILITE\n");

    DestroyMenu(hMenu);
}

static void test_menu_hilitemenuitem( void )
{
    HMENU hMenu, hPopupMenu;
    WNDCLASS wclass;
    HWND hWnd;

    wclass.lpszClassName = "HiliteMenuTestClass";
    wclass.style         = CS_HREDRAW | CS_VREDRAW;
    wclass.lpfnWndProc   = (WNDPROC)DefWindowProc;
    wclass.hInstance     = main_instance;
    wclass.hIcon         = LoadIcon( 0, IDI_APPLICATION );
    wclass.hCursor       = LoadCursor( NULL, IDC_ARROW );
    wclass.hbrBackground = (HBRUSH)( COLOR_WINDOW + 1 );
    wclass.lpszMenuName  = 0;
    wclass.cbClsExtra    = 0;
    wclass.cbWndExtra    = 0;
    ok (RegisterClass( &wclass ) != 0, "RegisterClass failed\n");
    hWnd = CreateWindow( wclass.lpszClassName, "HiliteMenuTest",
                         WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, 0,
                         400, 200, NULL, NULL, wclass.hInstance, NULL);
    ok (hWnd != 0, "CreateWindow failed\n");

    hMenu = CreateMenu();
    hPopupMenu = CreatePopupMenu();

    AppendMenu(hMenu, MF_POPUP | MF_STRING, (UINT)hPopupMenu, "Popup");

    AppendMenu(hPopupMenu, MF_STRING, 101, "Item 1");
    AppendMenu(hPopupMenu, MF_STRING, 102, "Item 2");
    AppendMenu(hPopupMenu, MF_STRING, 103, "Item 3");

    SetMenu(hWnd, hMenu);

    /* test invalid arguments */

    ok(!(GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 2 is hilited\n");

    ok(!HiliteMenuItem(NULL, hPopupMenu, 1, MF_HILITE | MF_BYPOSITION),
      "HiliteMenuItem: call should have failed.\n");

    ok(!HiliteMenuItem(hWnd, NULL, 1, MF_HILITE | MF_BYPOSITION),
      "HiliteMenuItem: call should have failed.\n");

    ok(!(GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 2 is hilited\n");

    /* either MF_HILITE or MF_UNHILITE *and* MF_BYCOMMAND or MF_BYPOSITION need to be set */

    ok(HiliteMenuItem(hWnd, hPopupMenu, 1, MF_BYPOSITION),
      "HiliteMenuItem: call should have succeeded.\n");

    ok(!(GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 2 is hilited\n");

    ok(HiliteMenuItem(hWnd, hPopupMenu, 1, MF_HILITE),
      "HiliteMenuItem: call should have succeeded.\n");

    ok(!(GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 2 is hilited\n");

    /* hilite a menu item (by position) */

    ok(HiliteMenuItem(hWnd, hPopupMenu, 1, MF_HILITE | MF_BYPOSITION),
      "HiliteMenuItem: call should not have failed.\n");

    ok((GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE) != 0,
      "HiliteMenuItem: Item 2 is not hilited\n");

    /* unhilite a menu item (by position) */

    ok(HiliteMenuItem(hWnd, hPopupMenu, 1, MF_UNHILITE | MF_BYPOSITION),
      "HiliteMenuItem: call should not have failed.\n");

    ok(!(GetMenuState(hPopupMenu, 1, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 2 is hilited\n");

    /* hilite a menu item (by command) */

    ok(HiliteMenuItem(hWnd, hPopupMenu, 103, MF_HILITE | MF_BYCOMMAND),
      "HiliteMenuItem: call should not have failed.\n");

    ok((GetMenuState(hPopupMenu, 2, MF_BYPOSITION) & MF_HILITE) != 0,
      "HiliteMenuItem: Item 3 is not hilited\n");

    /* unhilite a menu item (by command) */

    ok(HiliteMenuItem(hWnd, hPopupMenu, 103, MF_UNHILITE | MF_BYCOMMAND),
      "HiliteMenuItem: call should not have failed.\n");

    ok(!(GetMenuState(hPopupMenu, 2, MF_BYPOSITION) & MF_HILITE),
      "HiliteMenuItem: Item 3 is hilited\n");

    DestroyWindow(hWnd);
}

/* Cases 1 and 2 of Wine's test_menu_search_bycommand, with InsertMenu
 * in place of InsertMenuItem and GetMenuString in place of
 * GetMenuItemInfo. */
static void test_menu_search_bycommand( void )
{
    HMENU        hmenu, hmenuSub;
    BOOL         rc;
    UINT         id;
    char         strback[0x80];
    char         strIn[0x80];

    /* Case 1: Menu containing a menu item */
    hmenu = CreateMenu();

    lstrcpy(strIn, "Case 1 MenuItem");
    rc = InsertMenu(hmenu, 0, MF_BYPOSITION | MF_STRING, 0x1234, strIn);
    ok (rc, "Inserting the menuitem failed\n");

    id = GetMenuItemID(hmenu, 0);
    ok (id == 0x1234, "Getting the menuitem id failed(gave %x)\n", id);

    /* Confirm the menuitem was given the id supplied (getting by position) */
    strback[0] = 0x00;
    rc = GetMenuString(hmenu, 0, strback, sizeof(strback), MF_BYPOSITION);
    ok (rc, "Getting the menu items info failed\n");
    ok (!lstrcmp(strback, "Case 1 MenuItem"), "Returned item has wrong label\n");

    /* Search by id - Should return the item */
    strback[0] = 0x00;
    rc = GetMenuString(hmenu, 0x1234, strback, sizeof(strback), MF_BYCOMMAND);
    ok (rc, "Getting the menu items info failed\n");
    ok (!lstrcmp(strback, "Case 1 MenuItem"), "Returned item has wrong label\n");

    DestroyMenu( hmenu );

    /* Case 2: Menu containing a popup menu */
    hmenu = CreateMenu();
    hmenuSub = CreateMenu();

    lstrcpy(strIn, "Case 2 SubMenu");
    rc = InsertMenu(hmenu, 0, MF_BYPOSITION | MF_POPUP | MF_STRING, (UINT)hmenuSub, strIn);
    ok (rc, "Inserting the popup menu into the main menu failed\n");

    id = GetMenuItemID(hmenu, 0);
    ok (id == (UINT)-1, "Getting the menuitem id unexpectedly worked (gave %x)\n", id);

    /* Search by id - returns the popup menu itself */
    strback[0] = 0x00;
    rc = GetMenuString(hmenu, (UINT)hmenuSub, strback, sizeof(strback), MF_BYCOMMAND);
    ok (rc, "Getting the menu items info failed\n");
    ok (!lstrcmp(strback, "Case 2 SubMenu"), "Returned item has wrong label\n");

    /*
        Now add an item after it with the same id
     */
    lstrcpy(strIn, "Case 2 MenuItem 1");
    rc = InsertMenu(hmenu, -1, MF_BYPOSITION | MF_STRING, (UINT)hmenuSub, strIn);
    ok (rc, "Inserting the menuitem failed\n");

    /* Search by id - returns the item which follows the popup menu */
    strback[0] = 0x00;
    rc = GetMenuString(hmenu, (UINT)hmenuSub, strback, sizeof(strback), MF_BYCOMMAND);
    ok (rc, "Getting the menu items info failed\n");
    ok (!lstrcmp(strback, "Case 2 MenuItem 1"), "Returned item has wrong label (%s)\n", (LPSTR)strback);

    /*
        Now add an item before the popup (with the same id)
     */
    lstrcpy(strIn, "Case 2 MenuItem 2");
    rc = InsertMenu(hmenu, 0, MF_BYPOSITION | MF_STRING, (UINT)hmenuSub, strIn);
    ok (rc, "Inserting the menuitem failed\n");

    /* Search by id - returns the item which precedes the popup menu */
    strback[0] = 0x00;
    rc = GetMenuString(hmenu, (UINT)hmenuSub, strback, sizeof(strback), MF_BYCOMMAND);
    ok (rc, "Getting the menu items info failed\n");
    ok (!lstrcmp(strback, "Case 2 MenuItem 2"), "Returned item has wrong label (%s)\n", (LPSTR)strback);

    DestroyMenu( hmenu );
    DestroyMenu( hmenuSub );
}

static void test_menu_maxdepth(void)
{
#define NR_MENUS 100
    HMENU hmenus[ NR_MENUS];
    int i;
    DWORD ret;

    for( i = 0; i < NR_MENUS; i++) {
        hmenus[i] = CreatePopupMenu();
        if( !hmenus[i]) break;
    }
    ok( i == NR_MENUS, "could not create more than %d menu's\n", i);
    for( i = 1; i < NR_MENUS; i++) {
        ret = AppendMenu( hmenus[i], MF_POPUP, (UINT)hmenus[i-1],"test");
        if( !ret) break;
    }
    trace("Maximum depth is %d\n", i);
    ok( i < NR_MENUS ||
           broken( i == NR_MENUS), /* win98, NT */
           "no ( or very large) limit on menu depth!\n");

    for( i = 0; i < NR_MENUS; i++)
        DestroyMenu( hmenus[i]);
}

/* bug #12171 */
static void test_menu_circref(void)
{
    HMENU menu1, menu2;
    DWORD ret;

    menu1 = CreatePopupMenu();
    menu2 = CreatePopupMenu();
    ok( menu1 && menu2, "error creating menus.\n");
    ret = AppendMenu( menu1, MF_POPUP, (UINT)menu2, "winetest");
    ok( ret != 0, "AppendMenu failed\n");
    ret = AppendMenu( menu1, MF_STRING | MF_HILITE, 123, "winetest");
    ok( ret != 0, "AppendMenu failed\n");
    /* app chooses an id that happens to clash with its own hmenu */
    ret = AppendMenu( menu2, MF_STRING, (UINT)menu2, "winetest");
    ok( ret != 0, "AppendMenu failed\n");
    /* now attempt to change the string of the first item of menu1 */
    ret = ModifyMenu( menu1, (UINT)menu2, MF_POPUP, (UINT)menu2, "menu 2");
    ok( !ret ||
            broken( ret), /* win98, NT */
            "ModifyMenu should have failed.\n");
    if( !ret) { /* will probably stack fault if the ModifyMenu succeeded */
        ret = GetMenuState( menu1, 123, 0);
        ok( ret == MF_HILITE, "GetMenuState returned %lx\n",ret);
    }
    DestroyMenu( menu2);
    DestroyMenu( menu1);
}

/* The basic Windows 3.1 menu calls. */
static void test_menu_basic(void)
{
    HMENU hmenu, hpopup, hpopup2;
    char buf[32];
    UINT state;
    int ret;
    BOOL rc;

    hmenu = CreateMenu();
    ok(hmenu != 0, "CreateMenu failed\n");
    hpopup = CreatePopupMenu();
    ok(hpopup != 0, "CreatePopupMenu failed\n");
    ok(GetMenuItemCount(hmenu) == 0, "new menu has %d items\n", GetMenuItemCount(hmenu));

    rc = AppendMenu(hmenu, MF_STRING, 10, "&File");
    ok(rc, "AppendMenu failed\n");
    rc = AppendMenu(hmenu, MF_STRING | MF_POPUP, (UINT)hpopup, "&Edit");
    ok(rc, "AppendMenu popup failed\n");
    rc = AppendMenu(hpopup, MF_STRING, 20, "Cu&t");
    ok(rc, "AppendMenu failed\n");
    rc = AppendMenu(hpopup, MF_SEPARATOR, 0, NULL);
    ok(rc, "AppendMenu separator failed\n");
    rc = AppendMenu(hpopup, MF_STRING | MF_GRAYED, 21, "&Paste");
    ok(rc, "AppendMenu failed\n");
    /* insert before the first item, by position */
    rc = InsertMenu(hmenu, 0, MF_BYPOSITION | MF_STRING, 5, "&Game");
    ok(rc, "InsertMenu by position failed\n");
    /* insert before the item with id 10 */
    rc = InsertMenu(hmenu, 10, MF_BYCOMMAND | MF_STRING, 7, "&Options");
    ok(rc, "InsertMenu by command failed\n");
    /* position -1 appends */
    rc = InsertMenu(hmenu, -1, MF_BYPOSITION | MF_STRING | MF_HELP, 99, "&Help");
    ok(rc, "InsertMenu at -1 failed\n");

    ok(GetMenuItemCount(hmenu) == 5, "menu has %d items\n", GetMenuItemCount(hmenu));
    ok(GetMenuItemCount(hpopup) == 3, "popup has %d items\n", GetMenuItemCount(hpopup));
    ok(GetMenuItemCount((HMENU)0x1234) == -1, "GetMenuItemCount of a bad handle didn't return -1\n");

    ok(GetMenuItemID(hmenu, 0) == 5, "item 0 has id %u\n", GetMenuItemID(hmenu, 0));
    ok(GetMenuItemID(hmenu, 1) == 7, "item 1 has id %u\n", GetMenuItemID(hmenu, 1));
    ok(GetMenuItemID(hmenu, 2) == 10, "item 2 has id %u\n", GetMenuItemID(hmenu, 2));
    ok(GetMenuItemID(hmenu, 3) == (UINT)-1, "popup item has id %u\n", GetMenuItemID(hmenu, 3));
    ok(GetMenuItemID(hmenu, 4) == 99, "item 4 has id %u\n", GetMenuItemID(hmenu, 4));
    ok(GetMenuItemID(hpopup, 1) == 0, "separator has id %u\n", GetMenuItemID(hpopup, 1));
    ok(GetMenuItemID(hmenu, 5) == (UINT)-1, "item past the end has id %u\n", GetMenuItemID(hmenu, 5));

    ok(GetSubMenu(hmenu, 3) == hpopup, "GetSubMenu returned %04x\n", GetSubMenu(hmenu, 3));
    ok(GetSubMenu(hmenu, 0) == 0, "GetSubMenu of a plain item returned %04x\n", GetSubMenu(hmenu, 0));

    /* GetMenuString */
    ret = GetMenuString(hmenu, 1, buf, sizeof(buf), MF_BYPOSITION);
    ok(ret == 8 && !lstrcmp(buf, "&Options"), "got %d [%s]\n", ret, (LPSTR)buf);
    ret = GetMenuString(hmenu, 20, buf, sizeof(buf), MF_BYCOMMAND);
    ok(ret == 4 && !lstrcmp(buf, "Cu&t"), "item in the popup by command: got %d [%s]\n", ret, (LPSTR)buf);
    ret = GetMenuString(hmenu, 3, buf, sizeof(buf), MF_BYPOSITION);
    ok(ret == 5 && !lstrcmp(buf, "&Edit"), "popup: got %d [%s]\n", ret, (LPSTR)buf);
    ret = GetMenuString(hmenu, 1, buf, 4, MF_BYPOSITION);
    ok(ret == 3 && !lstrcmp(buf, "&Op"), "truncated: got %d [%s]\n", ret, (LPSTR)buf);
    buf[0] = 'x';
    ret = GetMenuString(hpopup, 1, buf, sizeof(buf), MF_BYPOSITION);
    ok(ret == 0, "separator: got %d\n", ret);
    ret = GetMenuString(hmenu, 1234, buf, sizeof(buf), MF_BYCOMMAND);
    ok(ret == 0, "missing item: got %d\n", ret);

    /* GetMenuState */
    state = GetMenuState(hmenu, 10, MF_BYCOMMAND);
    ok(state == MF_STRING, "plain item state %04x\n", state);
    state = GetMenuState(hpopup, 21, MF_BYCOMMAND);
    ok(state == MF_GRAYED, "grayed item state %04x\n", state);
    state = GetMenuState(hmenu, 21, MF_BYCOMMAND);
    ok(state == MF_GRAYED, "grayed item in the popup, state %04x\n", state);
    /* Windows 95 reports a separator as grayed (MFS_GRAYED) in
     * GetMenuItemInfo, so only the MF_SEPARATOR bit is checked. */
    state = GetMenuState(hpopup, 1, MF_BYPOSITION);
    trace("separator state %04x\n", state);
    ok((state & MF_SEPARATOR) != 0, "separator state %04x\n", state);
    state = GetMenuState(hmenu, 4, MF_BYPOSITION);
    ok(state == MF_HELP, "help item state %04x\n", state);
    /* for a popup the high byte is the number of items in it */
    state = GetMenuState(hmenu, 3, MF_BYPOSITION);
    ok(state == ((3 << 8) | MF_POPUP), "popup state %04x\n", state);
    state = GetMenuState(hmenu, 1234, MF_BYCOMMAND);
    ok(state == (UINT)-1, "missing item state %04x\n", state);

    /* CheckMenuItem returns the previous state */
    ret = CheckMenuItem(hmenu, 10, MF_BYCOMMAND | MF_CHECKED);
    ok(ret == MF_UNCHECKED, "CheckMenuItem returned %d\n", ret);
    ok(GetMenuState(hmenu, 10, MF_BYCOMMAND) == MF_CHECKED, "state %04x\n",
       GetMenuState(hmenu, 10, MF_BYCOMMAND));
    ret = CheckMenuItem(hmenu, 2, MF_BYPOSITION | MF_UNCHECKED);
    ok(ret == MF_CHECKED, "CheckMenuItem returned %d\n", ret);
    ok(GetMenuState(hmenu, 10, MF_BYCOMMAND) == MF_UNCHECKED, "state %04x\n",
       GetMenuState(hmenu, 10, MF_BYCOMMAND));
    ret = CheckMenuItem(hmenu, 20, MF_BYCOMMAND | MF_CHECKED);
    ok(ret == MF_UNCHECKED, "CheckMenuItem in the popup returned %d\n", ret);
    ok(GetMenuState(hpopup, 0, MF_BYPOSITION) == MF_CHECKED, "state %04x\n",
       GetMenuState(hpopup, 0, MF_BYPOSITION));
    ret = CheckMenuItem(hmenu, 1234, MF_BYCOMMAND | MF_CHECKED);
    ok(ret == -1, "CheckMenuItem of a missing item returned %d\n", ret);

    /* EnableMenuItem returns the previous state */
    ret = EnableMenuItem(hmenu, 7, MF_BYCOMMAND | MF_GRAYED);
    ok(ret == MF_ENABLED, "EnableMenuItem returned %d\n", ret);
    ok(GetMenuState(hmenu, 7, MF_BYCOMMAND) == MF_GRAYED, "state %04x\n",
       GetMenuState(hmenu, 7, MF_BYCOMMAND));
    ret = EnableMenuItem(hmenu, 1, MF_BYPOSITION | MF_DISABLED);
    ok(ret == MF_GRAYED, "EnableMenuItem returned %d\n", ret);
    ok(GetMenuState(hmenu, 7, MF_BYCOMMAND) == MF_DISABLED, "state %04x\n",
       GetMenuState(hmenu, 7, MF_BYCOMMAND));
    ret = EnableMenuItem(hmenu, 7, MF_BYCOMMAND | MF_ENABLED);
    ok(ret == MF_DISABLED, "EnableMenuItem returned %d\n", ret);
    ok(GetMenuState(hmenu, 7, MF_BYCOMMAND) == MF_ENABLED, "state %04x\n",
       GetMenuState(hmenu, 7, MF_BYCOMMAND));
    ret = EnableMenuItem(hmenu, 21, MF_BYCOMMAND | MF_ENABLED);
    ok(ret == MF_GRAYED, "EnableMenuItem in the popup returned %d\n", ret);
    ret = EnableMenuItem(hmenu, 1234, MF_BYCOMMAND | MF_GRAYED);
    ok(ret == -1, "EnableMenuItem of a missing item returned %d\n", ret);

    /* ModifyMenu */
    rc = ModifyMenu(hmenu, 0, MF_BYPOSITION | MF_STRING, 6, "&Play");
    ok(rc, "ModifyMenu by position failed\n");
    ok(GetMenuItemID(hmenu, 0) == 6, "modified item has id %u\n", GetMenuItemID(hmenu, 0));
    ret = GetMenuString(hmenu, 6, buf, sizeof(buf), MF_BYCOMMAND);
    ok(ret == 5 && !lstrcmp(buf, "&Play"), "modified item: got %d [%s]\n", ret, (LPSTR)buf);
    rc = ModifyMenu(hmenu, 6, MF_BYCOMMAND | MF_STRING | MF_CHECKED, 6, "P&ause");
    ok(rc, "ModifyMenu by command failed\n");
    ret = GetMenuString(hmenu, 0, buf, sizeof(buf), MF_BYPOSITION);
    ok(ret == 6 && !lstrcmp(buf, "P&ause"), "modified item: got %d [%s]\n", ret, (LPSTR)buf);
    ok(GetMenuState(hmenu, 6, MF_BYCOMMAND) == MF_CHECKED, "state %04x\n",
       GetMenuState(hmenu, 6, MF_BYCOMMAND));
    ok(GetMenuItemCount(hmenu) == 5, "menu has %d items\n", GetMenuItemCount(hmenu));
    rc = ModifyMenu(hmenu, 1234, MF_BYCOMMAND | MF_STRING, 8, "None");
    ok(!rc, "ModifyMenu of a missing item succeeded\n");

    /* RemoveMenu keeps the popup, DeleteMenu destroys it */
    hpopup2 = CreatePopupMenu();
    AppendMenu(hpopup2, MF_STRING, 30, "Item");
    rc = AppendMenu(hmenu, MF_POPUP, (UINT)hpopup2, "&Window");
    ok(rc, "AppendMenu failed\n");
    ok(GetMenuItemCount(hmenu) == 6, "menu has %d items\n", GetMenuItemCount(hmenu));
    rc = RemoveMenu(hmenu, 5, MF_BYPOSITION);
    ok(rc, "RemoveMenu failed\n");
    ok(GetMenuItemCount(hmenu) == 5, "menu has %d items\n", GetMenuItemCount(hmenu));
    ok(GetMenuItemCount(hpopup2) == 1, "removed popup has %d items\n",
       GetMenuItemCount(hpopup2));
    rc = DeleteMenu(hmenu, 7, MF_BYCOMMAND);
    ok(rc, "DeleteMenu by command failed\n");
    ok(GetMenuItemCount(hmenu) == 4, "menu has %d items\n", GetMenuItemCount(hmenu));
    ok(GetMenuItemID(hmenu, 1) == 10, "item 1 has id %u\n", GetMenuItemID(hmenu, 1));
    rc = DeleteMenu(hmenu, 2, MF_BYPOSITION);
    ok(rc, "DeleteMenu of the popup failed\n");
    ok(GetMenuItemCount(hmenu) == 3, "menu has %d items\n", GetMenuItemCount(hmenu));
    ok(GetMenuItemCount(hpopup) == -1, "the deleted popup still has %d items\n",
       GetMenuItemCount(hpopup));
    rc = DeleteMenu(hmenu, 1234, MF_BYCOMMAND);
    ok(!rc, "DeleteMenu of a missing item succeeded\n");

    ok(DestroyMenu(hpopup2), "DestroyMenu failed\n");
    ok(DestroyMenu(hmenu), "DestroyMenu failed\n");
    ok(GetMenuItemCount(hmenu) == -1, "destroyed menu still has %d items\n",
       GetMenuItemCount(hmenu));
}

/* A window's menu, and the system menu. */
static void test_window_menu(void)
{
    HWND hwnd;
    HMENU hmenu, hsys;
    int count;

    hwnd = CreateWindow("WineMenuCheck", "menu", WS_OVERLAPPEDWINDOW,
                        0, 0, 200, 200, NULL, NULL, main_instance, NULL);
    ok(hwnd != 0, "CreateWindow failed\n");
    ok(GetMenu(hwnd) == 0, "new window has menu %04x\n", GetMenu(hwnd));

    hmenu = CreateMenu();
    AppendMenu(hmenu, MF_STRING, 1, "&One");
    ok(SetMenu(hwnd, hmenu), "SetMenu failed\n");
    ok(GetMenu(hwnd) == hmenu, "GetMenu returned %04x\n", GetMenu(hwnd));
    ok(SetMenu(hwnd, 0), "SetMenu(0) failed\n");
    ok(GetMenu(hwnd) == 0, "GetMenu returned %04x\n", GetMenu(hwnd));
    ok(GetMenuItemCount(hmenu) == 1, "the menu is gone after SetMenu(0)\n");
    DestroyMenu(hmenu);

    hsys = GetSystemMenu(hwnd, FALSE);
    ok(hsys != 0, "no system menu\n");
    count = GetMenuItemCount(hsys);
    trace("system menu has %d items\n", count);
    ok(count > 0, "system menu has %d items\n", count);
    ok(GetMenuState(hsys, SC_CLOSE, MF_BYCOMMAND) != (UINT)-1, "no SC_CLOSE in the system menu\n");
    ok(AppendMenu(hsys, MF_STRING, 0x100, "Extra"), "AppendMenu to the system menu failed\n");
    ok(GetMenuItemCount(hsys) == count + 1, "system menu has %d items\n", GetMenuItemCount(hsys));
    /* bRevert restores the standard system menu */
    GetSystemMenu(hwnd, TRUE);
    hsys = GetSystemMenu(hwnd, FALSE);
    ok(GetMenuItemCount(hsys) == count, "reverted system menu has %d items\n",
       GetMenuItemCount(hsys));

    DestroyWindow(hwnd);
}

START_TEST(menu)
{
    /* A program's instance handle is its data segment's selector. */
    main_instance = (HINSTANCE)get_ds();

    register_menu_check_class();

    test_menu_basic();
    test_window_menu();
    test_menu_flags();
    test_menu_hilitemenuitem();
    test_menu_search_bycommand();
    test_menu_maxdepth();
    test_menu_circref();
    test_menu_locked_by_window();
}
