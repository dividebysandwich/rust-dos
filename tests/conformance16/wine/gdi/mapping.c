/*
 * Unit tests for mapping functions
 *
 * Copyright (c) 2005 Huw Davies
 * Copyright (c) 2008 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/mapping.c: SetMapMode
 * with MM_TEXT and MM_LOMETRIC, the extents GetViewportExtEx and
 * GetWindowExtEx give for them, LPtoDP, and how MM_ISOTROPIC adjusts the
 * viewport's extent. Windows 3.1 has no world transforms, no graphics
 * modes, no SetVirtualResolution and no GetTransform, so the tests of
 * those are left out, and of the MM_LOMETRIC extents only Windows 9x's
 * are kept. The extents and origins are also checked with Windows 3.1's
 * functions that return them as a DWORD (GetViewportExt and the others),
 * and test_dword_forms and test_lptodp aren't Wine's: they check what
 * SetWindowOrg, SetViewportExt, OffsetViewportOrg, ScaleWindowExt,
 * MoveTo and the other functions that return a point in a DWORD give
 * back, against the *Ex functions, and LPtoDP and DPtoLP on arrays of
 * points.
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

static int iabs(int x) { return x < 0 ? -x : x; }

#define rough_match(got, expected) (iabs((got) - (expected)) <= 5)

/* The x and y of a point a DWORD holds, as signed 16-bit numbers. */
#define DW_X(dw) ((int)LOWORD(dw))
#define DW_Y(dw) ((int)HIWORD(dw))

#define expect_LPtoDP(_hdc, _x, _y) \
{ \
    POINT _pt = { 1000, 1000 }; \
    LPtoDP(_hdc, &_pt, 1); \
    ok(rough_match(_pt.x, _x), "expected x %d, got %d\n", (_x), _pt.x); \
    ok(rough_match(_pt.y, _y), "expected y %d, got %d\n", (_y), _pt.y); \
}

#define expect_dc_ext(_func, _func16, _hdc, _cx, _cy) \
{ \
    BOOL _ret; \
    SIZE _size; \
    DWORD _dw; \
    _ret = _func(_hdc, &_size); \
    ok(_ret, #_func " failed\n"); \
    ok(_size.cx == (_cx), "expected cx %d, got %d\n", (_cx), _size.cx); \
    ok(_size.cy == (_cy), "expected cy %d, got %d\n", (_cy), _size.cy); \
    _dw = _func16(_hdc); \
    ok(DW_X(_dw) == (_cx) && DW_Y(_dw) == (_cy), \
       #_func16 " gave %08lx, expected %d x %d\n", _dw, (_cx), (_cy)); \
}

#define expect_viewport_ext(_hdc, _cx, _cy) \
    expect_dc_ext(GetViewportExtEx, GetViewportExt, _hdc, _cx, _cy)
#define expect_window_ext(_hdc, _cx, _cy) \
    expect_dc_ext(GetWindowExtEx, GetWindowExt, _hdc, _cx, _cy)

static void test_world_transform(void)
{
    HDC hdc;
    int ret, size_cx, size_cy, res_x, res_y, dpi_x, dpi_y;

    hdc = CreateCompatibleDC(0);

    size_cx = GetDeviceCaps(hdc, HORZSIZE);
    size_cy = GetDeviceCaps(hdc, VERTSIZE);
    res_x = GetDeviceCaps(hdc, HORZRES);
    res_y = GetDeviceCaps(hdc, VERTRES);
    dpi_x = GetDeviceCaps(hdc, LOGPIXELSX);
    dpi_y = GetDeviceCaps(hdc, LOGPIXELSY);
    trace("dc size %d x %d, resolution %d x %d dpi %d x %d\n",
          size_cx, size_cy, res_x, res_y, dpi_x, dpi_y );

    expect_viewport_ext(hdc, 1, 1);
    expect_window_ext(hdc, 1, 1);
    expect_LPtoDP(hdc, 1000, 1000);

    ret = SetMapMode(hdc, MM_LOMETRIC);
    ok(ret == MM_TEXT, "expected MM_TEXT, got %d\n", ret);

    expect_viewport_ext(hdc, dpi_x, dpi_y);
    expect_window_ext(hdc, 254, -254);
    expect_LPtoDP(hdc, MulDiv(1000 / 10, res_x, size_cx), -MulDiv(1000 / 10, res_y, size_cy));

    ret = SetMapMode(hdc, MM_TEXT);
    ok(ret == MM_LOMETRIC, "expected MM_LOMETRIC, got %d\n", ret);

    expect_viewport_ext(hdc, 1, 1);
    expect_window_ext(hdc, 1, 1);
    expect_LPtoDP(hdc, 1000, 1000);

    DeleteDC(hdc);
}

static void test_SetWindowExt(HDC hdc, int cx, int cy, int expected_vp_cx, int expected_vp_cy)
{
    SIZE windowExt, viewportExt;
    POINT windowOrg, windowOrgAfter, viewportOrg, viewportOrgAfter;
    DWORD dw;

    GetWindowOrgEx(hdc, &windowOrg);
    GetViewportOrgEx(hdc, &viewportOrg);

    SetWindowExtEx(hdc, cx, cy, NULL);
    GetWindowExtEx(hdc, &windowExt);
    ok(windowExt.cx == cx && windowExt.cy == cy,
       "Window extension: Expected %dx%d, got %dx%d\n",
       cx, cy, windowExt.cx, windowExt.cy);

    dw = GetWindowExt(hdc);
    ok(DW_X(dw) == windowExt.cx && DW_Y(dw) == windowExt.cy,
       "GetWindowExt gave %08lx, expected %dx%d\n", dw, windowExt.cx, windowExt.cy);

    GetViewportExtEx(hdc, &viewportExt);
    ok(rough_match(viewportExt.cx, expected_vp_cx) && rough_match(viewportExt.cy, expected_vp_cy),
        "Viewport extents have not been properly adjusted: Expected %dx%d, got %dx%d\n",
        expected_vp_cx, expected_vp_cy, viewportExt.cx, viewportExt.cy);
    dw = GetViewportExt(hdc);
    ok(DW_X(dw) == viewportExt.cx && DW_Y(dw) == viewportExt.cy,
       "GetViewportExt gave %08lx, expected %dx%d\n", dw, viewportExt.cx, viewportExt.cy);

    GetWindowOrgEx(hdc, &windowOrgAfter);
    ok(windowOrg.x == windowOrgAfter.x && windowOrg.y == windowOrgAfter.y,
        "Window origin changed from (%d,%d) to (%d,%d)\n",
        windowOrg.x, windowOrg.y, windowOrgAfter.x, windowOrgAfter.y);

    GetViewportOrgEx(hdc, &viewportOrgAfter);
    ok(viewportOrg.x == viewportOrgAfter.x && viewportOrg.y == viewportOrgAfter.y,
        "Viewport origin changed from (%d,%d) to (%d,%d)\n",
        viewportOrg.x, viewportOrg.y, viewportOrgAfter.x, viewportOrgAfter.y);
}

static void test_SetViewportExt(HDC hdc, int cx, int cy, int expected_vp_cx, int expected_vp_cy)
{
    SIZE windowExt, windowExtAfter, viewportExt;
    POINT windowOrg, windowOrgAfter, viewportOrg, viewportOrgAfter;
    DWORD dw;

    GetWindowOrgEx(hdc, &windowOrg);
    GetViewportOrgEx(hdc, &viewportOrg);
    GetWindowExtEx(hdc, &windowExt);

    SetViewportExtEx(hdc, cx, cy, NULL);
    GetViewportExtEx(hdc, &viewportExt);
    ok(rough_match(viewportExt.cx, expected_vp_cx) && rough_match(viewportExt.cy, expected_vp_cy),
        "Viewport extents have not been properly adjusted: Expected %dx%d, got %dx%d\n",
        expected_vp_cx, expected_vp_cy, viewportExt.cx, viewportExt.cy);

    GetWindowExtEx(hdc, &windowExtAfter);
    ok(windowExt.cx == windowExtAfter.cx && windowExt.cy == windowExtAfter.cy,
       "Window extension changed from %dx%d to %dx%d\n",
       windowExt.cx, windowExt.cy, windowExtAfter.cx, windowExtAfter.cy);
    dw = GetWindowExt(hdc);
    ok(DW_X(dw) == windowExtAfter.cx && DW_Y(dw) == windowExtAfter.cy,
       "GetWindowExt gave %08lx, expected %dx%d\n", dw, windowExtAfter.cx, windowExtAfter.cy);

    GetWindowOrgEx(hdc, &windowOrgAfter);
    ok(windowOrg.x == windowOrgAfter.x && windowOrg.y == windowOrgAfter.y,
        "Window origin changed from (%d,%d) to (%d,%d)\n",
        windowOrg.x, windowOrg.y, windowOrgAfter.x, windowOrgAfter.y);

    GetViewportOrgEx(hdc, &viewportOrgAfter);
    ok(viewportOrg.x == viewportOrgAfter.x && viewportOrg.y == viewportOrgAfter.y,
        "Viewport origin changed from (%d,%d) to (%d,%d)\n",
        viewportOrg.x, viewportOrg.y, viewportOrgAfter.x, viewportOrgAfter.y);
}

static void test_isotropic_mapping(void)
{
    SIZE win, vp;
    HDC hdc = GetDC(0);

    SetMapMode(hdc, MM_ISOTROPIC);

    /* MM_ISOTROPIC is set up like MM_LOMETRIC.
       Initial values after SetMapMode():
       (1 inch = 25.4 mm)

                       Windows 9x:               Windows NT:
       Window Ext:     254 x -254                HORZSIZE*10 x VERTSIZE*10
       Viewport Ext:   LOGPIXELSX x LOGPIXELSY   HORZRES x -VERTRES

       To test without rounding errors, we have to use multiples of
       these values!
     */

    GetWindowExtEx(hdc, &win);
    GetViewportExtEx(hdc, &vp);

    test_SetViewportExt(hdc, 10 * vp.cx, 10 * vp.cy, 10 * vp.cx, 10 * vp.cy);
    test_SetWindowExt(hdc, win.cx, win.cy, 10 * vp.cx, 10 * vp.cy);
    test_SetWindowExt(hdc, 2 * win.cx, win.cy, 10 * vp.cx, 5 * vp.cy);
    test_SetWindowExt(hdc, win.cx, win.cy, 5 * vp.cx, 5 * vp.cy);
    test_SetViewportExt(hdc, 4 * vp.cx, 2 * vp.cy, 2 * vp.cx, 2 * vp.cy);
    test_SetViewportExt(hdc, vp.cx, 2 * vp.cy, vp.cx, vp.cy);
    test_SetViewportExt(hdc, 2 * vp.cx, 2 * vp.cy, 2 * vp.cx, 2 * vp.cy);
    test_SetViewportExt(hdc, 4 * vp.cx, 2 * vp.cy, 2 * vp.cx, 2 * vp.cy);
    test_SetWindowExt(hdc, 4 * win.cx, 2 * win.cy, 2 * vp.cx, vp.cy);
    test_SetViewportExt(hdc, -2 * vp.cx, -4 * vp.cy, -2 * vp.cx, -vp.cy);
    test_SetViewportExt(hdc, -2 * vp.cx, -1 * vp.cy, -2 * vp.cx, -vp.cy);
    test_SetWindowExt(hdc, -4 * win.cx, -2 * win.cy, -2 * vp.cx, -vp.cy);
    test_SetWindowExt(hdc, 4 * win.cx, -4 * win.cy, -vp.cx, -vp.cy);

    SetMapMode(hdc, MM_TEXT);
    ReleaseDC(0, hdc);
}

#define expect_point(_what, _dw, _x, _y) \
    ok(DW_X(_dw) == (_x) && DW_Y(_dw) == (_y), \
       _what " gave (%d,%d), expected (%d,%d)\n", DW_X(_dw), DW_Y(_dw), (_x), (_y))

static void test_dword_forms(void)
{
    HDC hdc = CreateCompatibleDC(0);
    POINT pt;
    SIZE sz;
    DWORD dw;
    int ret;

    ret = SetMapMode(hdc, MM_ANISOTROPIC);
    ok(ret == MM_TEXT, "SetMapMode returned %d\n", ret);
    ret = GetMapMode(hdc);
    ok(ret == MM_ANISOTROPIC, "GetMapMode returned %d\n", ret);

    dw = SetWindowOrg(hdc, -100, 200);
    expect_point("SetWindowOrg", dw, 0, 0);
    dw = SetWindowOrg(hdc, 300, -400);
    expect_point("SetWindowOrg", dw, -100, 200);
    dw = GetWindowOrg(hdc);
    expect_point("GetWindowOrg", dw, 300, -400);
    ok(GetWindowOrgEx(hdc, &pt) && pt.x == 300 && pt.y == -400,
       "GetWindowOrgEx gave (%d,%d)\n", pt.x, pt.y);

    dw = SetViewportOrg(hdc, 10, -20);
    expect_point("SetViewportOrg", dw, 0, 0);
    dw = SetViewportOrg(hdc, -30, 40);
    expect_point("SetViewportOrg", dw, 10, -20);
    dw = GetViewportOrg(hdc);
    expect_point("GetViewportOrg", dw, -30, 40);

    dw = SetWindowExt(hdc, 1000, -2000);
    expect_point("SetWindowExt", dw, 1, 1);
    dw = SetViewportExt(hdc, 500, 4000);
    expect_point("SetViewportExt", dw, 1, 1);
    dw = GetWindowExt(hdc);
    expect_point("GetWindowExt", dw, 1000, -2000);
    dw = GetViewportExt(hdc);
    expect_point("GetViewportExt", dw, 500, 4000);
    ok(GetViewportExtEx(hdc, &sz) && sz.cx == 500 && sz.cy == 4000,
       "GetViewportExtEx gave %dx%d\n", sz.cx, sz.cy);

    dw = OffsetWindowOrg(hdc, 5, -5);
    expect_point("OffsetWindowOrg", dw, 300, -400);
    dw = GetWindowOrg(hdc);
    expect_point("GetWindowOrg after OffsetWindowOrg", dw, 305, -405);
    dw = OffsetViewportOrg(hdc, -7, 7);
    expect_point("OffsetViewportOrg", dw, -30, 40);
    dw = GetViewportOrg(hdc);
    expect_point("GetViewportOrg after OffsetViewportOrg", dw, -37, 47);

    dw = ScaleWindowExt(hdc, 1, 2, 3, 4);
    expect_point("ScaleWindowExt", dw, 1000, -2000);
    dw = GetWindowExt(hdc);
    expect_point("GetWindowExt after ScaleWindowExt", dw, 500, -1500);
    dw = ScaleViewportExt(hdc, 3, 1, 1, 2);
    expect_point("ScaleViewportExt", dw, 500, 4000);
    dw = GetViewportExt(hdc);
    expect_point("GetViewportExt after ScaleViewportExt", dw, 1500, 2000);

    /* The *Ex forms give the previous values too */
    ok(SetWindowOrgEx(hdc, 0, 0, &pt) && pt.x == 305 && pt.y == -405,
       "SetWindowOrgEx gave (%d,%d)\n", pt.x, pt.y);
    ok(SetViewportOrgEx(hdc, 0, 0, &pt) && pt.x == -37 && pt.y == 47,
       "SetViewportOrgEx gave (%d,%d)\n", pt.x, pt.y);
    ok(SetWindowExtEx(hdc, 1, 1, &sz) && sz.cx == 500 && sz.cy == -1500,
       "SetWindowExtEx gave %dx%d\n", sz.cx, sz.cy);
    ok(SetViewportExtEx(hdc, 1, 1, &sz) && sz.cx == 1500 && sz.cy == 2000,
       "SetViewportExtEx gave %dx%d\n", sz.cx, sz.cy);

    SetMapMode(hdc, MM_TEXT);

    /* MoveTo gives back the old position, GetCurrentPosition the new */
    dw = MoveTo(hdc, -12, 34);
    expect_point("MoveTo", dw, 0, 0);
    dw = MoveTo(hdc, 56, -78);
    expect_point("MoveTo", dw, -12, 34);
    dw = GetCurrentPosition(hdc);
    expect_point("GetCurrentPosition", dw, 56, -78);
    ok(MoveToEx(hdc, 1, 2, &pt) && pt.x == 56 && pt.y == -78,
       "MoveToEx gave (%d,%d)\n", pt.x, pt.y);
    ok(GetCurrentPositionEx(hdc, &pt) && pt.x == 1 && pt.y == 2,
       "GetCurrentPositionEx gave (%d,%d)\n", pt.x, pt.y);

    DeleteDC(hdc);
}

static void test_lptodp(void)
{
    HDC hdc = CreateCompatibleDC(0);
    POINT pts[3];
    BOOL ret;

    SetMapMode(hdc, MM_ANISOTROPIC);
    SetWindowOrg(hdc, 100, 200);
    SetWindowExt(hdc, 10, 20);
    SetViewportOrg(hdc, -5, 5);
    SetViewportExt(hdc, 30, -40);

    pts[0].x = 100; pts[0].y = 200;
    pts[1].x = 110; pts[1].y = 220;
    pts[2].x = 90;  pts[2].y = 190;
    ret = LPtoDP(hdc, pts, 3);
    ok(ret, "LPtoDP failed\n");
    ok(pts[0].x == -5 && pts[0].y == 5, "point 0 is (%d,%d)\n", pts[0].x, pts[0].y);
    ok(pts[1].x == 25 && pts[1].y == -35, "point 1 is (%d,%d)\n", pts[1].x, pts[1].y);
    ok(pts[2].x == -35 && pts[2].y == 25, "point 2 is (%d,%d)\n", pts[2].x, pts[2].y);

    ret = DPtoLP(hdc, pts, 3);
    ok(ret, "DPtoLP failed\n");
    ok(pts[0].x == 100 && pts[0].y == 200, "point 0 is (%d,%d)\n", pts[0].x, pts[0].y);
    ok(pts[1].x == 110 && pts[1].y == 220, "point 1 is (%d,%d)\n", pts[1].x, pts[1].y);
    ok(pts[2].x == 90 && pts[2].y == 190, "point 2 is (%d,%d)\n", pts[2].x, pts[2].y);

    DeleteDC(hdc);
}

START_TEST(mapping)
{
    test_world_transform();
    test_isotropic_mapping();
    test_dword_forms();
    test_lptodp();
}
