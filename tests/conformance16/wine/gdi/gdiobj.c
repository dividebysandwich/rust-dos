/*
 * Unit test suite for GDI objects
 *
 * Copyright 2002 Mike McCormack
 * Copyright 2004 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/gdiobj.c: SelectObject,
 * DeleteObject, GetObject and UnrealizeObject on null handles, the
 * objects of a display DC from CreateDC, the objects SelectObject puts
 * in a memory DC, and RectInRegion with rectangles whose sides are
 * swapped. Windows 3.1 has no threads, no SetLastError, no
 * GetObjectType, no GetCurrentObject and no ExtCreatePen: the thread
 * test runs in the one task, the checks of the last error and of
 * GetObject with a bad buffer (a fault in Windows 3.1) are left out,
 * and the object a DC has selected is the one SelectObject gives back
 * when the next is selected, instead of GetCurrentObject's.
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

static void test_gdi_objects(void)
{
    BYTE buff[256];
    HDC hdc = GetDC(NULL);
    HPEN hp;
    int i;
    BOOL ret;

    /* SelectObject() with a NULL DC returns 0 */
    hp = SelectObject(NULL, GetStockObject(BLACK_PEN));
    ok(!hp, "SelectObject(NULL DC) expected 0, got %04x\n", hp);

    /* With a valid DC and a NULL object, the call returns 0 */
    hp = SelectObject(hdc, NULL);
    ok(!hp, "SelectObject(NULL obj) expected 0, got %04x\n", hp);

    /* The DC is unaffected by the NULL SelectObject */
    hp = SelectObject(hdc, GetStockObject(BLACK_PEN));
    ok(hp != 0, "SelectObject(post NULL) expected non-null, got %04x\n", hp);

    /* DeleteObject fails on a null object */
    ret = DeleteObject(NULL);
    ok( !ret, "DeleteObject(NULL obj), expected 0, got %d\n", ret);

    /* GetObject fails on a null object */
    i = GetObject(NULL, sizeof(buff), buff);
    ok (!i, "GetObject(NULL obj), expected 0, got %d\n", i);

    /* UnrealizeObject fails on a null object */
    i = UnrealizeObject(NULL);
    ok (!i, "UnrealizeObject(NULL obj), expected 0, got %d\n", i);

    ReleaseDC(NULL, hdc);
}

static void test_thread_objects(void)
{
    LOGPEN lp;
    HDC hdc;
    HPEN hpen;
    HRGN hrgn;
    RECT rc;
    int ret;

    hdc = CreateDC("display", NULL, NULL, NULL);
    ok(hdc != NULL, "CreateDC failed\n");

    hpen = CreatePen(PS_DASHDOTDOT, 17, RGB(1, 2, 3));
    ok(hpen != 0, "Failed to create pen\n");

    hrgn = CreateRectRgn(0, 1, 12, 17);
    ok(hrgn != 0, "Failed to create region\n");

    ok(GetObject(hpen, sizeof(lp), &lp) == sizeof(lp), "GetObject failed\n");
    ok(lp.lopnStyle == PS_DASHDOTDOT, "wrong pen style %d\n", lp.lopnStyle);
    ok(lp.lopnWidth.x == 17, "wrong pen width.y %d\n", lp.lopnWidth.x);
    ok(lp.lopnWidth.y == 0, "wrong pen width.y %d\n", lp.lopnWidth.y);
    ok(lp.lopnColor == RGB(1, 2, 3), "wrong pen width.y %08lx\n", lp.lopnColor);

    ret = GetDeviceCaps(hdc, TECHNOLOGY);
    ok(ret == DT_RASDISPLAY, "GetDeviceCaps(TECHNOLOGY) should return DT_RASDISPLAY not %d\n", ret);

    ok(DeleteObject(hpen), "DeleteObject failed\n");
    ok(DeleteDC(hdc), "DeleteDC failed\n");

    /* The region outlives the DC (GetObjectType in Wine's test). */
    ret = GetRgnBox(hrgn, &rc);
    ok(ret == SIMPLEREGION, "GetRgnBox returned %d\n", ret);
    ok(rc.left == 0 && rc.top == 1 && rc.right == 12 && rc.bottom == 17,
       "GetRgnBox gave (%d,%d)-(%d,%d)\n", rc.left, rc.top, rc.right, rc.bottom);
    ok(DeleteObject(hrgn), "DeleteObject failed\n");
}

static void test_GetCurrentObject(void)
{
    HPEN hpen;
    HBRUSH hbrush;
    HFONT hfont;
    HBITMAP hbmp;
    HRGN hrgn;
    HDC hdc;
    HGDIOBJ hobj;
    int ret;

    hdc = CreateCompatibleDC(0);
    ok(hdc != 0, "CreateCompatibleDC failed\n");
    if (!hdc) return;

    hpen = CreatePen(PS_SOLID, 10, RGB(10, 20, 30));
    ok(hpen != 0, "CreatePen failed\n");
    hobj = SelectObject(hdc, hpen);
    ok(hobj == GetStockObject(BLACK_PEN), "the first pen is %04x\n", hobj);
    hobj = SelectObject(hdc, GetStockObject(WHITE_PEN));
    ok(hobj == hpen, "OBJ_PEN is wrong: %04x\n", hobj);

    hbrush = CreateSolidBrush(RGB(10, 20, 30));
    ok(hbrush != 0, "CreateSolidBrush failed\n");
    hobj = SelectObject(hdc, hbrush);
    ok(hobj == GetStockObject(WHITE_BRUSH), "the first brush is %04x\n", hobj);
    hobj = SelectObject(hdc, GetStockObject(BLACK_BRUSH));
    ok(hobj == hbrush, "OBJ_BRUSH is wrong: %04x\n", hobj);

    hfont = CreateFont(10, 5, 0, 0, FW_DONTCARE, 0, 0, 0, ANSI_CHARSET,
                       OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY,
                       DEFAULT_PITCH, "MS Sans Serif");
    ok(hfont != 0, "CreateFont failed\n");
    hobj = SelectObject(hdc, hfont);
    ok(hobj == GetStockObject(SYSTEM_FONT), "the first font is %04x\n", hobj);
    hobj = SelectObject(hdc, GetStockObject(SYSTEM_FONT));
    ok(hobj == hfont, "OBJ_FONT is wrong: %04x\n", hobj);

    hbmp = CreateBitmap(100, 100, 1, 1, NULL);
    ok(hbmp != 0, "CreateBitmap failed\n");
    hobj = SelectObject(hdc, hbmp);
    ok(hobj != 0, "the first bitmap is %04x\n", hobj);
    hobj = SelectObject(hdc, hobj);
    ok(hobj == hbmp, "OBJ_BITMAP is wrong: %04x\n", hobj);
    SelectObject(hdc, hbmp);

    hrgn = CreateRectRgn(1, 1, 100, 100);
    ok(hrgn != 0, "CreateRectRgn failed\n");
    /* Selecting a region selects it as the clip region and returns its
     * kind (Wine's test checks that GetCurrentObject has no region). */
    ret = (int)(UINT)SelectObject(hdc, hrgn);
    ok(ret == SIMPLEREGION, "SelectObject of a region returned %d\n", ret);

    DeleteDC(hdc);
    DeleteObject(hpen);
    DeleteObject(hbrush);
    DeleteObject(hfont);
    DeleteObject(hbmp);
    DeleteObject(hrgn);
}

static void test_region(void)
{
    HRGN hrgn = CreateRectRgn(10, 10, 20, 20);
    RECT rc = { 5, 5, 15, 15 };
    BOOL ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap left and right */
    SetRect( &rc, 15, 5, 5, 15 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap top and bottom */
    SetRect( &rc, 5, 15, 15, 5 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap both */
    SetRect( &rc, 15, 15, 5, 5 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    DeleteObject(hrgn);
    /* swap left and right in the region */
    hrgn = CreateRectRgn(20, 10, 10, 20);
    SetRect( &rc, 5, 5, 15, 15 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap left and right */
    SetRect( &rc, 15, 5, 5, 15 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap top and bottom */
    SetRect( &rc, 5, 15, 15, 5 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    /* swap both */
    SetRect( &rc, 15, 15, 5, 5 );
    ret = RectInRegion( hrgn, &rc);
    ok( ret, "RectInRegion should return TRUE\n");
    DeleteObject(hrgn);
}

START_TEST(gdiobj)
{
    test_gdi_objects();
    test_thread_objects();
    test_GetCurrentObject();
    test_region();
}
