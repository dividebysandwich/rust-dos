/*
 * Unit test suite for bitmaps
 *
 * Copyright 2004 Huw Davies
 * Copyright 2006 Dmitry Timoshkov
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

#include <stdarg.h>
#include <assert.h>
#include <string.h>

#include "windef.h"
#include "winbase.h"
#include "winerror.h"
#include "wingdi.h"
#include "winuser.h"
#include "mmsystem.h"

#include "wine/test.h"

static BOOL (WINAPI *pGdiAlphaBlend)(HDC,int,int,int,int,HDC,int,int,int,int,BLENDFUNCTION);
static BOOL (WINAPI *pGdiGradientFill)(HDC,TRIVERTEX*,ULONG,void*,ULONG,ULONG);

static void test_GdiAlphaBlend(void)
{
    HDC hdcNull;
    HDC hdcDst;
    HBITMAP bmpDst;
    HBITMAP oldDst;
    BITMAPINFO *bmi;
    HDC hdcSrc;
    HBITMAP bmpSrc;
    HBITMAP oldSrc;
    LPVOID bits;
    BOOL ret;
    BLENDFUNCTION blend;

    if (!pGdiAlphaBlend)
    {
        win_skip("GdiAlphaBlend() is not implemented\n");
        return;
    }

    hdcNull = GetDC(NULL);
    hdcDst = CreateCompatibleDC(hdcNull);
    bmpDst = CreateCompatibleBitmap(hdcNull, 100, 100);
    hdcSrc = CreateCompatibleDC(hdcNull);

    bmi = HeapAlloc( GetProcessHeap(), HEAP_ZERO_MEMORY, FIELD_OFFSET( BITMAPINFO, bmiColors[3] ));
    bmi->bmiHeader.biSize = sizeof(bmi->bmiHeader);
    bmi->bmiHeader.biHeight = 20;
    bmi->bmiHeader.biWidth = 20;
    bmi->bmiHeader.biBitCount = 32;
    bmi->bmiHeader.biPlanes = 1;
    bmi->bmiHeader.biCompression = BI_RGB;
    bmpSrc = CreateDIBSection(hdcDst, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");

    oldDst = SelectObject(hdcDst, bmpDst);
    oldSrc = SelectObject(hdcSrc, bmpSrc);

    blend.BlendOp = AC_SRC_OVER;
    blend.BlendFlags = 0;
    blend.SourceConstantAlpha = 128;
    blend.AlphaFormat = 0;

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -1, 0, 10, 10, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );

    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, -1, 10, 10, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 15, 0, 10, 10, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 10, 10, -2, 3, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 10, 10, -2, 3, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );

    SetWindowOrgEx(hdcSrc, -10, -10, NULL);
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -1, 0, 10, 10, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, -1, 10, 10, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
    SetMapMode(hdcSrc, MM_ANISOTROPIC);
    ScaleWindowExtEx(hdcSrc, 10, 1, 10, 1, NULL);
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -1, 0, 30, 30, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, -1, 30, 30, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );

    SetMapMode(hdcDst, MM_ANISOTROPIC);
    SetViewportExtEx(hdcDst, -1, -1, NULL);
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, -1, 50, 50, blend);
    ok( ret || broken(!ret), "GdiAlphaBlend failed err %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, -20, -20, 20, 20, hdcSrc, 0, -1, 50, 50, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, -20, -20, -20, -20, hdcSrc, 0, -1, 50, 50, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, -20, 0, -20, 20, hdcSrc, 0, -1, 50, 50, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, -20, 20, -20, hdcSrc, 0, -1, 50, 50, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetMapMode(hdcDst, MM_TEXT);

    SetViewportExtEx(hdcSrc, -1, -1, NULL);
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -20, -20, -30, -30, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -20, -20, 30, -30, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -20, -20, -30, 30, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -20, -20, 30, 30, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 20, 20, 30, 30, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, -60, -60, 30, 30, blend);
    ok( !ret || broken(ret), "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetViewportExtEx(hdcSrc, 1, 1, NULL);

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, NULL, 0, 0, 20, 20, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == 0xdeadbeef, "wrong error %u\n", GetLastError() );

    /* overlapping source and dest not allowed */

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcDst, 19, 19, 20, 20, blend);
    ok( !ret || broken(ret), "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 20, 20, 20, 20, hdcDst, 1, 1, 20, 20, blend);
    ok( !ret || broken(ret), "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcDst, 20, 10, 20, 20, blend);
    ok( ret, "GdiAlphaBlend succeeded\n" );
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcDst, 10, 20, 20, 20, blend);
    ok( ret, "GdiAlphaBlend succeeded\n" );

    /* AC_SRC_ALPHA requires 32-bpp BI_RGB format */

    blend.AlphaFormat = AC_SRC_ALPHA;
    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );

    bmi->bmiHeader.biCompression = BI_BITFIELDS;
    ((DWORD *)bmi->bmiColors)[0] = 0xff0000;
    ((DWORD *)bmi->bmiColors)[1] = 0x00ff00;
    ((DWORD *)bmi->bmiColors)[2] = 0x0000ff;
    bmpSrc = CreateDIBSection(hdcDst, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");
    oldSrc = SelectObject(hdcSrc, bmpSrc);
    DeleteObject( oldSrc );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );

    bmi->bmiHeader.biCompression = BI_BITFIELDS;
    ((DWORD *)bmi->bmiColors)[0] = 0x0000ff;
    ((DWORD *)bmi->bmiColors)[1] = 0x00ff00;
    ((DWORD *)bmi->bmiColors)[2] = 0xff0000;
    bmpSrc = CreateDIBSection(hdcDst, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok(bmpSrc != NULL || broken(bmpSrc == NULL), "Couldn't create source bitmap\n");
    oldSrc = SelectObject(hdcSrc, bmpSrc);
    DeleteObject( oldSrc );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( !ret || broken(ret), "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );

    bmi->bmiHeader.biBitCount = 24;
    bmi->bmiHeader.biCompression = BI_RGB;
    bmpSrc = CreateDIBSection(hdcDst, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");
    oldSrc = SelectObject(hdcSrc, bmpSrc);
    DeleteObject( oldSrc );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );

    bmi->bmiHeader.biBitCount = 1;
    bmpSrc = CreateDIBSection(hdcDst, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");
    oldSrc = SelectObject(hdcSrc, bmpSrc);
    DeleteObject( oldSrc );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );

    bmpSrc = CreateBitmap( 100, 100, 1, 1, NULL );
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");
    oldSrc = SelectObject(hdcSrc, bmpSrc);
    DeleteObject( oldSrc );

    SetLastError(0xdeadbeef);
    ret = pGdiAlphaBlend(hdcDst, 0, 0, 20, 20, hdcSrc, 0, 0, 20, 20, blend);
    ok( !ret, "GdiAlphaBlend succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );

    SelectObject(hdcDst, oldDst);
    SelectObject(hdcSrc, oldSrc);
    DeleteObject(bmpSrc);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);
    DeleteDC(hdcSrc);

    ReleaseDC(NULL, hdcNull);

}

static void test_GdiGradientFill(void)
{
    HDC hdc;
    BOOL ret;
    HBITMAP bmp;
    BITMAPINFO *bmi;
    void *bits;
    GRADIENT_RECT rect[] = { { 0, 0 }, { 0, 1 }, { 2, 3 } };
    GRADIENT_TRIANGLE tri[] = { { 0, 0, 0 }, { 0, 1, 2 }, { 0, 2, 1 }, { 0, 1, 3 } };
    TRIVERTEX vt[3] = { { 2,  2,  0xff00, 0x0000, 0x0000, 0x8000 },
                        { 10, 10, 0x0000, 0xff00, 0x0000, 0x8000 },
                        { 20, 10, 0x0000, 0x0000, 0xff00, 0xff00 } };

    if (!pGdiGradientFill)
    {
        win_skip( "GdiGradientFill is not implemented\n" );
        return;
    }

    hdc = CreateCompatibleDC( NULL );
    bmi = HeapAlloc( GetProcessHeap(), HEAP_ZERO_MEMORY, FIELD_OFFSET( BITMAPINFO, bmiColors[3] ));
    bmi->bmiHeader.biSize = sizeof(bmi->bmiHeader);
    bmi->bmiHeader.biHeight = 20;
    bmi->bmiHeader.biWidth = 20;
    bmi->bmiHeader.biBitCount = 32;
    bmi->bmiHeader.biPlanes = 1;
    bmi->bmiHeader.biCompression = BI_RGB;
    bmp = CreateDIBSection(hdc, bmi, DIB_RGB_COLORS, &bits, NULL, 0);
    ok( bmp != NULL, "couldn't create bitmap\n" );
    SelectObject( hdc, bmp );

    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, rect, 1, GRADIENT_FILL_RECT_H );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, rect, 1, 3 );
    ok( !ret, "GdiGradientFill succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( (HDC)0xdead, vt, 3, rect, 1, GRADIENT_FILL_RECT_H );
    ok( !ret, "GdiGradientFill succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    /* rust-dos: Windows 98's GradientFill reads the vertices and the
       mesh through the NULL pointers, and its program dies. */
    if (GetVersion() & 0x80000000)
        win_skip( "GradientFill with NULL pointers\n" );
    else
    {
        SetLastError( 0xdeadbeef );
        ret = pGdiGradientFill( NULL, NULL, 0, rect, 1, GRADIENT_FILL_RECT_H );
        ok( !ret, "GdiGradientFill succeeded\n" );
        ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
        ret = pGdiGradientFill( hdc, NULL, 0, rect, 1, GRADIENT_FILL_RECT_H );
        ok( !ret, "GdiGradientFill succeeded\n" );
        ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
        SetLastError( 0xdeadbeef );
        ret = pGdiGradientFill( hdc, NULL, 3, rect, 1, GRADIENT_FILL_RECT_H );
        ok( !ret, "GdiGradientFill succeeded\n" );
        ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
        SetLastError( 0xdeadbeef );
        ret = pGdiGradientFill( hdc, vt, 3, NULL, 0, GRADIENT_FILL_RECT_H );
        ok( !ret, "GdiGradientFill succeeded\n" );
        ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
        SetLastError( 0xdeadbeef );
        ret = pGdiGradientFill( hdc, vt, 3, NULL, 1, GRADIENT_FILL_RECT_H );
        ok( !ret, "GdiGradientFill succeeded\n" );
        ok( GetLastError() == ERROR_INVALID_PARAMETER, "wrong error %u\n", GetLastError() );
    }
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, rect, 0, GRADIENT_FILL_RECT_H );
    ok( !ret || broken(ret), "GdiGradientFill succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, rect, 3, GRADIENT_FILL_RECT_H );
    ok( !ret, "GdiGradientFill succeeded\n" );
    ok( GetLastError() == 0xdeadbeef, "wrong error %u\n", GetLastError() );
    rect[2].UpperLeft = rect[2].LowerRight = 1;
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, rect, 3, GRADIENT_FILL_RECT_H );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 1, rect, 1, GRADIENT_FILL_RECT_H );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 1, tri, 0, GRADIENT_FILL_TRIANGLE );
    ok( !ret || broken(ret), "GdiGradientFill succeeded\n" );
    ok( GetLastError() == ERROR_INVALID_PARAMETER ||
        broken(GetLastError() == 0xdeadbeef), "wrong error %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 1, tri, 1, GRADIENT_FILL_TRIANGLE );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 2, GRADIENT_FILL_TRIANGLE );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 3, GRADIENT_FILL_TRIANGLE );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 4, GRADIENT_FILL_TRIANGLE );
    ok( !ret || broken(ret), "GdiGradientFill succeeded\n" );
    ok( GetLastError() == 0xdeadbeef, "wrong error %u\n", GetLastError() );
    tri[3].Vertex3 = 1;
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 4, GRADIENT_FILL_TRIANGLE );
    ok( !ret, "GdiGradientFill succeeded\n" );
    ok( GetLastError() == 0xdeadbeef, "wrong error %u\n", GetLastError() );
    tri[3].Vertex3 = 0;
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 4, GRADIENT_FILL_TRIANGLE );
    ok( !ret, "GdiGradientFill succeeded\n" );
    ok( GetLastError() == 0xdeadbeef, "wrong error %u\n", GetLastError() );
    tri[3].Vertex1 = tri[3].Vertex2 = tri[3].Vertex3 = 1;
    SetLastError( 0xdeadbeef );
    ret = pGdiGradientFill( hdc, vt, 3, tri, 4, GRADIENT_FILL_TRIANGLE );
    ok( ret, "GdiGradientFill failed err %u\n", GetLastError() );

    DeleteDC( hdc );
    DeleteObject( bmp );
}

static void test_32bit_ddb(void)
{
    char buffer[sizeof(BITMAPINFOHEADER) + sizeof(DWORD)];
    BITMAPINFO *biDst = (BITMAPINFO *)buffer;
    HBITMAP bmpSrc, bmpDst;
    HBITMAP oldSrc, oldDst;
    HDC hdcSrc, hdcDst, hdcScreen;
    HBRUSH brush;
    DWORD *dstBuffer, *data;
    DWORD colorSrc = 0x40201008;

    memset(biDst, 0, sizeof(BITMAPINFOHEADER));
    biDst->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    biDst->bmiHeader.biWidth = 1;
    biDst->bmiHeader.biHeight = -1;
    biDst->bmiHeader.biPlanes = 1;
    biDst->bmiHeader.biBitCount = 32;
    biDst->bmiHeader.biCompression = BI_RGB;

    hdcScreen = CreateCompatibleDC(0);
    if(GetDeviceCaps(hdcScreen, BITSPIXEL) != 32)
    {
        DeleteDC(hdcScreen);
        trace("Skipping 32-bit DDB test\n");
        return;
    }

    hdcSrc = CreateCompatibleDC(hdcScreen);
    bmpSrc = CreateBitmap(1, 1, 1, 32, &colorSrc);
    oldSrc = SelectObject(hdcSrc, bmpSrc);

    hdcDst = CreateCompatibleDC(hdcScreen);
    bmpDst = CreateDIBSection(hdcDst, biDst, DIB_RGB_COLORS, (void**)&dstBuffer, NULL, 0);
    oldDst = SelectObject(hdcDst, bmpDst);

    StretchBlt(hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, 1, 1, SRCCOPY);
    ok(dstBuffer[0] == colorSrc, "Expected color=%x, received color=%x\n", colorSrc, dstBuffer[0]);

    if (pGdiAlphaBlend)
    {
        BLENDFUNCTION blend;
        BOOL ret;

        blend.BlendOp = AC_SRC_OVER;
        blend.BlendFlags = 0;
        blend.SourceConstantAlpha = 128;
        blend.AlphaFormat = 0;
        dstBuffer[0] = 0x80808080;
        ret = pGdiAlphaBlend( hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, 1, 1, blend );
        ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
        ok(dstBuffer[0] == 0x60504844, "wrong color %x\n", dstBuffer[0]);
        blend.AlphaFormat = AC_SRC_ALPHA;
        dstBuffer[0] = 0x80808080;
        ret = pGdiAlphaBlend( hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, 1, 1, blend );
        ok( ret, "GdiAlphaBlend failed err %u\n", GetLastError() );
        ok(dstBuffer[0] == 0x90807874, "wrong color %x\n", dstBuffer[0]);
    }

    data = (DWORD *)biDst->bmiColors;
    data[0] = 0x20304050;
    brush = CreateDIBPatternBrushPt( biDst, DIB_RGB_COLORS );
    ok( brush != 0, "brush creation failed\n" );
    SelectObject( hdcSrc, brush );
    PatBlt( hdcSrc, 0, 0, 1, 1, PATCOPY );
    BitBlt( hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, SRCCOPY );
    ok(dstBuffer[0] == data[0], "Expected color=%x, received color=%x\n", data[0], dstBuffer[0]);
    SelectObject( hdcSrc, GetStockObject(BLACK_BRUSH) );
    DeleteObject( brush );

    biDst->bmiHeader.biBitCount = 24;
    brush = CreateDIBPatternBrushPt( biDst, DIB_RGB_COLORS );
    ok( brush != 0, "brush creation failed\n" );
    SelectObject( hdcSrc, brush );
    PatBlt( hdcSrc, 0, 0, 1, 1, PATCOPY );
    BitBlt( hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, SRCCOPY );
    ok(dstBuffer[0] == (data[0] & ~0xff000000),
       "Expected color=%x, received color=%x\n", data[0] & 0xff000000, dstBuffer[0]);
    SelectObject( hdcSrc, GetStockObject(BLACK_BRUSH) );
    DeleteObject( brush );

    /* Tidy up */
    SelectObject(hdcDst, oldDst);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);

    SelectObject(hdcSrc, oldSrc);
    DeleteObject(bmpSrc);
    DeleteDC(hdcSrc);

    DeleteDC(hdcScreen);
}

START_TEST(bitmap)
{
    HMODULE hdll;

    hdll = GetModuleHandle("gdi32.dll");
    pGdiAlphaBlend   = (void*)GetProcAddress(hdll, "GdiAlphaBlend");
    pGdiGradientFill = (void*)GetProcAddress(hdll, "GdiGradientFill");
    /* rust-dos: Windows 98's GDI32 has no Gdi* functions; MSIMG32's
       AlphaBlend and GradientFill are them. */
    hdll = LoadLibraryA("msimg32.dll");
    if (!pGdiAlphaBlend && hdll)
        pGdiAlphaBlend = (void*)GetProcAddress(hdll, "AlphaBlend");
    if (!pGdiGradientFill && hdll)
        pGdiGradientFill = (void*)GetProcAddress(hdll, "GradientFill");

    test_GdiAlphaBlend();
    test_GdiGradientFill();
    test_32bit_ddb();
}
