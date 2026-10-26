/*
 * Unit test suite for clipping
 *
 * Copyright 2005 Huw Davies
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/clipping.c: of
 * test_GetRandomRgn the clip rectangles IntersectClipRect makes in a
 * window's DC, and of test_GetClipRgn what SelectClipRgn returns when it
 * sets and clears the clip region, with the clip region seen through
 * GetClipBox. Windows 3.1 has no GetRandomRgn, no SetMetaRgn, no
 * GetRegionData, no ExtCreateRegion and no GetClipRgn that copies the
 * clip region, so the tests of those are left out. test_regions,
 * test_clip_ops and test_clip_drawing aren't Wine's: they check
 * CreateRectRgn, CombineRgn, GetRgnBox, OffsetRgn, PtInRegion,
 * RectInRegion and EqualRgn, then IntersectClipRect, ExcludeClipRect,
 * OffsetClipRgn and SaveDC/RestoreDC through GetClipBox and PtVisible,
 * and then which pixels PatBlt changes inside and outside a clip region.
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

#ifndef CLR_INVALID
#define CLR_INVALID 0xffffffffL
#endif

static const char *rect_str(const RECT *rc)
{
    static char buf[4][40];
    static int n;
    char *p = buf[n++ & 3];
    wsprintf(p, "(%d,%d)-(%d,%d)", rc->left, rc->top, rc->right, rc->bottom);
    return p;
}

static void test_GetRandomRgn(void)
{
    HWND hwnd = CreateWindow("BUTTON","test",WS_VISIBLE|WS_POPUP,0,0,100,100,GetDesktopWindow(),0,0,0);
    HDC hdc;
    int ret;
    RECT rc, rc2;
    RECT ret_rc, window_rc;

    ok( hwnd != 0, "CreateWindow failed\n" );

    SetRect(&window_rc, 400, 300, 500, 400);
    MoveWindow(hwnd, window_rc.left, window_rc.top, window_rc.right - window_rc.left, window_rc.bottom - window_rc.top, FALSE);
    hdc = GetDC(hwnd);

    /* Without a clip region the clip box is the window's client area */
    ret = GetClipBox(hdc, &ret_rc);
    ok(ret == SIMPLEREGION, "GetClipBox rets %d\n", ret);
    ok(ret_rc.left == 0 && ret_rc.top == 0 && ret_rc.right == 100 && ret_rc.bottom == 100,
       "GetClipBox %s\n", rect_str(&ret_rc));

    /* Set a clip region */
    SetRect(&rc, 20, 20, 80, 80);
    ret = IntersectClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom);
    ok(ret == SIMPLEREGION, "IntersectClipRect rets %d\n", ret);

    ret = GetClipBox(hdc, &ret_rc);
    ok(ret == SIMPLEREGION, "GetClipBox rets %d\n", ret);
    ok(EqualRect(&rc, &ret_rc), "GetClipBox %s\n", rect_str(&ret_rc));

    /* Set a new clip, which intersects with the first */
    SetRect(&rc2, 10, 30, 70, 90);
    ret = IntersectClipRect(hdc, rc2.left, rc2.top, rc2.right, rc2.bottom);
    ok(ret == SIMPLEREGION, "IntersectClipRect rets %d\n", ret);

    IntersectRect(&rc2, &rc, &rc2);

    ret = GetClipBox(hdc, &ret_rc);
    ok(ret == SIMPLEREGION, "GetClipBox rets %d\n", ret);
    ok(EqualRect(&rc2, &ret_rc), "GetClipBox %s, expected %s\n",
       rect_str(&ret_rc), rect_str(&rc2));

    ReleaseDC(hwnd, hdc);
    DestroyWindow(hwnd);
}

static void test_GetClipRgn(void)
{
    HDC hdc;
    HRGN hrgn, hrgn2;
    RECT rc;
    int ret;

    hdc = GetDC(NULL);
    ok(hdc != NULL, "Expected GetDC to return a valid device context handle\n");

    /* Initialize the test regions. */
    hrgn = CreateRectRgn(100, 100, 100, 100);
    ok(hrgn != NULL,
       "Expected CreateRectRgn to return a handle to a new rectangular region\n");
    hrgn2 = CreateRectRgn(1, 2, 3, 4);
    ok(hrgn2 != NULL,
       "Expected CreateRectRgn to return a handle to a new rectangular region\n");

    /* Try setting a clipping region. */
    ret = SelectClipRgn(hdc, hrgn);
    ok(ret == NULLREGION,
       "Expected SelectClipRgn to return NULLREGION, got %d\n", ret);
    ret = GetClipBox(hdc, &rc);
    ok(ret == NULLREGION, "GetClipBox returned %d\n", ret);
    ok(IsRectEmpty(&rc), "GetClipBox gave %s\n", rect_str(&rc));

    /* Try unsetting it. */
    ret = SelectClipRgn(hdc, NULL);
    ok(ret == SIMPLEREGION,
       "Expected SelectClipRgn to return SIMPLEREGION, got %d\n", ret);
    ret = GetClipBox(hdc, &rc);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d\n", ret);
    ok(rc.left == 0 && rc.top == 0 &&
       rc.right == GetDeviceCaps(hdc, HORZRES) && rc.bottom == GetDeviceCaps(hdc, VERTRES),
       "GetClipBox gave %s\n", rect_str(&rc));

    /* SelectClipRgn copies the region: changing it later changes nothing */
    ret = SelectClipRgn(hdc, hrgn2);
    ok(ret == SIMPLEREGION, "SelectClipRgn returned %d\n", ret);
    SetRectRgn(hrgn2, 10, 10, 20, 20);
    ret = GetClipBox(hdc, &rc);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d\n", ret);
    ok(rc.left == 1 && rc.top == 2 && rc.right == 3 && rc.bottom == 4,
       "GetClipBox gave %s\n", rect_str(&rc));
    SelectClipRgn(hdc, NULL);

    DeleteObject(hrgn2);
    DeleteObject(hrgn);
    ReleaseDC(NULL, hdc);
}

static void test_regions(void)
{
    HRGN r1, r2, dst, cmp;
    RECT rc;
    int ret;

    r1 = CreateRectRgn(10, 10, 50, 50);
    r2 = CreateRectRgn(30, 30, 70, 70);
    dst = CreateRectRgn(0, 0, 0, 0);
    ok(r1 && r2 && dst, "CreateRectRgn failed\n");

    ret = GetRgnBox(r1, &rc);
    ok(ret == SIMPLEREGION, "GetRgnBox returned %d\n", ret);
    ok(rc.left == 10 && rc.top == 10 && rc.right == 50 && rc.bottom == 50,
       "GetRgnBox gave %s\n", rect_str(&rc));

    ret = GetRgnBox(dst, &rc);
    ok(ret == NULLREGION, "GetRgnBox of an empty region returned %d\n", ret);

    ret = CombineRgn(dst, r1, r2, RGN_AND);
    ok(ret == SIMPLEREGION, "RGN_AND returned %d\n", ret);
    GetRgnBox(dst, &rc);
    ok(rc.left == 30 && rc.top == 30 && rc.right == 50 && rc.bottom == 50,
       "RGN_AND gave %s\n", rect_str(&rc));

    ret = CombineRgn(dst, r1, r2, RGN_OR);
    ok(ret == COMPLEXREGION, "RGN_OR returned %d\n", ret);
    GetRgnBox(dst, &rc);
    ok(rc.left == 10 && rc.top == 10 && rc.right == 70 && rc.bottom == 70,
       "RGN_OR gave %s\n", rect_str(&rc));
    ok(PtInRegion(dst, 15, 15), "(15,15) isn't in the union\n");
    ok(PtInRegion(dst, 65, 65), "(65,65) isn't in the union\n");
    ok(!PtInRegion(dst, 65, 15), "(65,15) is in the union\n");
    ok(!PtInRegion(dst, 15, 65), "(15,65) is in the union\n");
    /* right and bottom edges are outside */
    ok(!PtInRegion(dst, 70, 69), "(70,69) is in the union\n");
    ok(PtInRegion(dst, 69, 69), "(69,69) isn't in the union\n");

    ret = CombineRgn(dst, r1, r2, RGN_DIFF);
    ok(ret == COMPLEXREGION, "RGN_DIFF returned %d\n", ret);
    ok(PtInRegion(dst, 15, 15), "(15,15) isn't in the difference\n");
    ok(!PtInRegion(dst, 40, 40), "(40,40) is in the difference\n");
    GetRgnBox(dst, &rc);
    ok(rc.left == 10 && rc.top == 10 && rc.right == 50 && rc.bottom == 50,
       "RGN_DIFF gave %s\n", rect_str(&rc));

    ret = CombineRgn(dst, r1, r2, RGN_XOR);
    ok(ret == COMPLEXREGION, "RGN_XOR returned %d\n", ret);
    ok(!PtInRegion(dst, 40, 40), "(40,40) is in the xor\n");
    ok(PtInRegion(dst, 60, 60), "(60,60) isn't in the xor\n");

    ret = CombineRgn(dst, r1, NULL, RGN_COPY);
    ok(ret == SIMPLEREGION, "RGN_COPY returned %d\n", ret);
    ok(EqualRgn(dst, r1), "RGN_COPY isn't equal to its source\n");
    ok(!EqualRgn(dst, r2), "different regions are equal\n");

    /* Regions that don't meet */
    SetRectRgn(r2, 100, 100, 120, 120);
    ret = CombineRgn(dst, r1, r2, RGN_AND);
    ok(ret == NULLREGION, "RGN_AND of disjoint regions returned %d\n", ret);

    ret = OffsetRgn(r1, -5, 7);
    ok(ret == SIMPLEREGION, "OffsetRgn returned %d\n", ret);
    GetRgnBox(r1, &rc);
    ok(rc.left == 5 && rc.top == 17 && rc.right == 45 && rc.bottom == 57,
       "OffsetRgn gave %s\n", rect_str(&rc));

    SetRect(&rc, 0, 0, 6, 18);
    ok(RectInRegion(r1, &rc), "RectInRegion of an overlapping rect failed\n");
    SetRect(&rc, 0, 0, 5, 17);
    ok(!RectInRegion(r1, &rc), "RectInRegion of a rect next to it succeeded\n");

    cmp = CreateRectRgnIndirect(&rc);
    ok(cmp != 0, "CreateRectRgnIndirect failed\n");
    GetRgnBox(cmp, &rc);
    ok(rc.left == 0 && rc.top == 0 && rc.right == 5 && rc.bottom == 17,
       "CreateRectRgnIndirect gave %s\n", rect_str(&rc));

    DeleteObject(cmp);
    DeleteObject(dst);
    DeleteObject(r2);
    DeleteObject(r1);
}

static void test_clip_ops(void)
{
    HDC hdc = CreateCompatibleDC(0);
    HBITMAP bmp = CreateBitmap(100, 100, 1, 1, NULL);
    HBITMAP old = SelectObject(hdc, bmp);
    RECT rc;
    int ret, saved;

    ret = GetClipBox(hdc, &rc);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d\n", ret);
    ok(rc.left == 0 && rc.top == 0 && rc.right == 100 && rc.bottom == 100,
       "GetClipBox gave %s\n", rect_str(&rc));

    saved = SaveDC(hdc);
    ok(saved != 0, "SaveDC failed\n");

    ret = IntersectClipRect(hdc, 10, 10, 60, 60);
    ok(ret == SIMPLEREGION, "IntersectClipRect returned %d\n", ret);
    ret = ExcludeClipRect(hdc, 20, 20, 30, 30);
    ok(ret == COMPLEXREGION, "ExcludeClipRect returned %d\n", ret);
    ret = GetClipBox(hdc, &rc);
    ok(ret == COMPLEXREGION, "GetClipBox returned %d\n", ret);
    ok(rc.left == 10 && rc.top == 10 && rc.right == 60 && rc.bottom == 60,
       "GetClipBox gave %s\n", rect_str(&rc));
    ok(PtVisible(hdc, 15, 15), "(15,15) isn't visible\n");
    ok(!PtVisible(hdc, 25, 25), "(25,25) is visible\n");
    ok(!PtVisible(hdc, 5, 5), "(5,5) is visible\n");
    SetRect(&rc, 21, 21, 29, 29);
    ok(!RectVisible(hdc, &rc), "the excluded rectangle is visible\n");
    SetRect(&rc, 0, 0, 11, 11);
    ok(RectVisible(hdc, &rc), "a rectangle at the corner isn't visible\n");

    ret = OffsetClipRgn(hdc, 5, -5);
    ok(ret == COMPLEXREGION, "OffsetClipRgn returned %d\n", ret);
    ret = GetClipBox(hdc, &rc);
    ok(rc.left == 15 && rc.top == 5 && rc.right == 65 && rc.bottom == 55,
       "GetClipBox after OffsetClipRgn gave %s\n", rect_str(&rc));
    ok(!PtVisible(hdc, 27, 17), "(27,17) is visible\n");
    ok(PtVisible(hdc, 22, 27), "(22,27) isn't visible\n");

    /* Excluding all of it */
    ret = ExcludeClipRect(hdc, 0, 0, 100, 100);
    ok(ret == NULLREGION, "ExcludeClipRect of everything returned %d\n", ret);

    ok(RestoreDC(hdc, saved), "RestoreDC failed\n");
    ret = GetClipBox(hdc, &rc);
    ok(ret == SIMPLEREGION, "GetClipBox after RestoreDC returned %d\n", ret);
    ok(rc.left == 0 && rc.top == 0 && rc.right == 100 && rc.bottom == 100,
       "GetClipBox after RestoreDC gave %s\n", rect_str(&rc));

    /* The clip rectangles are in logical coordinates */
    SetWindowOrg(hdc, -10, -20);
    ret = IntersectClipRect(hdc, 0, 0, 10, 10);
    ok(ret == SIMPLEREGION, "IntersectClipRect returned %d\n", ret);
    ret = GetClipBox(hdc, &rc);
    ok(rc.left == 0 && rc.top == 0 && rc.right == 10 && rc.bottom == 10,
       "GetClipBox with a window origin gave %s\n", rect_str(&rc));
    SetWindowOrg(hdc, 0, 0);
    ret = GetClipBox(hdc, &rc);
    ok(rc.left == 10 && rc.top == 20 && rc.right == 20 && rc.bottom == 30,
       "GetClipBox without the window origin gave %s\n", rect_str(&rc));

    SelectObject(hdc, old);
    DeleteObject(bmp);
    DeleteDC(hdc);
}

static void test_clip_drawing(void)
{
    HDC hdc = CreateCompatibleDC(0);
    HBITMAP bmp = CreateBitmap(32, 32, 1, 1, NULL);
    HBITMAP old = SelectObject(hdc, bmp);
    HRGN rgn;
    COLORREF c;
    int ret;

    PatBlt(hdc, 0, 0, 32, 32, WHITENESS);
    c = GetPixel(hdc, 5, 5);
    ok(c == RGB(255,255,255), "the bitmap isn't white: %08lx\n", c);

    rgn = CreateRectRgn(8, 8, 16, 16);
    ret = SelectClipRgn(hdc, rgn);
    ok(ret == SIMPLEREGION, "SelectClipRgn returned %d\n", ret);
    ret = ExcludeClipRect(hdc, 10, 10, 12, 12);
    ok(ret == COMPLEXREGION, "ExcludeClipRect returned %d\n", ret);

    PatBlt(hdc, 0, 0, 32, 32, BLACKNESS);
    c = GetPixel(hdc, 8, 8);
    ok(c == RGB(0,0,0), "(8,8) is %08lx\n", c);
    c = GetPixel(hdc, 15, 15);
    ok(c == RGB(0,0,0), "(15,15) is %08lx\n", c);
    c = GetPixel(hdc, 12, 12);
    ok(c == RGB(0,0,0), "(12,12) is %08lx\n", c);

    /* GetPixel outside the clip region fails */
    c = GetPixel(hdc, 20, 20);
    ok(c == CLR_INVALID, "GetPixel outside the clip region gave %08lx\n", c);
    c = GetPixel(hdc, 10, 10);
    ok(c == CLR_INVALID, "GetPixel in the excluded rectangle gave %08lx\n", c);

    /* What PatBlt left outside the clip region */
    SelectClipRgn(hdc, NULL);
    c = GetPixel(hdc, 20, 20);
    ok(c == RGB(255,255,255), "(20,20) is %08lx\n", c);
    c = GetPixel(hdc, 16, 16);
    ok(c == RGB(255,255,255), "(16,16) is %08lx\n", c);
    c = GetPixel(hdc, 7, 8);
    ok(c == RGB(255,255,255), "(7,8) is %08lx\n", c);
    c = GetPixel(hdc, 10, 10);
    ok(c == RGB(255,255,255), "(10,10), excluded, is %08lx\n", c);
    c = GetPixel(hdc, 11, 11);
    ok(c == RGB(255,255,255), "(11,11), excluded, is %08lx\n", c);
    c = GetPixel(hdc, 15, 8);
    ok(c == RGB(0,0,0), "(15,8) is %08lx\n", c);

    SelectObject(hdc, old);
    DeleteObject(rgn);
    DeleteObject(bmp);
    DeleteDC(hdc);
}

START_TEST(clipping)
{
    test_GetRandomRgn();
    test_GetClipRgn();
    test_regions();
    test_clip_ops();
    test_clip_drawing();
}
