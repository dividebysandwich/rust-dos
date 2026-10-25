/*
 * Unit test suite for pens
 *
 * Copyright 2006 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/pen.c: the cosmetic pens
 * of CreatePen and CreatePenIndirect, and what GetObject gives for them
 * in Windows 3.1's LOGPEN (a 16-bit style and 16-bit widths). Left out
 * are ExtCreatePen, EXTLOGPEN, GetObjectType, PS_USERSTYLE and
 * PS_ALTERNATE, which Windows 3.1 doesn't have, and with them the
 * PS_ALTERNATE and PS_USERSTYLE drawing tests, and GetObject's size
 * query with no buffer, which Windows 3.1 doesn't document;
 * GetLastError is left out of the messages. Windows 9x's answers are
 * the ones Windows 3.1 gives: CreatePenIndirect may keep the width's
 * y, and GetObject copies as much of a LOGPEN as the buffer holds.
 * test_pen_drawing isn't Wine's: it checks with GetPixel the lines
 * that MoveTo and LineTo, Polyline, Polygon and Rectangle draw with
 * the pens, and what MoveTo and GetCurrentPosition return.
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

static void test_logpen(void)
{
    static const struct
    {
        UINT style;
        int width;
        COLORREF color;
        UINT ret_style;
        int ret_width;
        COLORREF ret_color;
    } pen[] = {
        { PS_SOLID, -123, RGB(0x12,0x34,0x56), PS_SOLID, 123, RGB(0x12,0x34,0x56) },
        { PS_SOLID, 0, RGB(0x12,0x34,0x56), PS_SOLID, 0, RGB(0x12,0x34,0x56) },
        { PS_SOLID, 123, RGB(0x12,0x34,0x56), PS_SOLID, 123, RGB(0x12,0x34,0x56) },
        { PS_DASH, 123, RGB(0x12,0x34,0x56), PS_DASH, 123, RGB(0x12,0x34,0x56) },
        { PS_DOT, 123, RGB(0x12,0x34,0x56), PS_DOT, 123, RGB(0x12,0x34,0x56) },
        { PS_DASHDOT, 123, RGB(0x12,0x34,0x56), PS_DASHDOT, 123, RGB(0x12,0x34,0x56) },
        { PS_DASHDOTDOT, 123, RGB(0x12,0x34,0x56), PS_DASHDOTDOT, 123, RGB(0x12,0x34,0x56) },
        { PS_NULL, -123, RGB(0x12,0x34,0x56), PS_NULL, 1, 0 },
        { PS_NULL, 123, RGB(0x12,0x34,0x56), PS_NULL, 1, 0 },
        { PS_INSIDEFRAME, 123, RGB(0x12,0x34,0x56), PS_INSIDEFRAME, 123, RGB(0x12,0x34,0x56) }
    };
    int i, size;
    HPEN hpen;
    LOGPEN lp;
    char big[sizeof(LOGPEN) * 4];

    for (i = 0; i < sizeof(pen)/sizeof(pen[0]); i++)
    {
        trace("%d: testing style %u\n", i, pen[i].style);

        /********************** cosmetic pens **********************/
        /* CreatePenIndirect behaviour */
        lp.lopnStyle = pen[i].style,
        lp.lopnWidth.x = pen[i].width;
        lp.lopnWidth.y = 11; /* just in case */
        lp.lopnColor = pen[i].color;
        hpen = CreatePenIndirect(&lp);
        ok(hpen != 0, "CreatePen failed\n");

        memset(&lp, 0xb0, sizeof(lp));
        size = GetObject(hpen, sizeof(lp), &lp);
        ok(size == sizeof(lp), "GetObject returned %d\n", size);

        ok(lp.lopnStyle == pen[i].ret_style, "expected %u, got %u\n", pen[i].ret_style, lp.lopnStyle);
        ok(lp.lopnWidth.x == pen[i].ret_width, "expected %u, got %d\n", pen[i].ret_width, lp.lopnWidth.x);
        ok(lp.lopnWidth.y == 0 ||
            broken(lp.lopnWidth.y == 0xb), /* Win9x */
            "expected 0, got %d\n", lp.lopnWidth.y);
        ok(lp.lopnColor == pen[i].ret_color, "expected %08lx, got %08lx\n", pen[i].ret_color, lp.lopnColor);

        DeleteObject(hpen);

        /* CreatePen behaviour */
        hpen = CreatePen(pen[i].style, pen[i].width, pen[i].color);
        ok(hpen != 0, "CreatePen failed\n");

        /* ask for truncated data */
        memset(&lp, 0xb0, sizeof(lp));
        size = GetObject(hpen, sizeof(lp.lopnStyle), &lp);
        ok(!size ||
            broken(size == sizeof(lp.lopnStyle)), /* Win9x */
            "GetObject should fail: size %d\n", size);

        /* see how larger buffer sizes are handled */
        memset(big, 0xb0, sizeof(big));
        size = GetObject(hpen, sizeof(big), big);
        ok(size == sizeof(lp), "GetObject returned %d\n", size);

        memset(&lp, 0xb0, sizeof(lp));
        size = GetObject(hpen, sizeof(lp), &lp);
        ok(size == sizeof(lp), "GetObject returned %d\n", size);

        ok(lp.lopnStyle == pen[i].ret_style, "expected %u, got %u\n", pen[i].ret_style, lp.lopnStyle);
        ok(lp.lopnWidth.x == pen[i].ret_width, "expected %u, got %d\n", pen[i].ret_width, lp.lopnWidth.x);
        ok(lp.lopnWidth.y == 0, "expected 0, got %d\n", lp.lopnWidth.y);
        ok(lp.lopnColor == pen[i].ret_color, "expected %08lx, got %08lx\n", pen[i].ret_color, lp.lopnColor);

        DeleteObject(hpen);
    }
}

#define expect_pixel(_hdc, _x, _y, _c) \
{ \
    COLORREF _got = GetPixel(_hdc, _x, _y); \
    ok(_got == (_c), "pixel (%d,%d) is %08lx, expected %08lx\n", _x, _y, _got, (COLORREF)(_c)); \
}

#define BLACK RGB(0,0,0)
#define WHITE RGB(255,255,255)

static void test_pen_drawing(void)
{
    static POINT line[] = { {2,5}, {10,5}, {10,12} };
    static POINT triangle[] = { {15,15}, {25,15}, {15,25} };
    static POINT shifted[] = { {0,28}, {4,28} };
    HDC hdc = CreateCompatibleDC(0);
    HBITMAP bmp = CreateBitmap(32, 32, 1, 1, NULL);
    HBITMAP oldbmp = SelectObject(hdc, bmp);
    HPEN pen, oldpen;
    HBRUSH oldbrush;
    DWORD dw;
    POINT pt;
    int x;

    PatBlt(hdc, 0, 0, 32, 32, WHITENESS);

    /* MoveTo and LineTo: the last point isn't drawn */
    dw = MoveTo(hdc, 2, 2);
    ok(dw == 0, "MoveTo gave back %08lx\n", dw);
    ok(LineTo(hdc, 10, 2), "LineTo failed\n");
    for (x = 2; x < 10; x++)
        expect_pixel(hdc, x, 2, BLACK);
    expect_pixel(hdc, 10, 2, WHITE);
    expect_pixel(hdc, 1, 2, WHITE);
    expect_pixel(hdc, 5, 3, WHITE);
    dw = GetCurrentPosition(hdc);
    ok(LOWORD(dw) == 10 && HIWORD(dw) == 2, "position after LineTo %08lx\n", dw);

    /* Polyline doesn't use or change the current position */
    ok(Polyline(hdc, line, 3), "Polyline failed\n");
    expect_pixel(hdc, 2, 5, BLACK);
    expect_pixel(hdc, 9, 5, BLACK);
    expect_pixel(hdc, 10, 5, BLACK);
    expect_pixel(hdc, 10, 8, BLACK);
    expect_pixel(hdc, 10, 11, BLACK);
    expect_pixel(hdc, 10, 12, WHITE);
    expect_pixel(hdc, 6, 8, WHITE);
    ok(GetCurrentPositionEx(hdc, &pt) && pt.x == 10 && pt.y == 2,
       "position after Polyline (%d,%d)\n", pt.x, pt.y);

    /* Polygon fills with the brush and outlines with the pen */
    oldbrush = SelectObject(hdc, GetStockObject(BLACK_BRUSH));
    ok(Polygon(hdc, triangle, 3), "Polygon failed\n");
    expect_pixel(hdc, 15, 15, BLACK);
    expect_pixel(hdc, 17, 17, BLACK);
    expect_pixel(hdc, 20, 15, BLACK);
    expect_pixel(hdc, 15, 22, BLACK);
    expect_pixel(hdc, 24, 24, WHITE);
    expect_pixel(hdc, 26, 15, WHITE);
    expect_pixel(hdc, 14, 20, WHITE);

    /* Rectangle with a null brush: only the outline, inside the
     * rectangle's right and bottom edges */
    SelectObject(hdc, GetStockObject(NULL_BRUSH));
    ok(Rectangle(hdc, 20, 2, 30, 8), "Rectangle failed\n");
    expect_pixel(hdc, 20, 2, BLACK);
    expect_pixel(hdc, 29, 2, BLACK);
    expect_pixel(hdc, 29, 7, BLACK);
    expect_pixel(hdc, 20, 7, BLACK);
    expect_pixel(hdc, 25, 5, WHITE);
    expect_pixel(hdc, 30, 2, WHITE);
    expect_pixel(hdc, 25, 8, WHITE);

    /* A wide pen */
    pen = CreatePen(PS_SOLID, 3, BLACK);
    oldpen = SelectObject(hdc, pen);
    MoveTo(hdc, 2, 20);
    LineTo(hdc, 10, 20);
    expect_pixel(hdc, 5, 19, BLACK);
    expect_pixel(hdc, 5, 20, BLACK);
    expect_pixel(hdc, 5, 21, BLACK);
    expect_pixel(hdc, 5, 23, WHITE);
    expect_pixel(hdc, 5, 17, WHITE);
    SelectObject(hdc, oldpen);
    DeleteObject(pen);

    /* The points are logical: a viewport origin moves them */
    SetViewportOrg(hdc, 5, 0);
    ok(Polyline(hdc, shifted, 2), "Polyline failed\n");
    SetViewportOrg(hdc, 0, 0);
    expect_pixel(hdc, 4, 28, WHITE);
    expect_pixel(hdc, 5, 28, BLACK);
    expect_pixel(hdc, 8, 28, BLACK);
    expect_pixel(hdc, 9, 28, WHITE);

    /* A null pen draws nothing */
    PatBlt(hdc, 0, 0, 32, 32, WHITENESS);
    oldpen = SelectObject(hdc, GetStockObject(NULL_PEN));
    MoveTo(hdc, 2, 2);
    LineTo(hdc, 10, 2);
    expect_pixel(hdc, 5, 2, WHITE);
    SelectObject(hdc, oldpen);

    SelectObject(hdc, oldbrush);
    SelectObject(hdc, oldbmp);
    DeleteObject(bmp);
    DeleteDC(hdc);
}

START_TEST(pen)
{
    test_logpen();
    test_pen_drawing();
}
