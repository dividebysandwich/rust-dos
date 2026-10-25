/*
 * Unit tests for dc functions
 *
 * Copyright (c) 2005 Huw Davies
 * Copyright (c) 2005 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/dc.c: SaveDC and
 * RestoreDC on a display DC and on a window's DC, CreateCompatibleDC,
 * GetPixel on bitmaps selected into a memory DC, GetBoundsRect and
 * SetBoundsRect, and the display's COLORRES. Left out are
 * GdiConvertToDevmodeW and the Unicode DEVMODE, GetClipRgn's copy of the
 * clip region and test_DeleteDC, which checks with GetObjectType what
 * DeleteDC does to the DCs GetDC gives, all missing from Windows 3.1.
 * Windows 9x's answers are the ones Windows 3.1 gives. test_dc_attributes
 * isn't Wine's: it checks what SetBkColor, SetTextColor, SetBkMode,
 * SetROP2, SetPolyFillMode, SetStretchBltMode and SetTextAlign return,
 * and that SaveDC and RestoreDC keep them.
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

static void test_savedc_2(void)
{
    HWND hwnd;
    HDC hdc;
    RECT rc, rc_clip;
    int ret;

    hwnd = CreateWindow("static", "", WS_POPUP, 0,0,100,100,
                        0, 0, 0, NULL);
    ok(hwnd != 0, "CreateWindow failed\n");
    if (!hwnd) return;
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);

    hdc = GetDC(hwnd);
    ok(hdc != NULL, "GetDC failed\n");

    ret = GetClipBox(hdc, &rc_clip);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d instead of SIMPLEREGION\n", ret);
    SetRect(&rc, 0, 0, 100, 100);
    ok(EqualRect(&rc, &rc_clip),
       "rects are not equal: (%d,%d-%d,%d) - (%d,%d-%d,%d)\n",
       rc.left, rc.top, rc.right, rc.bottom,
       rc_clip.left, rc_clip.top, rc_clip.right, rc_clip.bottom);

    ret = SaveDC(hdc);
    ok(ret == 1, "ret = %d\n", ret);

    ret = IntersectClipRect(hdc, 0, 0, 50, 50);
    ok(ret == SIMPLEREGION, "IntersectClipRect returned %d instead of SIMPLEREGION\n", ret);

    ret = GetClipBox(hdc, &rc_clip);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d instead of SIMPLEREGION\n", ret);
    SetRect(&rc, 0, 0, 50, 50);
    ok(EqualRect(&rc, &rc_clip), "rects are not equal\n");

    ret = RestoreDC(hdc, 1);
    ok(ret, "ret = %d\n", ret);

    ret = GetClipBox(hdc, &rc_clip);
    ok(ret == SIMPLEREGION, "GetClipBox returned %d instead of SIMPLEREGION\n", ret);
    SetRect(&rc, 0, 0, 100, 100);
    ok(EqualRect(&rc, &rc_clip), "rects are not equal\n");

    ReleaseDC(hwnd, hdc);
    DestroyWindow(hwnd);
}

static void test_savedc(void)
{
    HDC hdc = CreateDC("DISPLAY", NULL, NULL, NULL);
    int ret;

    ok(hdc != NULL, "CreateDC rets %04x\n", hdc);

    ret = SaveDC(hdc);
    ok(ret == 1, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 2, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 3, "ret = %d\n", ret);
    ret = RestoreDC(hdc, -1);
    ok(ret, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 3, "ret = %d\n", ret);
    ret = RestoreDC(hdc, 1);
    ok(ret, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 1, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 2, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 3, "ret = %d\n", ret);
    ret = RestoreDC(hdc, -2);
    ok(ret, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 2, "ret = %d\n", ret);
    ret = RestoreDC(hdc, -2);
    ok(ret, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 1, "ret = %d\n", ret);
    ret = SaveDC(hdc);
    ok(ret == 2, "ret = %d\n", ret);
    ret = RestoreDC(hdc, -4);
    ok(!ret, "ret = %d\n", ret);
    ret = RestoreDC(hdc, 3);
    ok(!ret, "ret = %d\n", ret);

    /* Under Win9x the following RestoreDC call succeeds and clears the save stack. */
    ret = RestoreDC(hdc, -3);
    ok(!ret ||
       broken(ret), /* Win9x */
       "ret = %d\n", ret);

    /* Trying to clear an empty save stack fails. */
    ret = RestoreDC(hdc, -3);
    ok(!ret, "ret = %d\n", ret);

    ret = SaveDC(hdc);
    ok(ret == 3 ||
       broken(ret == 1), /* Win9x */
       "ret = %d\n", ret);

    /* Under Win9x the following RestoreDC call succeeds and clears the save stack. */
    ret = RestoreDC(hdc, 0);
    ok(!ret ||
       broken(ret), /* Win9x */
       "ret = %d\n", ret);

    /* Trying to clear an empty save stack fails. */
    ret = RestoreDC(hdc, 0);
    ok(!ret, "ret = %d\n", ret);

    ret = RestoreDC(hdc, 1);
    ok(ret ||
       broken(!ret), /* Win9x */
       "ret = %d\n", ret);

    DeleteDC(hdc);
}

static void test_CreateCompatibleDC(void)
{
    BOOL bRet;
    HDC hDC;
    HDC hNewDC;

    /* Create a DC compatible with the screen */
    hDC = CreateCompatibleDC(NULL);
    ok(hDC != NULL, "CreateCompatibleDC returned %04x\n", hDC);

    /* Delete this DC, this should succeed */
    bRet = DeleteDC(hDC);
    ok(bRet == TRUE, "DeleteDC returned %u\n", bRet);

    /* Try to create a DC compatible to the deleted DC. This has to fail */
    hNewDC = CreateCompatibleDC(hDC);
    ok(hNewDC == NULL, "CreateCompatibleDC returned %04x\n", hNewDC);
}

static void test_DC_bitmap(void)
{
    HDC hdc, hdcmem;
    DWORD bits[64];
    HBITMAP hbmp, oldhbmp;
    COLORREF col;
    int i, bitspixel;

    /* fill bitmap data with b&w pattern */
    for( i = 0; i < 64; i++) bits[i] = i & 1 ? 0 : 0xffffff;

    hdc = GetDC(0);
    ok( hdc != NULL, "CreateDC rets %04x\n", hdc);
    bitspixel = GetDeviceCaps( hdc, BITSPIXEL);
    /* create a memory dc */
    hdcmem = CreateCompatibleDC( hdc);
    ok( hdcmem != NULL, "CreateCompatibleDC rets %04x\n", hdcmem);
    /* tests */
    /* test monochrome bitmap: should always work */
    hbmp = CreateBitmap(32, 32, 1, 1, bits);
    ok( hbmp != NULL, "CreateBitmap returns %04x\n", hbmp);
    oldhbmp = SelectObject( hdcmem, hbmp);
    ok( oldhbmp != NULL, "SelectObject returned NULL\n" ); /* a memdc always has a bitmap selected */
    col = GetPixel( hdcmem, 0, 0);
    ok( col == 0xffffff, "GetPixel returned %08lx, expected 00ffffff\n", col);
    col = GetPixel( hdcmem, 1, 1);
    ok( col == 0x000000, "GetPixel returned %08lx, expected 00000000\n", col);
    col = GetPixel( hdcmem, 100, 1);
    ok( col == CLR_INVALID, "GetPixel returned %08lx, expected ffffffff\n", col);
    SelectObject( hdcmem, oldhbmp);
    DeleteObject( hbmp);

    /* test with 2 bits color depth, not likely to succeed */
    hbmp = CreateBitmap(16, 16, 1, 2, bits);
    ok( hbmp != NULL, "CreateBitmap returns %04x\n", hbmp);
    oldhbmp = SelectObject( hdcmem, hbmp);
    if( bitspixel != 2)
        ok( !oldhbmp, "SelectObject of a bitmap with 2 bits/pixel should return  NULL\n");
    if( oldhbmp) SelectObject( hdcmem, oldhbmp);
    DeleteObject( hbmp);

    /* test with 16 bits color depth, might succeed */
    hbmp = CreateBitmap(6, 6, 1, 16, bits);
    ok( hbmp != NULL, "CreateBitmap returns %04x\n", hbmp);
    oldhbmp = SelectObject( hdcmem, hbmp);
    if( bitspixel == 16) {
        ok( oldhbmp != NULL, "SelectObject returned NULL\n" );
        col = GetPixel( hdcmem, 0, 0);
        ok( col == 0xffffff,
            "GetPixel of a bitmap with 16 bits/pixel returned %08lx, expected 00ffffff\n", col);
        col = GetPixel( hdcmem, 1, 1);
        ok( col == 0x000000,
            "GetPixel of a bitmap with 16 bits/pixel returned returned %08lx, expected 00000000\n", col);
    }
    if( oldhbmp) SelectObject( hdcmem, oldhbmp);
    DeleteObject( hbmp);

    /* test with 32 bits color depth, probably succeed */
    hbmp = CreateBitmap(4, 4, 1, 32, bits);
    ok( hbmp != NULL, "CreateBitmap returns %04x\n", hbmp);
    oldhbmp = SelectObject( hdcmem, hbmp);
    if( bitspixel == 32) {
        ok( oldhbmp != NULL, "SelectObject returned NULL\n" );
        col = GetPixel( hdcmem, 0, 0);
        ok( col == 0xffffff,
            "GetPixel of a bitmap with 32 bits/pixel returned %08lx, expected 00ffffff\n", col);
        col = GetPixel( hdcmem, 1, 1);
        ok( col == 0x000000,
            "GetPixel of a bitmap with 32 bits/pixel returned returned %08lx, expected 00000000\n", col);
    }
    if( oldhbmp) SelectObject( hdcmem, oldhbmp);
    DeleteObject( hbmp);
    DeleteDC( hdcmem );
    ReleaseDC( 0, hdc );
}

static void test_boundsrect_invalid(void)
{
    HDC hdc;
    RECT rect, expect;
    UINT ret;

    hdc = GetDC(NULL);
    ok(hdc != NULL, "GetDC failed\n");

    ret = GetBoundsRect(hdc, NULL, 0);
    ok(ret == 0 ||
       broken(ret == DCB_RESET), /* Win9x */
       "Expected GetBoundsRect to return 0, got %u\n", ret);

    ret = GetBoundsRect(hdc, NULL, ~0U);
    ok(ret == 0 ||
       broken(ret == DCB_RESET), /* Win9x */
       "Expected GetBoundsRect to return 0, got %u\n", ret);

    if (GetBoundsRect(hdc, NULL, 0) == DCB_RESET)
        win_skip("Win9x fails catastrophically with first GetBoundsRect call\n");
    else
    {
        /* Test parameter handling order. */
        SetRect(&rect, 0, 0, 50, 50);
        ret = SetBoundsRect(hdc, &rect, DCB_SET);
        ok(ret & DCB_RESET,
           "Expected return flag DCB_RESET to be set, got %u\n", ret);

        ret = GetBoundsRect(hdc, NULL, DCB_RESET);
        ok(ret == 0,
           "Expected GetBoundsRect to return 0, got %u\n", ret);

        ret = GetBoundsRect(hdc, &rect, 0);
        ok(ret == DCB_RESET,
           "Expected GetBoundsRect to return DCB_RESET, got %u\n", ret);
        SetRect(&expect, 0, 0, 0, 0);
        ok(EqualRect(&rect, &expect),
           "Expected output rectangle (0,0)-(0,0), got (%d,%d)-(%d,%d)\n",
           rect.left, rect.top, rect.right, rect.bottom);
    }

    if (GetBoundsRect(hdc, NULL, 0) == DCB_RESET)
        win_skip("Win9x fails catastrophically with NULL device context parameter\n");
    else
    {
        ret = GetBoundsRect(NULL, NULL, 0);
        ok(ret == 0, "Expected GetBoundsRect to return 0, got %u\n", ret);

        ret = GetBoundsRect(NULL, NULL, ~0U);
        ok(ret == 0, "Expected GetBoundsRect to return 0, got %u\n", ret);

        ret = SetBoundsRect(NULL, NULL, 0);
        ok(ret == 0, "Expected SetBoundsRect to return 0, got %u\n", ret);

        ret = SetBoundsRect(NULL, NULL, ~0U);
        ok(ret == 0, "Expected SetBoundsRect to return 0, got %u\n", ret);
    }

    ReleaseDC(NULL, hdc);
}

static void test_desktop_colorres(void)
{
    HDC hdc = GetDC(NULL);
    int bitspixel, colorres;

    bitspixel = GetDeviceCaps(hdc, BITSPIXEL);
    ok(bitspixel != 0, "Expected to get valid BITSPIXEL capability value\n");

    colorres = GetDeviceCaps(hdc, COLORRES);
    ok(colorres != 0 ||
       broken(colorres == 0), /* Win9x */
       "Expected to get valid COLORRES capability value\n");

    if (colorres)
    {
        switch (bitspixel)
        {
        case 8:
            ok(colorres == 18,
               "Expected COLORRES to be 18, got %d\n", colorres);
            break;
        case 16:
            ok(colorres == 16,
               "Expected COLORRES to be 16, got %d\n", colorres);
            break;
        case 24:
        case 32:
            ok(colorres == 24,
               "Expected COLORRES to be 24, got %d\n", bitspixel);
            break;
        default:
            ok(0, "Got unknown BITSPIXEL %d with COLORRES %d\n", bitspixel, colorres);
            break;
        }
    }

    ReleaseDC(NULL, hdc);
}

static void test_dc_attributes(void)
{
    HDC hdc = CreateCompatibleDC(0);
    COLORREF c;
    int ret, saved;
    UINT align;

    trace("display: %d x %d, %d bits, %d planes, %d dpi, technology %d\n",
          GetDeviceCaps(hdc, HORZRES), GetDeviceCaps(hdc, VERTRES),
          GetDeviceCaps(hdc, BITSPIXEL), GetDeviceCaps(hdc, PLANES),
          GetDeviceCaps(hdc, LOGPIXELSX), GetDeviceCaps(hdc, TECHNOLOGY));
    ok(GetDeviceCaps(hdc, TECHNOLOGY) == DT_RASDISPLAY, "TECHNOLOGY is %d\n",
       GetDeviceCaps(hdc, TECHNOLOGY));
    ok(GetDeviceCaps(hdc, HORZRES) > 0 && GetDeviceCaps(hdc, VERTRES) > 0,
       "no resolution\n");
    ok(GetDeviceCaps(hdc, PLANES) == 1 || GetDeviceCaps(hdc, PLANES) == 4,
       "PLANES is %d\n", GetDeviceCaps(hdc, PLANES));
    ok(GetDeviceCaps(hdc, RASTERCAPS) & RC_BITBLT, "no RC_BITBLT in RASTERCAPS\n");

    /* The defaults */
    c = GetBkColor(hdc);
    ok(c == RGB(255,255,255), "background colour %08lx\n", c);
    c = GetTextColor(hdc);
    ok(c == RGB(0,0,0), "text colour %08lx\n", c);
    ret = GetBkMode(hdc);
    ok(ret == OPAQUE, "background mode %d\n", ret);
    ret = GetROP2(hdc);
    ok(ret == R2_COPYPEN, "ROP2 %d\n", ret);
    ret = GetPolyFillMode(hdc);
    ok(ret == ALTERNATE, "polygon fill mode %d\n", ret);
    ret = GetStretchBltMode(hdc);
    ok(ret == BLACKONWHITE, "stretch mode %d\n", ret);
    align = GetTextAlign(hdc);
    ok(align == (TA_LEFT|TA_TOP|TA_NOUPDATECP), "text align %u\n", align);
    ret = GetMapMode(hdc);
    ok(ret == MM_TEXT, "map mode %d\n", ret);

    /* What the Set functions return is the old value */
    c = SetBkColor(hdc, RGB(0x12,0x34,0x56));
    ok(c == RGB(255,255,255), "SetBkColor returned %08lx\n", c);
    c = SetBkColor(hdc, RGB(0x65,0x43,0x21));
    ok(c == RGB(0x12,0x34,0x56), "SetBkColor returned %08lx\n", c);
    c = SetTextColor(hdc, RGB(0xab,0xcd,0xef));
    ok(c == RGB(0,0,0), "SetTextColor returned %08lx\n", c);
    c = SetTextColor(hdc, RGB(1,2,3));
    ok(c == RGB(0xab,0xcd,0xef), "SetTextColor returned %08lx\n", c);
    ret = SetBkMode(hdc, TRANSPARENT);
    ok(ret == OPAQUE, "SetBkMode returned %d\n", ret);
    ret = SetROP2(hdc, R2_XORPEN);
    ok(ret == R2_COPYPEN, "SetROP2 returned %d\n", ret);
    ret = SetPolyFillMode(hdc, WINDING);
    ok(ret == ALTERNATE, "SetPolyFillMode returned %d\n", ret);
    ret = SetStretchBltMode(hdc, COLORONCOLOR);
    ok(ret == BLACKONWHITE, "SetStretchBltMode returned %d\n", ret);
    align = SetTextAlign(hdc, TA_RIGHT|TA_BASELINE);
    ok(align == (TA_LEFT|TA_TOP|TA_NOUPDATECP), "SetTextAlign returned %u\n", align);

    saved = SaveDC(hdc);
    ok(saved == 1, "SaveDC returned %d\n", saved);
    SetBkColor(hdc, RGB(9,9,9));
    SetTextColor(hdc, RGB(8,8,8));
    SetBkMode(hdc, OPAQUE);
    SetROP2(hdc, R2_NOT);
    SetPolyFillMode(hdc, ALTERNATE);
    SetTextAlign(hdc, TA_CENTER);
    SetMapMode(hdc, MM_LOMETRIC);
    ok(RestoreDC(hdc, saved), "RestoreDC failed\n");

    c = GetBkColor(hdc);
    ok(c == RGB(0x65,0x43,0x21), "restored background colour %08lx\n", c);
    c = GetTextColor(hdc);
    ok(c == RGB(1,2,3), "restored text colour %08lx\n", c);
    ret = GetBkMode(hdc);
    ok(ret == TRANSPARENT, "restored background mode %d\n", ret);
    ret = GetROP2(hdc);
    ok(ret == R2_XORPEN, "restored ROP2 %d\n", ret);
    ret = GetPolyFillMode(hdc);
    ok(ret == WINDING, "restored polygon fill mode %d\n", ret);
    align = GetTextAlign(hdc);
    ok(align == (TA_RIGHT|TA_BASELINE), "restored text align %u\n", align);
    ret = GetMapMode(hdc);
    ok(ret == MM_TEXT, "restored map mode %d\n", ret);

    DeleteDC(hdc);
}

START_TEST(dc)
{
    test_savedc();
    test_savedc_2();
    test_CreateCompatibleDC();
    test_DC_bitmap();
    test_boundsrect_invalid();
    test_desktop_colorres();
    test_dc_attributes();
}
