/*
 * Unit test suite for bitmaps
 *
 * Copyright 2004 Huw Davies
 * Copyright 2006 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/bitmap.c: CreateDIBitmap
 * and the device-dependent bitmaps it makes, CreateBitmap,
 * CreateBitmapIndirect, CreateCompatibleBitmap and GetObject's BITMAP
 * (16-bit sizes in Windows 3.1), GetBitmapBits, GetDIBits and SetDIBits
 * with 1-bit and 24-bit DIBs, SelectObject of bitmaps, BitBlt, StretchBlt
 * and StretchDIBits with each raster operation, and BitBlt with an empty
 * clip region. Windows 3.1 has no DIB sections, so the tests of them
 * (test_dibsections, test_mono_dibsection, test_GetDIBits_selected_DIB,
 * test_bitmapinfoheadersize and test_GetDIBits_BI_BITFIELDS) are left
 * out, as are GdiAlphaBlend, GetObjectType, GetCurrentObject,
 * SetLastError and the bitmaps wider than Windows 3.1's 16-bit sizes.
 * Where Wine's tests read and write the pixels of DIB sections, these
 * read and write those of 32-bit device-dependent bitmaps with
 * GetBitmapBits and SetBitmapBits, so they skip on other displays, and
 * compare only the colour, not the byte above it; the source DIBs that
 * Wine's tests make 32-bit and top-down are 24-bit and bottom-up, as
 * Windows 3.1 has no other, and of StretchDIBits's tests only those whose
 * answer doesn't depend on that are kept. The DIBs that GetDIBits makes of
 * the display's bitmaps are 24-bit where Wine's are the display's depth.
 * The checks of GetObject and GetBitmapBits with no buffer, which Windows
 * 3.1 doesn't document, are left out. Windows 9x's answers are the ones
 * Windows 3.1 gives. test_dib_formats isn't Wine's: it checks the colours
 * that SetDIBits, CreateDIBitmap, SetDIBitsToDevice and StretchDIBits
 * draw from 1, 4, 8 and 24-bit DIBs, and that GetDIBits reads back.
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

static int BITMAP_GetWidthBytes( int bmWidth, int bpp )
{
    switch(bpp)
    {
    case 1:
        return 2 * ((bmWidth+15) >> 4);

    case 24:
        bmWidth *= 3; /* fall through */
    case 8:
        return bmWidth + (bmWidth & 1);

    case 32:
        return bmWidth * 4;

    case 16:
    case 15:
        return bmWidth * 2;

    case 4:
        return 2 * ((bmWidth+3) >> 2);

    default:
        trace("Unknown depth %d, please report.\n", bpp );
    }
    return -1;
}

static BYTE buf[1024], buf_cmp[1024];

static void test_bitmap_info(HBITMAP hbm, int expected_depth, const BITMAPINFOHEADER *bmih)
{
    BITMAP bm;
    BITMAP bma[2];
    int ret, width_bytes;
    long lret;

    ret = GetObject(hbm, sizeof(bm), &bm);
    ok(ret == sizeof(bm), "GetObject returned %d\n", ret);

    ok(bm.bmType == 0 || broken(bm.bmType == 21072 /* Win9x */), "wrong bm.bmType %d\n", bm.bmType);
    ok(bm.bmWidth == bmih->biWidth, "wrong bm.bmWidth %d\n", bm.bmWidth);
    ok(bm.bmHeight == bmih->biHeight, "wrong bm.bmHeight %d\n", bm.bmHeight);
    width_bytes = BITMAP_GetWidthBytes(bm.bmWidth, bm.bmBitsPixel);
    ok(bm.bmWidthBytes == width_bytes, "wrong bm.bmWidthBytes %d != %d\n", bm.bmWidthBytes, width_bytes);
    ok(bm.bmPlanes == bmih->biPlanes, "wrong bm.bmPlanes %d\n", bm.bmPlanes);
    ok(bm.bmBitsPixel == expected_depth, "wrong bm.bmBitsPixel %d != %d\n", bm.bmBitsPixel, expected_depth);
    ok(bm.bmBits == NULL, "wrong bm.bmBits %08lx\n", (DWORD)bm.bmBits);

    if (sizeof(buf) < (long)bm.bmWidthBytes * bm.bmHeight)
    {
        skip("the bitmap is larger than the buffer\n");
        return;
    }

    memset(buf_cmp, 0xAA, sizeof(buf_cmp));
    memset(buf_cmp, 0, bm.bmWidthBytes * bm.bmHeight);

    memset(buf, 0xAA, sizeof(buf));
    lret = GetBitmapBits(hbm, sizeof(buf), buf);
    ok(lret == (long)bm.bmWidthBytes * bm.bmHeight, "%ld != %d\n", lret, bm.bmWidthBytes * bm.bmHeight);
    ok(!memcmp(buf, buf_cmp, sizeof(buf)) ||
       broken(memcmp(buf, buf_cmp, sizeof(buf))), /* win9x doesn't init the bitmap bits */
        "buffers do not match, depth %d\n", bmih->biBitCount);

    /* test various buffer sizes for GetObject */
    ret = GetObject(hbm, sizeof(*bma) * 2, bma);
    ok(ret == sizeof(*bma) || broken(ret == sizeof(*bma) * 2 /* Win9x */), "wrong size %d\n", ret);

    ret = GetObject(hbm, sizeof(bm) / 2, &bm);
    ok(ret == 0 || broken(ret == sizeof(bm) / 2 /* Win9x */), "%d != 0\n", ret);

    ret = GetObject(hbm, 0, &bm);
    ok(ret == 0, "%d != 0\n", ret);

    ret = GetObject(hbm, 1, &bm);
    ok(ret == 0 || broken(ret == 1 /* Win9x */), "%d != 0\n", ret);
}

static void test_createdibitmap(void)
{
    HDC hdc, hdcmem;
    BITMAPINFOHEADER bmih;
    BITMAPINFO bm;
    HBITMAP hbm, hbm_colour, hbm_old;
    int screen_depth;
    DWORD pixel;

    hdc = GetDC(0);
    screen_depth = GetDeviceCaps(hdc, BITSPIXEL);
    memset(&bmih, 0, sizeof(bmih));
    bmih.biSize = sizeof(bmih);
    bmih.biWidth = 10;
    bmih.biHeight = 10;
    bmih.biPlanes = 1;
    bmih.biBitCount = 32;
    bmih.biCompression = BI_RGB;

    hbm = CreateDIBitmap(hdc, NULL, CBM_INIT, NULL, NULL, 0);
    ok(hbm == NULL, "CreateDIBitmap should fail\n");
    hbm = CreateDIBitmap(hdc, NULL, 0, NULL, NULL, 0);
    ok(hbm == NULL, "CreateDIBitmap should fail\n");

    /* First create an un-initialised bitmap.  The depth of the bitmap
       should match that of the hdc and not that supplied in bmih.
    */

    /* First try 32 bits */
    hbm = CreateDIBitmap(hdc, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    /* Then 16 */
    bmih.biBitCount = 16;
    hbm = CreateDIBitmap(hdc, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    /* Then 1 */
    bmih.biBitCount = 1;
    hbm = CreateDIBitmap(hdc, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    /* Now with a monochrome dc we expect a monochrome bitmap */
    hdcmem = CreateCompatibleDC(hdc);

    /* First try 32 bits */
    bmih.biBitCount = 32;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, 1, &bmih);
    DeleteObject(hbm);

    /* Then 16 */
    bmih.biBitCount = 16;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, 1, &bmih);
    DeleteObject(hbm);

    /* Then 1 */
    bmih.biBitCount = 1;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, 1, &bmih);
    DeleteObject(hbm);

    /* Now select a polychrome bitmap into the dc and we expect
       screen_depth bitmaps again */
    hbm_colour = CreateCompatibleBitmap(hdc, (int)bmih.biWidth, (int)bmih.biHeight);
    test_bitmap_info(hbm_colour, screen_depth, &bmih);
    hbm_old = SelectObject(hdcmem, hbm_colour);

    /* First try 32 bits */
    bmih.biBitCount = 32;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    /* Then 16 */
    bmih.biBitCount = 16;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    /* Then 1 */
    bmih.biBitCount = 1;
    hbm = CreateDIBitmap(hdcmem, &bmih, 0, NULL, NULL, 0);
    ok(hbm != NULL, "CreateDIBitmap failed\n");
    test_bitmap_info(hbm, screen_depth, &bmih);
    DeleteObject(hbm);

    SelectObject(hdcmem, hbm_old);
    DeleteObject(hbm_colour);
    DeleteDC(hdcmem);

    /* Test how formats are converted */
    pixel = 0xffffffffL;
    bmih.biBitCount = 1;
    bmih.biWidth = 1;
    bmih.biHeight = 1;

    memset(&bm, 0, sizeof(bm));
    bm.bmiHeader.biSize = sizeof(bm.bmiHeader);
    bm.bmiHeader.biWidth = 1;
    bm.bmiHeader.biHeight = 1;
    bm.bmiHeader.biPlanes = 1;
    bm.bmiHeader.biBitCount= 24;
    bm.bmiHeader.biCompression= BI_RGB;
    bm.bmiHeader.biSizeImage = 0;
    hbm = CreateDIBitmap(hdc, &bmih, CBM_INIT, &pixel, &bm, DIB_RGB_COLORS);
    ok(hbm != NULL, "CreateDIBitmap failed\n");

    pixel = 0xdeadbeefL;
    GetDIBits(hdc, hbm, 0, 1, &pixel, &bm, DIB_RGB_COLORS);
    ok((pixel & 0xffffffL) == 0x00ffffffL, "Reading a 24 bit pixel from a DDB returned %08lx\n", pixel);
    DeleteObject(hbm);

    ReleaseDC(0, hdc);
}

static void test_bitmap(void)
{
    static char buf[256], buf_cmp[256];
    HBITMAP hbmp, hbmp_old;
    HDC hdc;
    BITMAP bm;
    BITMAP bma[2];
    int ret;
    long lret;

    hdc = CreateCompatibleDC(0);
    ok(hdc != 0, "CreateCompatibleDC failed\n");

    hbmp = CreateBitmap(15, 15, 1, 1, NULL);
    ok(hbmp != NULL, "CreateBitmap failed\n");

    ret = GetObject(hbmp, sizeof(bm), &bm);
    ok(ret == sizeof(bm), "wrong size %d\n", ret);

    ok(bm.bmType == 0, "wrong bm.bmType %d\n", bm.bmType);
    ok(bm.bmWidth == 15, "wrong bm.bmWidth %d\n", bm.bmWidth);
    ok(bm.bmHeight == 15, "wrong bm.bmHeight %d\n", bm.bmHeight);
    ok(bm.bmWidthBytes == 2, "wrong bm.bmWidthBytes %d\n", bm.bmWidthBytes);
    ok(bm.bmPlanes == 1, "wrong bm.bmPlanes %d\n", bm.bmPlanes);
    ok(bm.bmBitsPixel == 1, "wrong bm.bmBitsPixel %d\n", bm.bmBitsPixel);
    ok(bm.bmBits == NULL, "wrong bm.bmBits %08lx\n", (DWORD)bm.bmBits);

    memset(buf_cmp, 0xAA, sizeof(buf_cmp));
    memset(buf_cmp, 0, bm.bmWidthBytes * bm.bmHeight);

    memset(buf, 0xAA, sizeof(buf));
    lret = GetBitmapBits(hbmp, sizeof(buf), buf);
    ok(lret == (long)bm.bmWidthBytes * bm.bmHeight, "%ld != %d\n", lret, bm.bmWidthBytes * bm.bmHeight);
    ok(!memcmp(buf, buf_cmp, sizeof(buf)) ||
       broken(memcmp(buf, buf_cmp, sizeof(buf))), /* win9x doesn't init the bitmap bits */
       "buffers do not match\n");

    hbmp_old = SelectObject(hdc, hbmp);

    ret = GetObject(hbmp, sizeof(bm), &bm);
    ok(ret == sizeof(bm), "wrong size %d\n", ret);

    ok(bm.bmType == 0, "wrong bm.bmType %d\n", bm.bmType);
    ok(bm.bmWidth == 15, "wrong bm.bmWidth %d\n", bm.bmWidth);
    ok(bm.bmHeight == 15, "wrong bm.bmHeight %d\n", bm.bmHeight);
    ok(bm.bmWidthBytes == 2, "wrong bm.bmWidthBytes %d\n", bm.bmWidthBytes);
    ok(bm.bmPlanes == 1, "wrong bm.bmPlanes %d\n", bm.bmPlanes);
    ok(bm.bmBitsPixel == 1, "wrong bm.bmBitsPixel %d\n", bm.bmBitsPixel);
    ok(bm.bmBits == NULL, "wrong bm.bmBits %08lx\n", (DWORD)bm.bmBits);

    memset(buf, 0xAA, sizeof(buf));
    lret = GetBitmapBits(hbmp, sizeof(buf), buf);
    ok(lret == (long)bm.bmWidthBytes * bm.bmHeight, "%ld != %d\n", lret, bm.bmWidthBytes * bm.bmHeight);
    ok(!memcmp(buf, buf_cmp, sizeof(buf)) ||
       broken(memcmp(buf, buf_cmp, sizeof(buf))), /* win9x doesn't init the bitmap bits */
       "buffers do not match\n");

    hbmp_old = SelectObject(hdc, hbmp_old);
    ok(hbmp_old == hbmp, "wrong old bitmap %04x\n", hbmp_old);

    /* test various buffer sizes for GetObject */
    ret = GetObject(hbmp, sizeof(*bma) * 2, bma);
    ok(ret == sizeof(*bma) || broken(ret == sizeof(*bma) * 2 /* Win9x */), "wrong size %d\n", ret);

    ret = GetObject(hbmp, sizeof(bm) / 2, &bm);
    ok(ret == 0 || broken(ret == sizeof(bm) / 2 /* Win9x */), "%d != 0\n", ret);

    ret = GetObject(hbmp, 0, &bm);
    ok(ret == 0, "%d != 0\n", ret);

    ret = GetObject(hbmp, 1, &bm);
    ok(ret == 0 || broken(ret == 1 /* Win9x */), "%d != 0\n", ret);

    DeleteObject(hbmp);
    DeleteDC(hdc);
}

static void test_bmBits(void)
{
    BYTE bits[4];
    HBITMAP hbmp;
    BITMAP bmp;

    memset(bits, 0, sizeof(bits));
    hbmp = CreateBitmap(2, 2, 1, 4, bits);
    ok(hbmp != NULL, "CreateBitmap failed\n");

    memset(&bmp, 0xFF, sizeof(bmp));
    ok(GetObject(hbmp, sizeof(bmp), &bmp) == sizeof(bmp),
       "GetObject failed or returned a wrong structure size\n");
    ok(!bmp.bmBits, "bmBits must be NULL for device-dependent bitmaps\n");

    DeleteObject(hbmp);
}

static char dd_info_buf[sizeof(BITMAPINFOHEADER) + 256 * sizeof(RGBQUAD)];
static char dd_info2_buf[sizeof(BITMAPINFOHEADER) + 256 * sizeof(RGBQUAD)];
static BYTE dd_bits[16 * 16 * 4], dd_bits2[16 * 16 * 4];

static void test_GetDIBits_selected_DDB(BOOL monochrome)
{
    HBITMAP ddb;
    BITMAPINFO * info = (BITMAPINFO *)dd_info_buf;
    BITMAPINFO * info2 = (BITMAPINFO *)dd_info2_buf;
    BYTE * bits = dd_bits;
    BYTE * bits2 = dd_bits2;
    HDC ddb_dc, dc;
    HBITMAP old_bmp;
    BOOL equalContents;
    UINT width, height;
    UINT bpp;
    UINT i, j;
    int res;

    width = height = 16;

    /* Create a DDB (device-dependent bitmap) */
    if (monochrome)
    {
        bpp = 1;
        ddb = CreateBitmap(width, height, 1, 1, NULL);
    }
    else
    {
        HDC screen_dc = GetDC(NULL);
        bpp = GetDeviceCaps(screen_dc, BITSPIXEL) * GetDeviceCaps(screen_dc, PLANES);
        /* DIBs of more than 8 bits are 24-bit ones in Windows 3.1 */
        if (bpp > 8) bpp = 24;
        ddb = CreateCompatibleBitmap(screen_dc, width, height);
        ReleaseDC(NULL, screen_dc);
    }

    /* Set the pixels */
    ddb_dc = CreateCompatibleDC(NULL);
    old_bmp = SelectObject(ddb_dc, ddb);
    for (i = 0; i < width; i++)
    {
        for (j=0; j < height; j++)
        {
            BYTE c = (BYTE)((i * width + j) % 256);
            SetPixel(ddb_dc, i, j, RGB(c, c, c));
        }
    }
    SelectObject(ddb_dc, old_bmp);

    memset(dd_info_buf, 0, sizeof(dd_info_buf));
    memset(dd_info2_buf, 0, sizeof(dd_info2_buf));
    memset(dd_bits, 0, sizeof(dd_bits));
    memset(dd_bits2, 0, sizeof(dd_bits2));

    info->bmiHeader.biSize = sizeof(info->bmiHeader);
    info->bmiHeader.biWidth = width;
    info->bmiHeader.biHeight = height;
    info->bmiHeader.biPlanes = 1;
    info->bmiHeader.biBitCount = bpp;
    info->bmiHeader.biCompression = BI_RGB;

    dc = CreateCompatibleDC(NULL);

    /* Fill in biSizeImage */
    GetDIBits(dc, ddb, 0, height, NULL, info, DIB_RGB_COLORS);
    ok(info->bmiHeader.biSizeImage != 0, "GetDIBits failed to get the DIB attributes\n");
    ok(info->bmiHeader.biSizeImage <= sizeof(dd_bits), "biSizeImage is %lu\n",
       info->bmiHeader.biSizeImage);
    if (info->bmiHeader.biSizeImage > sizeof(dd_bits))
        info->bmiHeader.biSizeImage = sizeof(dd_bits);

    /* Get the bits */
    res = GetDIBits(dc, ddb, 0, height, bits, info, DIB_RGB_COLORS);
    ok(res, "GetDIBits failed\n");

    /* Copy the DIB attributes but not the color table */
    memcpy(info2, info, sizeof(BITMAPINFOHEADER));

    /* Select the DDB into another DC */
    old_bmp = SelectObject(ddb_dc, ddb);

    /* Get the bits */
    res = GetDIBits(dc, ddb, 0, height, bits2, info2, DIB_RGB_COLORS);
    ok(res, "GetDIBits failed\n");

    /* Compare the color table and the bits */
    if (bpp <= 8)
    {
        equalContents = TRUE;
        for (i=0; i < (1u << bpp); i++)
        {
            if ((info->bmiColors[i].rgbRed != info2->bmiColors[i].rgbRed)
                || (info->bmiColors[i].rgbGreen != info2->bmiColors[i].rgbGreen)
                || (info->bmiColors[i].rgbBlue != info2->bmiColors[i].rgbBlue)
                || (info->bmiColors[i].rgbReserved != info2->bmiColors[i].rgbReserved))
            {
                equalContents = FALSE;
                break;
            }
        }
        ok(equalContents, "GetDIBits with DDB selected in DC: Got a different color table\n");
    }

    equalContents = !memcmp(bits, bits2, (size_t)info->bmiHeader.biSizeImage);
    ok(equalContents, "GetDIBits with DDB selected in DC: Got different DIB bits\n");

    /* Test the palette */
    equalContents = TRUE;
    if (info2->bmiHeader.biBitCount <= 8)
    {
        WORD *colors = (WORD*)info2->bmiColors;

        /* Get the palette indices */
        res = GetDIBits(dc, ddb, 0, 0, NULL, info2, DIB_PAL_COLORS);
        if (res == 0) /* Win9x */
            res = GetDIBits(dc, ddb, 0, height, NULL, info2, DIB_PAL_COLORS);
        ok(res, "GetDIBits failed\n");

        for (i=0;i < (1u << info2->bmiHeader.biBitCount); i++)
        {
            if (colors[i] != i)
            {
                equalContents = FALSE;
                break;
            }
        }
    }

    ok(equalContents, "GetDIBits with DDB selected in DC: non 1:1 palette indices\n");

    DeleteDC(dc);

    SelectObject(ddb_dc, old_bmp);
    DeleteDC(ddb_dc);
    DeleteObject(ddb);
}

/* 2-bytes aligned 1-bit bitmap data: 16x16 */
static const BYTE bmp_bits_1[16 * 2] =
{
    0xff,0xff, 0,0, 0xff,0xff, 0,0,
    0xff,0xff, 0,0, 0xff,0xff, 0,0,
    0xff,0xff, 0,0, 0xff,0xff, 0,0,
    0xff,0xff, 0,0, 0xff,0xff, 0,0
};
/* 4-bytes aligned 1-bit DIB data: 16x16 */
static const BYTE dib_bits_1[16 * 4] =
{
    0,0,0,0, 0xff,0xff,0,0, 0,0,0,0, 0xff,0xff,0,0,
    0,0,0,0, 0xff,0xff,0,0, 0,0,0,0, 0xff,0xff,0,0,
    0,0,0,0, 0xff,0xff,0,0, 0,0,0,0, 0xff,0xff,0,0,
    0,0,0,0, 0xff,0xff,0,0, 0,0,0,0, 0xff,0xff,0,0
};
static const BYTE dib_bits_1_9x[16 * 4] =
{
    0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa, 0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa,
    0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa, 0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa,
    0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa, 0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa,
    0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa, 0,0,0xaa,0xaa, 0xff,0xff,0xaa,0xaa
};
/* 4-bytes aligned 24-bit DIB data: 16x16, a black row, then a white
 * one, from the bottom up */
static BYTE dib_bits_24[16 * 16*3];

static void init_dib_bits_24(void)
{
    int row;
    for (row = 0; row < 16; row++)
        memset(dib_bits_24 + row * 48, (row & 1) ? 0xff : 0, 48);
}

static char gd_bi_buf[sizeof(BITMAPINFOHEADER) + sizeof(RGBQUAD) * 256];

static void test_GetDIBits(void)
{
    HBITMAP hbmp;
    BITMAP bm;
    HDC hdc;
    int i, lines;
    long bytes;
    BITMAPINFO *bi = (BITMAPINFO *)gd_bi_buf;

    init_dib_bits_24();
    hdc = GetDC(0);

    /* 1-bit source bitmap data */
    hbmp = CreateBitmap(16, 16, 1, 1, bmp_bits_1);
    ok(hbmp != 0, "CreateBitmap failed\n");

    memset(&bm, 0xAA, sizeof(bm));
    bytes = GetObject(hbmp, sizeof(bm), &bm);
    ok(bytes == sizeof(bm), "GetObject returned %ld\n", bytes);
    ok(bm.bmType == 0, "wrong bmType %d\n", bm.bmType);
    ok(bm.bmWidth == 16, "wrong bmWidth %d\n", bm.bmWidth);
    ok(bm.bmHeight == 16, "wrong bmHeight %d\n", bm.bmHeight);
    ok(bm.bmWidthBytes == 2, "wrong bmWidthBytes %d\n", bm.bmWidthBytes);
    ok(bm.bmPlanes == 1, "wrong bmPlanes %u\n", bm.bmPlanes);
    ok(bm.bmBitsPixel == 1, "wrong bmBitsPixel %d\n", bm.bmBitsPixel);
    ok(!bm.bmBits, "wrong bmBits %08lx\n", (DWORD)bm.bmBits);

    bytes = GetBitmapBits(hbmp, sizeof(buf), buf);
    ok(bytes == sizeof(bmp_bits_1), "expected 16*2 got %ld bytes\n", bytes);
    ok(!memcmp(buf, bmp_bits_1, sizeof(bmp_bits_1)), "bitmap bits don't match\n");

    /* retrieve 1-bit DIB data */
    memset(bi, 0, sizeof(*bi));
    bi->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi->bmiHeader.biWidth = bm.bmWidth;
    bi->bmiHeader.biHeight = bm.bmHeight;
    bi->bmiHeader.biPlanes = 1;
    bi->bmiHeader.biBitCount = 1;
    bi->bmiHeader.biCompression = BI_RGB;
    bi->bmiHeader.biSizeImage = 0;
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    lines = GetDIBits(0, hbmp, 0, bm.bmHeight, buf, bi, DIB_RGB_COLORS);
    ok(lines == 0, "GetDIBits copied %d lines with hdc = 0\n", lines);
    ok(bi->bmiHeader.biSizeImage == 0, "expected 0, got %lu\n", bi->bmiHeader.biSizeImage);

    memset(buf, 0xAA, sizeof(buf));
    lines = GetDIBits(hdc, hbmp, 0, bm.bmHeight, buf, bi, DIB_RGB_COLORS);
    ok(lines == bm.bmHeight, "GetDIBits copied %d lines of %d\n",
       lines, bm.bmHeight);
    ok(bi->bmiHeader.biSizeImage == sizeof(dib_bits_1) ||
       broken(bi->bmiHeader.biSizeImage == 0), /* win9x */
       "expected 16*4, got %lu\n", bi->bmiHeader.biSizeImage);

    /* the color table consists of black and white */
    ok(bi->bmiColors[0].rgbRed == 0 && bi->bmiColors[0].rgbGreen == 0 &&
       bi->bmiColors[0].rgbBlue == 0 && bi->bmiColors[0].rgbReserved == 0,
       "expected bmiColors[0] 0,0,0,0 - got %x %x %x %x\n",
       bi->bmiColors[0].rgbRed, bi->bmiColors[0].rgbGreen,
       bi->bmiColors[0].rgbBlue, bi->bmiColors[0].rgbReserved);
    ok(bi->bmiColors[1].rgbRed == 0xff && bi->bmiColors[1].rgbGreen == 0xff &&
       bi->bmiColors[1].rgbBlue == 0xff && bi->bmiColors[1].rgbReserved == 0,
       "expected bmiColors[0] 0xff,0xff,0xff,0 - got %x %x %x %x\n",
       bi->bmiColors[1].rgbRed, bi->bmiColors[1].rgbGreen,
       bi->bmiColors[1].rgbBlue, bi->bmiColors[1].rgbReserved);
    for (i = 2; i < 256; i++)
    {
        ok(bi->bmiColors[i].rgbRed == 0xAA && bi->bmiColors[i].rgbGreen == 0xAA &&
           bi->bmiColors[i].rgbBlue == 0xAA && bi->bmiColors[i].rgbReserved == 0xAA,
           "expected bmiColors[%d] 0xAA,0xAA,0xAA,0xAA - got %x %x %x %x\n", i,
           bi->bmiColors[i].rgbRed, bi->bmiColors[i].rgbGreen,
           bi->bmiColors[i].rgbBlue, bi->bmiColors[i].rgbReserved);
    }

    /* returned bits are DWORD aligned and upside down */
    ok(!memcmp(buf, dib_bits_1, sizeof(dib_bits_1)) ||
       broken(!memcmp(buf, dib_bits_1_9x, sizeof(dib_bits_1_9x))), /* Win9x, WinME */
       "DIB bits don't match\n");

    /* Test the palette indices */
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    lines = GetDIBits(hdc, hbmp, 0, 0, NULL, bi, DIB_PAL_COLORS);
    if (lines == 0)
        win_skip("Win9x/WinMe doesn't handle 0 for the number of scan lines\n");
    else
    {
        ok(((WORD*)bi->bmiColors)[0] == 0, "Color 0 is %d\n", ((WORD*)bi->bmiColors)[0]);
        ok(((WORD*)bi->bmiColors)[1] == 1, "Color 1 is %d\n", ((WORD*)bi->bmiColors)[1]);
    }
    for (i = 2; i < 256; i++)
        ok(((WORD*)bi->bmiColors)[i] == 0xAAAA, "Color %d is %d\n", i, ((WORD*)bi->bmiColors)[i]);

    /* retrieve 24-bit DIB data */
    memset(bi, 0, sizeof(*bi));
    bi->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi->bmiHeader.biWidth = bm.bmWidth;
    bi->bmiHeader.biHeight = bm.bmHeight;
    bi->bmiHeader.biPlanes = 1;
    bi->bmiHeader.biBitCount = 24;
    bi->bmiHeader.biCompression = BI_RGB;
    bi->bmiHeader.biSizeImage = 0;
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    memset(buf, 0xAA, sizeof(buf));
    lines = GetDIBits(hdc, hbmp, 0, bm.bmHeight, buf, bi, DIB_RGB_COLORS);
    ok(lines == bm.bmHeight, "GetDIBits copied %d lines of %d\n",
       lines, bm.bmHeight);
    ok(bi->bmiHeader.biSizeImage == sizeof(dib_bits_24) ||
       broken(bi->bmiHeader.biSizeImage == 0), /* win9x */
       "expected 16*16*3, got %lu\n", bi->bmiHeader.biSizeImage);

    /* the color table doesn't exist for 24-bit images */
    for (i = 0; i < 256; i++)
    {
        ok(bi->bmiColors[i].rgbRed == 0xAA && bi->bmiColors[i].rgbGreen == 0xAA &&
           bi->bmiColors[i].rgbBlue == 0xAA && bi->bmiColors[i].rgbReserved == 0xAA,
           "expected bmiColors[%d] 0xAA,0xAA,0xAA,0xAA - got %x %x %x %x\n", i,
           bi->bmiColors[i].rgbRed, bi->bmiColors[i].rgbGreen,
           bi->bmiColors[i].rgbBlue, bi->bmiColors[i].rgbReserved);
    }

    /* returned bits are DWORD aligned and upside down */
    ok(!memcmp(buf, dib_bits_24, sizeof(dib_bits_24)), "DIB bits don't match\n");
    DeleteObject(hbmp);

    /* 24-bit source bitmap data: bottom-up, as Windows 3.1 has them,
     * so the same rows as Wine's top-down data */
    hbmp = CreateCompatibleBitmap(hdc, 16, 16);
    ok(hbmp != 0, "CreateBitmap failed\n");
    lines = SetDIBits(hdc, hbmp, 0, bm.bmHeight, dib_bits_24, bi, DIB_RGB_COLORS);
    ok(lines == bm.bmHeight, "SetDIBits copied %d lines of %d\n",
       lines, bm.bmHeight);

    memset(&bm, 0xAA, sizeof(bm));
    bytes = GetObject(hbmp, sizeof(bm), &bm);
    ok(bytes == sizeof(bm), "GetObject returned %ld\n", bytes);
    ok(bm.bmType == 0 ||
       broken(bm.bmType == 21072), /* win9x */
       "wrong bmType %d\n", bm.bmType);
    ok(bm.bmWidth == 16, "wrong bmWidth %d\n", bm.bmWidth);
    ok(bm.bmHeight == 16, "wrong bmHeight %d\n", bm.bmHeight);
    ok(bm.bmWidthBytes == BITMAP_GetWidthBytes(bm.bmWidth, bm.bmBitsPixel), "wrong bmWidthBytes %d\n", bm.bmWidthBytes);
    ok(bm.bmPlanes == GetDeviceCaps(hdc, PLANES), "wrong bmPlanes %u\n", bm.bmPlanes);
    ok(bm.bmBitsPixel == GetDeviceCaps(hdc, BITSPIXEL), "wrong bmBitsPixel %d\n", bm.bmBitsPixel);
    ok(!bm.bmBits, "wrong bmBits %08lx\n", (DWORD)bm.bmBits);

    bytes = GetBitmapBits(hbmp, sizeof(buf), buf);
    ok(bytes == (long)bm.bmWidthBytes * bm.bmHeight, "expected %d got %ld bytes\n",
       bm.bmWidthBytes * bm.bmHeight, bytes);

    /* retrieve 1-bit DIB data */
    memset(bi, 0, sizeof(*bi));
    bi->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi->bmiHeader.biWidth = bm.bmWidth;
    bi->bmiHeader.biHeight = bm.bmHeight;
    bi->bmiHeader.biPlanes = 1;
    bi->bmiHeader.biBitCount = 1;
    bi->bmiHeader.biCompression = BI_RGB;
    bi->bmiHeader.biSizeImage = 0;
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    memset(buf, 0xAA, sizeof(buf));
    lines = GetDIBits(hdc, hbmp, 0, bm.bmHeight, buf, bi, DIB_RGB_COLORS);
    ok(lines == bm.bmHeight, "GetDIBits copied %d lines of %d\n",
       lines, bm.bmHeight);
    ok(bi->bmiHeader.biSizeImage == sizeof(dib_bits_1) ||
       broken(bi->bmiHeader.biSizeImage == 0), /* win9x */
       "expected 16*4, got %lu\n", bi->bmiHeader.biSizeImage);

    /* the color table consists of black and white */
    ok(bi->bmiColors[0].rgbRed == 0 && bi->bmiColors[0].rgbGreen == 0 &&
       bi->bmiColors[0].rgbBlue == 0 && bi->bmiColors[0].rgbReserved == 0,
       "expected bmiColors[0] 0,0,0,0 - got %x %x %x %x\n",
       bi->bmiColors[0].rgbRed, bi->bmiColors[0].rgbGreen,
       bi->bmiColors[0].rgbBlue, bi->bmiColors[0].rgbReserved);
    ok(bi->bmiColors[1].rgbRed == 0xff && bi->bmiColors[1].rgbGreen == 0xff &&
       bi->bmiColors[1].rgbBlue == 0xff && bi->bmiColors[1].rgbReserved == 0,
       "expected bmiColors[0] 0xff,0xff,0xff,0 - got %x %x %x %x\n",
       bi->bmiColors[1].rgbRed, bi->bmiColors[1].rgbGreen,
       bi->bmiColors[1].rgbBlue, bi->bmiColors[1].rgbReserved);
    for (i = 2; i < 256; i++)
    {
        ok(bi->bmiColors[i].rgbRed == 0xAA && bi->bmiColors[i].rgbGreen == 0xAA &&
           bi->bmiColors[i].rgbBlue == 0xAA && bi->bmiColors[i].rgbReserved == 0xAA,
           "expected bmiColors[%d] 0xAA,0xAA,0xAA,0xAA - got %x %x %x %x\n", i,
           bi->bmiColors[i].rgbRed, bi->bmiColors[i].rgbGreen,
           bi->bmiColors[i].rgbBlue, bi->bmiColors[i].rgbReserved);
    }

    /* returned bits are DWORD aligned and upside down */
    ok(!memcmp(buf, dib_bits_1, sizeof(dib_bits_1)) ||
       broken(!memcmp(buf, dib_bits_1_9x, sizeof(dib_bits_1_9x))), /* Win9x, WinME */
       "DIB bits don't match\n");

    /* Test the palette indices */
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    lines = GetDIBits(hdc, hbmp, 0, 0, NULL, bi, DIB_PAL_COLORS);
    if (lines == 0)
        win_skip("Win9x/WinMe doesn't handle 0 for the number of scan lines\n");
    else
    {
        ok(((WORD*)bi->bmiColors)[0] == 0, "Color 0 is %d\n", ((WORD*)bi->bmiColors)[0]);
        ok(((WORD*)bi->bmiColors)[1] == 1, "Color 1 is %d\n", ((WORD*)bi->bmiColors)[1]);
    }
    for (i = 2; i < 256; i++)
        ok(((WORD*)bi->bmiColors)[i] == 0xAAAA, "Color %d is %d\n", i, ((WORD*)bi->bmiColors)[i]);

    /* retrieve 24-bit DIB data */
    memset(bi, 0, sizeof(*bi));
    bi->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi->bmiHeader.biWidth = bm.bmWidth;
    bi->bmiHeader.biHeight = bm.bmHeight;
    bi->bmiHeader.biPlanes = 1;
    bi->bmiHeader.biBitCount = 24;
    bi->bmiHeader.biCompression = BI_RGB;
    bi->bmiHeader.biSizeImage = 0;
    memset(bi->bmiColors, 0xAA, sizeof(RGBQUAD) * 256);
    memset(buf, 0xAA, sizeof(buf));
    lines = GetDIBits(hdc, hbmp, 0, bm.bmHeight, buf, bi, DIB_RGB_COLORS);
    ok(lines == bm.bmHeight, "GetDIBits copied %d lines of %d\n",
       lines, bm.bmHeight);
    ok(bi->bmiHeader.biSizeImage == sizeof(dib_bits_24) ||
       broken(bi->bmiHeader.biSizeImage == 0), /* win9x */
       "expected 16*16*3, got %lu\n", bi->bmiHeader.biSizeImage);

    /* the color table doesn't exist for 24-bit images */
    for (i = 0; i < 256; i++)
    {
        ok(bi->bmiColors[i].rgbRed == 0xAA && bi->bmiColors[i].rgbGreen == 0xAA &&
           bi->bmiColors[i].rgbBlue == 0xAA && bi->bmiColors[i].rgbReserved == 0xAA,
           "expected bmiColors[%d] 0xAA,0xAA,0xAA,0xAA - got %x %x %x %x\n", i,
           bi->bmiColors[i].rgbRed, bi->bmiColors[i].rgbGreen,
           bi->bmiColors[i].rgbBlue, bi->bmiColors[i].rgbReserved);
    }

    /* returned bits are DWORD aligned and upside down */
    ok(!memcmp(buf, dib_bits_24, sizeof(dib_bits_24)), "DIB bits don't match\n");
    DeleteObject(hbmp);

    ReleaseDC(0, hdc);
}

static void test_select_object(void)
{
    HDC hdc;
    HBITMAP hbm, hbm_old;
    int planes, bpp, i;
    static const int depths[] = {8, 15, 16, 24, 32};
    BITMAP bm;
    int bytes;

    hdc = GetDC(0);
    ok(hdc != 0, "GetDC(0) failed\n");
    hbm = CreateCompatibleBitmap(hdc, 10, 10);
    ok(hbm != 0, "CreateCompatibleBitmap failed\n");

    hbm_old = SelectObject(hdc, hbm);
    ok(hbm_old == 0, "SelectObject should fail\n");

    DeleteObject(hbm);
    ReleaseDC(0, hdc);

    hdc = CreateCompatibleDC(0);
    ok(hdc != 0, "GetDC(0) failed\n");
    hbm = CreateCompatibleBitmap(hdc, 10, 10);
    ok(hbm != 0, "CreateCompatibleBitmap failed\n");

    hbm_old = SelectObject(hdc, hbm);
    ok(hbm_old != 0, "SelectObject failed\n");
    hbm_old = SelectObject(hdc, hbm_old);
    ok(hbm_old == hbm, "SelectObject failed\n");

    DeleteObject(hbm);

    /* test an 1-bpp bitmap */
    planes = GetDeviceCaps(hdc, PLANES);
    bpp = 1;

    hbm = CreateBitmap(10, 10, planes, bpp, NULL);
    ok(hbm != 0, "CreateBitmap failed\n");

    hbm_old = SelectObject(hdc, hbm);
    ok(hbm_old != 0, "SelectObject failed\n");
    hbm_old = SelectObject(hdc, hbm_old);
    ok(hbm_old == hbm, "SelectObject failed\n");

    DeleteObject(hbm);

    for(i = 0; i < sizeof(depths)/sizeof(depths[0]); i++) {
        /* test a color bitmap to dc bpp matching */
        planes = GetDeviceCaps(hdc, PLANES);
        bpp = GetDeviceCaps(hdc, BITSPIXEL);

        hbm = CreateBitmap(10, 10, planes, depths[i], NULL);
        ok(hbm != 0, "CreateBitmap failed\n");

        hbm_old = SelectObject(hdc, hbm);
        if(depths[i] == bpp ||
          (bpp == 16 && depths[i] == 15)        /* 16 and 15 bpp are compatible */
          ) {
            ok(hbm_old != 0, "SelectObject failed, BITSPIXEL: %d, created depth: %d\n", bpp, depths[i]);
            SelectObject(hdc, hbm_old);
        } else {
            ok(hbm_old == 0, "SelectObject should fail. BITSPIXELS: %d, created depth: %d\n", bpp, depths[i]);
        }

        memset(&bm, 0xAA, sizeof(bm));
        bytes = GetObject(hbm, sizeof(bm), &bm);
        ok(bytes == sizeof(bm), "GetObject returned %d\n", bytes);
        ok(bm.bmType == 0 ||
           broken(bm.bmType == 21072), /* win9x */
           "wrong bmType %d\n", bm.bmType);
        ok(bm.bmWidth == 10, "wrong bmWidth %d\n", bm.bmWidth);
        ok(bm.bmHeight == 10, "wrong bmHeight %d\n", bm.bmHeight);
        ok(bm.bmWidthBytes == BITMAP_GetWidthBytes(bm.bmWidth, bm.bmBitsPixel), "wrong bmWidthBytes %d\n", bm.bmWidthBytes);
        ok(bm.bmPlanes == planes, "wrong bmPlanes %u\n", bm.bmPlanes);
        if(depths[i] == 15) {
            ok(bm.bmBitsPixel == 16 ||
               broken(bm.bmBitsPixel == 15), /* Win9x/WinME */
               "wrong bmBitsPixel %d(15 bpp special)\n", bm.bmBitsPixel);
        } else {
            ok(bm.bmBitsPixel == depths[i], "wrong bmBitsPixel %d\n", bm.bmBitsPixel);
        }
        ok(!bm.bmBits, "wrong bmBits %08lx\n", (DWORD)bm.bmBits);

        DeleteObject(hbm);
    }

    DeleteDC(hdc);
}

static void test_mono_1x1_bmp_dbg(HBITMAP hbmp, int line)
{
    int ret;
    BITMAP bm;

    memset(&bm, 0xDA, sizeof(bm));
    ret = GetObject(hbmp, sizeof(bm), &bm);
    ok_(__FILE__, line)(ret == sizeof(BITMAP),
                        "GetObject returned %d\n", ret);
    ok_(__FILE__, line)(bm.bmType == 0, "wrong bmType, expected 0 got %d\n", bm.bmType);
    ok_(__FILE__, line)(bm.bmWidth == 1, "wrong bmWidth, expected 1 got %d\n", bm.bmWidth);
    ok_(__FILE__, line)(bm.bmHeight == 1, "wrong bmHeight, expected 1 got %d\n", bm.bmHeight);
    ok_(__FILE__, line)(bm.bmWidthBytes == 2, "wrong bmWidthBytes, expected 2 got %d\n", bm.bmWidthBytes);
    ok_(__FILE__, line)(bm.bmPlanes == 1, "wrong bmPlanes, expected 1 got %u\n", bm.bmPlanes);
    ok_(__FILE__, line)(bm.bmBitsPixel == 1, "wrong bmBitsPixel, expected 1 got %d\n", bm.bmBitsPixel);
    ok_(__FILE__, line)(!bm.bmBits, "wrong bmBits %08lx\n", (DWORD)bm.bmBits);
}

#define test_mono_1x1_bmp(a) test_mono_1x1_bmp_dbg((a), __LINE__)

static void test_CreateBitmap(void)
{
    BITMAP bmp;
    HDC screenDC = GetDC(0);
    HDC hdc = CreateCompatibleDC(screenDC);
    UINT i, expect = 0;

    /* all of these are the stock monochrome bitmap */
    HBITMAP bm = CreateCompatibleBitmap(hdc, 0, 0);
    HBITMAP bm1 = CreateCompatibleBitmap(screenDC, 0, 0);
    HBITMAP bm4 = CreateBitmap(0, 1, 0, 0, 0);
    HBITMAP bm5 = CreateDiscardableBitmap(hdc, 0, 0);

    /* these 2 are not the stock monochrome bitmap */
    HBITMAP bm2 = CreateCompatibleBitmap(hdc, 1, 1);
    HBITMAP bm3 = CreateBitmap(1, 1, 1, 1, 0);

    HBITMAP old1 = SelectObject(hdc, bm2);
    HBITMAP old2 = SelectObject(screenDC, bm3);
    SelectObject(hdc, old1);
    SelectObject(screenDC, old2);

    ok(bm == bm1 && bm == bm4 && bm == bm5 && bm == old1,
       "0: %04x, 1: %04x, 4: %04x, 5: %04x, old1 %04x\n",
       bm, bm1, bm4, bm5, old1);
    ok(bm != bm2 && bm != bm3, "0: %04x, 2: %04x, 3: %04x\n", bm, bm2, bm3);
    ok(old2 == 0, "old2 %04x\n", old2);

    test_mono_1x1_bmp(bm);
    test_mono_1x1_bmp(bm1);
    test_mono_1x1_bmp(bm2);
    test_mono_1x1_bmp(bm3);
    test_mono_1x1_bmp(bm4);
    test_mono_1x1_bmp(bm5);
    test_mono_1x1_bmp(old1);

    DeleteObject(bm);
    DeleteObject(bm1);
    DeleteObject(bm2);
    DeleteObject(bm3);
    DeleteObject(bm4);
    DeleteObject(bm5);

    DeleteDC(hdc);
    ReleaseDC(0, screenDC);

    /* show that Windows ignores the provided bm.bmWidthBytes */
    bmp.bmType = 0;
    bmp.bmWidth = 1;
    bmp.bmHeight = 1;
    bmp.bmWidthBytes = 28;
    bmp.bmPlanes = 1;
    bmp.bmBitsPixel = 1;
    bmp.bmBits = NULL;
    bm = CreateBitmapIndirect(&bmp);
    ok(bm != 0, "CreateBitmapIndirect failed\n");
    test_mono_1x1_bmp(bm);
    DeleteObject(bm);

    /* Test how the bmBitsPixel field is treated */
    for(i = 1; i <= 33; i++) {
        bmp.bmType = 0;
        bmp.bmWidth = 1;
        bmp.bmHeight = 1;
        bmp.bmWidthBytes = 28;
        bmp.bmPlanes = 1;
        bmp.bmBitsPixel = i;
        bmp.bmBits = NULL;
        bm = CreateBitmapIndirect(&bmp);
        if(i > 32) {
            ok(bm == 0 ||
               broken(bm != 0), /* Win9x and WinMe */
               "CreateBitmapIndirect for %d bpp succeeded\n", i);
            DeleteObject(bm);
            continue;
        }
        ok(bm != 0, "CreateBitmapIndirect failed\n");
        GetObject(bm, sizeof(bmp), &bmp);
        if(i == 1) {
            expect = 1;
        } else if(i <= 4) {
            expect = 4;
        } else if(i <= 8) {
            expect = 8;
        } else if(i <= 16) {
            expect = 16;
        } else if(i <= 24) {
            expect = 24;
        } else if(i <= 32) {
            expect = 32;
        }
        ok(bmp.bmBitsPixel == expect ||
           broken(bmp.bmBitsPixel == i), /* Win9x and WinMe */
           "CreateBitmapIndirect for a %d bpp bitmap created a %d bpp bitmap, expected %d\n",
           i, bmp.bmBitsPixel, expect);
        DeleteObject(bm);
    }
}

static char g16_info[sizeof(BITMAPINFOHEADER) + 1024];

static void test_get16dibits(void)
{
    BYTE bits[4 * (16 / sizeof(BYTE))];
    HBITMAP hbmp;
    HDC screen_dc = GetDC(NULL);
    int ret;
    BITMAPINFO * info = (BITMAPINFO *)g16_info;
    int info_len = sizeof(g16_info);
    BYTE *p;
    int overwritten_bytes = 0;

    memset(bits, 0, sizeof(bits));
    hbmp = CreateBitmap(2, 2, 1, 16, bits);
    ok(hbmp != NULL, "CreateBitmap failed\n");

    memset(info, '!', info_len);
    memset(info, 0, sizeof(info->bmiHeader));

    info->bmiHeader.biSize = sizeof(info->bmiHeader);
    info->bmiHeader.biWidth = 2;
    info->bmiHeader.biHeight = 2;
    info->bmiHeader.biPlanes = 1;
    info->bmiHeader.biCompression = BI_RGB;

    ret = GetDIBits(screen_dc, hbmp, 0, 0, NULL, info, 0);
    ok(ret != 0 ||
       broken(ret == 0), /* win9x */
       "GetDIBits failed got %d\n", ret);

    for (p = ((BYTE *) info) + sizeof(info->bmiHeader); (p - ((BYTE *) info)) < info_len; p++)
        if (*p != '!')
            overwritten_bytes++;
    ok(overwritten_bytes == 0, "GetDIBits wrote past the buffer given\n");

    DeleteObject(hbmp);
    ReleaseDC(NULL, screen_dc);
}

/* The pixels of 32-bit device-dependent bitmaps, which stand in for
 * Wine's DIB sections. */
static BOOL have_32bit_display(void)
{
    HDC hdc = GetDC(0);
    int bpp = GetDeviceCaps(hdc, BITSPIXEL) * GetDeviceCaps(hdc, PLANES);
    ReleaseDC(0, hdc);
    return bpp == 32;
}

static HBITMAP create_32bit_ddb(int width, int height)
{
    HDC hdc = GetDC(0);
    HBITMAP bmp = CreateCompatibleBitmap(hdc, width, height);
    ReleaseDC(0, hdc);
    return bmp;
}

static void set_pixels(HBITMAP bmp, const DWORD *pixels, int count)
{
    SetBitmapBits(bmp, count * 4L, pixels);
}

static void get_pixels(HBITMAP bmp, DWORD *pixels, int count)
{
    memset(pixels, 0xcc, count * 4);
    GetBitmapBits(bmp, count * 4L, pixels);
}

static BOOL compare_buffers_no_alpha(const DWORD *a, const DWORD *b, int length)
{
    int i;
    for(i = 0; i < length; i++)
        if((a[i] & 0x00FFFFFFL) != (b[i] & 0x00FFFFFFL))
            return FALSE;
    return TRUE;
}

static void check_BitBlt_pixel(HDC hdcDst, HDC hdcSrc, HBITMAP bmpDst, HBITMAP bmpSrc,
                               DWORD dwRop, DWORD expected, int line)
{
    DWORD src = 0xFEDCBA98L, dst = 0x89ABCDEFL;
    set_pixels(bmpSrc, &src, 1);
    set_pixels(bmpDst, &dst, 1);
    BitBlt(hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, dwRop);
    get_pixels(bmpDst, &dst, 1);
    ok_(__FILE__, line)(compare_buffers_no_alpha(&expected, &dst, 1),
        "BitBlt with dwRop %06lX. Expected 0x%08lX, got 0x%08lX\n",
        dwRop, expected, dst);
}

static void test_BitBlt(void)
{
    HBITMAP bmpDst, bmpSrc;
    HBITMAP oldDst, oldSrc;
    HDC hdcScreen, hdcDst, hdcSrc;
    HBRUSH hBrush, hOldBrush;

    if (!have_32bit_display())
    {
        skip("BitBlt's tests need a 32-bit display\n");
        return;
    }

    hdcScreen = CreateCompatibleDC(0);
    hdcDst = CreateCompatibleDC(hdcScreen);
    hdcSrc = CreateCompatibleDC(hdcDst);

    /* Setup the destination bitmap */
    bmpDst = create_32bit_ddb(1, 1);
    oldDst = SelectObject(hdcDst, bmpDst);

    hBrush = CreateSolidBrush(0x012345678L);
    hOldBrush = SelectObject(hdcDst, hBrush);

    /* Setup the source bitmap */
    bmpSrc = create_32bit_ddb(1, 1);
    oldSrc = SelectObject(hdcSrc, bmpSrc);

    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCCOPY, 0xFEDCBA98L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCPAINT, 0xFFFFFFFFL, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCAND, 0x88888888L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCINVERT, 0x77777777L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCERASE, 0x76543210L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, NOTSRCCOPY, 0x01234567L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, NOTSRCERASE, 0x00000000L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, MERGECOPY, 0x00581210L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, MERGEPAINT, 0x89ABCDEFL, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATCOPY, 0x00785634L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATPAINT, 0x89FBDFFFL, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATINVERT, 0x89D39BDBL, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, DSTINVERT, 0x76543210L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, BLACKNESS, 0x00000000L, __LINE__);
    check_BitBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, WHITENESS, 0xFFFFFFFFL, __LINE__);

    /* Tidy up */
    SelectObject(hdcSrc, oldSrc);
    DeleteObject(bmpSrc);
    DeleteDC(hdcSrc);

    SelectObject(hdcDst, hOldBrush);
    DeleteObject(hBrush);
    SelectObject(hdcDst, oldDst);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);

    DeleteDC(hdcScreen);
}

static void check_StretchBlt_pixel(HDC hdcDst, HDC hdcSrc, HBITMAP bmpDst, HBITMAP bmpSrc,
                                   DWORD dwRop, DWORD expected, int line)
{
    DWORD src[4] = { 0xFEDCBA98L, 0, 0, 0 };
    DWORD dst[4] = { 0x89ABCDEFL, 0, 0, 0 };
    set_pixels(bmpSrc, src, 4);
    set_pixels(bmpDst, dst, 4);
    StretchBlt(hdcDst, 0, 0, 2, 1, hdcSrc, 0, 0, 1, 1, dwRop);
    get_pixels(bmpDst, dst, 4);
    ok_(__FILE__, line)(compare_buffers_no_alpha(&expected, dst, 1),
        "StretchBlt with dwRop %06lX. Expected 0x%08lX, got 0x%08lX\n",
        dwRop, expected, dst[0]);
}

static void check_StretchBlt_stretch(HDC hdcDst, HDC hdcSrc, HBITMAP bmpDst, HBITMAP bmpSrc,
                                     const DWORD *srcBuffer,
                                     int nXOriginDest, int nYOriginDest, int nWidthDest, int nHeightDest,
                                     int nXOriginSrc, int nYOriginSrc, int nWidthSrc, int nHeightSrc,
                                     const DWORD expected[4], const DWORD legacy_expected[4], int line)
{
    static const DWORD zero[4];
    DWORD dstBuffer[4];
    set_pixels(bmpSrc, srcBuffer, 4);
    set_pixels(bmpDst, zero, 4);
    StretchBlt(hdcDst, nXOriginDest, nYOriginDest, nWidthDest, nHeightDest,
               hdcSrc, nXOriginSrc, nYOriginSrc, nWidthSrc, nHeightSrc, SRCCOPY);
    get_pixels(bmpDst, dstBuffer, 4);
    ok_(__FILE__, line)(compare_buffers_no_alpha(dstBuffer, expected, 4) ||
        broken(compare_buffers_no_alpha(dstBuffer, legacy_expected, 4)),
        "StretchBlt expected { %08lX, %08lX, %08lX, %08lX } got { %08lX, %08lX, %08lX, %08lX } "
        "stretching { %d, %d, %d, %d } to { %d, %d, %d, %d }\n",
        expected[0], expected[1], expected[2], expected[3],
        dstBuffer[0], dstBuffer[1], dstBuffer[2], dstBuffer[3],
        nXOriginSrc, nYOriginSrc, nWidthSrc, nHeightSrc,
        nXOriginDest, nYOriginDest, nWidthDest, nHeightDest);
}

static void test_StretchBlt(void)
{
    HBITMAP bmpDst, bmpSrc;
    HBITMAP oldDst, oldSrc;
    HDC hdcScreen, hdcDst, hdcSrc;
    HBRUSH hBrush, hOldBrush;
    DWORD srcBuffer[4], expected[4], legacy_expected[4];

    if (!have_32bit_display())
    {
        skip("StretchBlt's tests need a 32-bit display\n");
        return;
    }

    hdcScreen = CreateCompatibleDC(0);
    hdcDst = CreateCompatibleDC(hdcScreen);
    hdcSrc = CreateCompatibleDC(hdcDst);

    /* Pixel Tests */
    bmpDst = create_32bit_ddb(2, 2);
    oldDst = SelectObject(hdcDst, bmpDst);

    bmpSrc = create_32bit_ddb(2, 2);
    oldSrc = SelectObject(hdcSrc, bmpSrc);

    hBrush = CreateSolidBrush(0x012345678L);
    hOldBrush = SelectObject(hdcDst, hBrush);

    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCCOPY, 0xFEDCBA98L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCPAINT, 0xFFFFFFFFL, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCAND, 0x88888888L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCINVERT, 0x77777777L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, SRCERASE, 0x76543210L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, NOTSRCCOPY, 0x01234567L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, NOTSRCERASE, 0x00000000L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, MERGECOPY, 0x00581210L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, MERGEPAINT, 0x89ABCDEFL, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATCOPY, 0x00785634L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATPAINT, 0x89FBDFFFL, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, PATINVERT, 0x89D39BDBL, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, DSTINVERT, 0x76543210L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, BLACKNESS, 0x00000000L, __LINE__);
    check_StretchBlt_pixel(hdcDst, hdcSrc, bmpDst, bmpSrc, WHITENESS, 0xFFFFFFFFL, __LINE__);

    SelectObject(hdcDst, hOldBrush);
    DeleteObject(hBrush);

    /* Top-down to top-down tests: a device-dependent bitmap's pixels
     * are top-down */
    srcBuffer[0] = 0xCAFED00DL, srcBuffer[1] = 0xFEEDFACEL;
    srcBuffer[2] = 0xFEDCBA98L, srcBuffer[3] = 0x76543210L;

    expected[0] = 0xCAFED00DL, expected[1] = 0xFEEDFACEL;
    expected[2] = 0xFEDCBA98L, expected[3] = 0x76543210L;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             0, 0, 2, 2, 0, 0, 2, 2, expected, expected, __LINE__);

    expected[0] = 0xCAFED00DL, expected[1] = 0x00000000L;
    expected[2] = 0x00000000L, expected[3] = 0x00000000L;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             0, 0, 1, 1, 0, 0, 1, 1, expected, expected, __LINE__);

    expected[0] = 0xCAFED00DL, expected[1] = 0xCAFED00DL;
    expected[2] = 0xCAFED00DL, expected[3] = 0xCAFED00DL;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             0, 0, 2, 2, 0, 0, 1, 1, expected, expected, __LINE__);

    expected[0] = 0xCAFED00DL, expected[1] = 0x00000000L;
    expected[2] = 0x00000000L, expected[3] = 0x00000000L;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             0, 0, 1, 1, 0, 0, 2, 2, expected, expected, __LINE__);

    expected[0] = 0x76543210L, expected[1] = 0xFEDCBA98L;
    expected[2] = 0xFEEDFACEL, expected[3] = 0xCAFED00DL;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             0, 0, 2, 2, 1, 1, -2, -2, expected, expected, __LINE__);

    expected[0] = 0x76543210L, expected[1] = 0xFEDCBA98L;
    expected[2] = 0xFEEDFACEL, expected[3] = 0xCAFED00DL;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             1, 1, -2, -2, 0, 0, 2, 2, expected, expected, __LINE__);

    /* This result seems broken. One might expect the following result:
     * 0xCAFED00D 0xFEEDFACE
     * 0xFEDCBA98 0x76543210
     */
    expected[0] = 0xCAFED00DL, expected[1] = 0x00000000L;
    expected[2] = 0xFEDCBA98L, expected[3] = 0x76543210L;
    legacy_expected[0] = 0xCAFED00DL, legacy_expected[1] = 0x00000000L;
    legacy_expected[2] = 0x00000000L, legacy_expected[3] = 0x00000000L;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             1, 1, -2, -2, 1, 1, -2, -2, expected,
                             legacy_expected, __LINE__);

    expected[0] = 0x00000000L, expected[1] = 0x00000000L;
    expected[2] = 0x00000000L, expected[3] = 0xCAFED00DL;
    check_StretchBlt_stretch(hdcDst, hdcSrc, bmpDst, bmpSrc, srcBuffer,
                             1, 1, 2, 2, 0, 0, 2, 2, expected, expected, __LINE__);

    /* Tidy up */
    SelectObject(hdcSrc, oldSrc);
    DeleteObject(bmpSrc);
    DeleteDC(hdcSrc);

    SelectObject(hdcDst, oldDst);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);

    DeleteDC(hdcScreen);
}

/* A 2x1 24-bit DIB: its first pixel, 0xFEDCBA98's colour, and black */
static void check_StretchDIBits_pixel(HDC hdcDst, HBITMAP bmpDst,
                                      DWORD dwRop, DWORD expected, int line)
{
    static const BYTE buffer[8] = { 0x98, 0xBA, 0xDC, 0, 0, 0, 0, 0 };
    BITMAPINFO bitmapInfo;
    DWORD dst[4] = { 0x89ABCDEFL, 0, 0, 0 };

    memset(&bitmapInfo, 0, sizeof(BITMAPINFO));
    bitmapInfo.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bitmapInfo.bmiHeader.biWidth = 2;
    bitmapInfo.bmiHeader.biHeight = 1;
    bitmapInfo.bmiHeader.biPlanes = 1;
    bitmapInfo.bmiHeader.biBitCount = 24;
    bitmapInfo.bmiHeader.biCompression = BI_RGB;
    bitmapInfo.bmiHeader.biSizeImage = sizeof(buffer);

    set_pixels(bmpDst, dst, 4);

    StretchDIBits(hdcDst, 0, 0, 2, 1, 0, 0, 1, 1, buffer, &bitmapInfo, DIB_RGB_COLORS, dwRop);
    get_pixels(bmpDst, dst, 4);
    ok_(__FILE__, line)(compare_buffers_no_alpha(&expected, dst, 1),
        "StretchDIBits with dwRop %06lX. Expected 0x%08lX, got 0x%08lX\n",
        dwRop, expected, dst[0]);
}

/* srcBuffer is a 2x2 24-bit bottom-up DIB of Wine's top-down 32-bit
 * one: its last row first */
static void check_StretchDIBits_stretch(HDC hdcDst, HBITMAP bmpDst, const DWORD *srcBuffer,
                                        int nXOriginDest, int nYOriginDest, int nWidthDest, int nHeightDest,
                                        int nXOriginSrc, int nYOriginSrc, int nWidthSrc, int nHeightSrc,
                                        const DWORD expected[4], const DWORD legacy_expected[4], int line)
{
    static const DWORD zero[4];
    BITMAPINFO bitmapInfo;
    BYTE dib[16];
    DWORD dstBuffer[4];
    int i;

    memset(dib, 0, sizeof(dib));
    for (i = 0; i < 4; i++)
    {
        /* row 1 of the top-down image is row 0 of the bottom-up DIB */
        BYTE *p = dib + (i < 2 ? 8 : 0) + (i & 1) * 3;
        p[0] = (BYTE)srcBuffer[i];
        p[1] = (BYTE)(srcBuffer[i] >> 8);
        p[2] = (BYTE)(srcBuffer[i] >> 16);
    }

    memset(&bitmapInfo, 0, sizeof(BITMAPINFO));
    bitmapInfo.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bitmapInfo.bmiHeader.biWidth = 2;
    bitmapInfo.bmiHeader.biHeight = 2;
    bitmapInfo.bmiHeader.biPlanes = 1;
    bitmapInfo.bmiHeader.biBitCount = 24;
    bitmapInfo.bmiHeader.biCompression = BI_RGB;

    set_pixels(bmpDst, zero, 4);
    StretchDIBits(hdcDst, nXOriginDest, nYOriginDest, nWidthDest, nHeightDest,
                  nXOriginSrc, nYOriginSrc, nWidthSrc, nHeightSrc,
                  dib, &bitmapInfo, DIB_RGB_COLORS, SRCCOPY);
    get_pixels(bmpDst, dstBuffer, 4);
    ok_(__FILE__, line)(compare_buffers_no_alpha(dstBuffer, expected, 4) ||
        broken(compare_buffers_no_alpha(dstBuffer, legacy_expected, 4)),
        "StretchDIBits expected { %08lX, %08lX, %08lX, %08lX } got { %08lX, %08lX, %08lX, %08lX } "
        "stretching { %d, %d, %d, %d } to { %d, %d, %d, %d }\n",
        expected[0], expected[1], expected[2], expected[3],
        dstBuffer[0], dstBuffer[1], dstBuffer[2], dstBuffer[3],
        nXOriginSrc, nYOriginSrc, nWidthSrc, nHeightSrc,
        nXOriginDest, nYOriginDest, nWidthDest, nHeightDest);
}

static void test_StretchDIBits(void)
{
    HBITMAP bmpDst;
    HBITMAP oldDst;
    HDC hdcScreen, hdcDst;
    DWORD srcBuffer[4];
    HBRUSH hBrush, hOldBrush;
    DWORD expected[4], legacy_expected[4];

    if (!have_32bit_display())
    {
        skip("StretchDIBits's tests need a 32-bit display\n");
        return;
    }

    hdcScreen = CreateCompatibleDC(0);
    hdcDst = CreateCompatibleDC(hdcScreen);

    /* Pixel Tests */
    bmpDst = create_32bit_ddb(2, 2);
    oldDst = SelectObject(hdcDst, bmpDst);

    hBrush = CreateSolidBrush(0x012345678L);
    hOldBrush = SelectObject(hdcDst, hBrush);

    check_StretchDIBits_pixel(hdcDst, bmpDst, SRCCOPY, 0xFEDCBA98L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, SRCPAINT, 0xFFFFFFFFL, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, SRCAND, 0x88888888L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, SRCINVERT, 0x77777777L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, SRCERASE, 0x76543210L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, NOTSRCCOPY, 0x01234567L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, NOTSRCERASE, 0x00000000L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, MERGECOPY, 0x00581210L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, MERGEPAINT, 0x89ABCDEFL, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, PATCOPY, 0x00785634L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, PATPAINT, 0x89FBDFFFL, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, PATINVERT, 0x89D39BDBL, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, DSTINVERT, 0x76543210L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, BLACKNESS, 0x00000000L, __LINE__);
    check_StretchDIBits_pixel(hdcDst, bmpDst, WHITENESS, 0xFFFFFFFFL, __LINE__);

    SelectObject(hdcDst, hOldBrush);
    DeleteObject(hBrush);

    srcBuffer[0] = 0xCAFED00DL, srcBuffer[1] = 0xFEEDFACEL;
    srcBuffer[2] = 0xFEDCBA98L, srcBuffer[3] = 0x76543210L;

    expected[0] = 0xCAFED00DL, expected[1] = 0xFEEDFACEL;
    expected[2] = 0xFEDCBA98L, expected[3] = 0x76543210L;
    check_StretchDIBits_stretch(hdcDst, bmpDst, srcBuffer,
                                0, 0, 2, 2, 0, 0, 2, 2, expected, expected, __LINE__);

    /* The source origin of a bottom-up DIB is its bottom left corner */
    expected[0] = 0xFEDCBA98L, expected[1] = 0x00000000L;
    expected[2] = 0x00000000L, expected[3] = 0x00000000L;
    legacy_expected[0] = 0xFEDCBA98L, legacy_expected[1] = 0x00000000L;
    legacy_expected[2] = 0x00000000L, legacy_expected[3] = 0x00000000L;
    check_StretchDIBits_stretch(hdcDst, bmpDst, srcBuffer,
                                0, 0, 1, 1, 0, 0, 1, 1, expected, legacy_expected, __LINE__);

    expected[0] = 0xFEDCBA98L, expected[1] = 0xFEDCBA98L;
    expected[2] = 0xFEDCBA98L, expected[3] = 0xFEDCBA98L;
    check_StretchDIBits_stretch(hdcDst, bmpDst, srcBuffer,
                                0, 0, 2, 2, 0, 0, 1, 1, expected, expected, __LINE__);

    expected[0] = 0x00000000L, expected[1] = 0x00000000L;
    expected[2] = 0x00000000L, expected[3] = 0xCAFED00DL;
    check_StretchDIBits_stretch(hdcDst, bmpDst, srcBuffer,
                                1, 1, 2, 2, 0, 0, 2, 2, expected, expected, __LINE__);

    /* Tidy up */
    SelectObject(hdcDst, oldDst);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);

    DeleteDC(hdcScreen);
}

static void test_clipping(void)
{
    HBITMAP bmpDst;
    HBITMAP bmpSrc;
    HRGN hRgn;
    BOOL result;
    HDC hdcScreen = GetDC(0);

    HDC hdcDst = CreateCompatibleDC( NULL );
    HDC hdcSrc = CreateCompatibleDC( NULL );

    bmpDst = CreateCompatibleBitmap( hdcScreen, 100, 100 );
    ok(bmpDst != NULL, "Couldn't create destination bitmap\n");
    SelectObject( hdcDst, bmpDst );

    bmpSrc = CreateCompatibleBitmap( hdcScreen, 100, 100 );
    ok(bmpSrc != NULL, "Couldn't create source bitmap\n");
    SelectObject( hdcSrc, bmpSrc );
    ReleaseDC(0, hdcScreen);

    result = BitBlt( hdcDst, 0, 0, 100, 100, hdcSrc, 100, 100, SRCCOPY );
    ok(result, "BitBlt failed\n");

    hRgn = CreateRectRgn( 0,0,0,0 );
    SelectClipRgn( hdcDst, hRgn );

    result = BitBlt( hdcDst, 0, 0, 100, 100, hdcSrc, 0, 0, SRCCOPY );
    ok(result, "BitBlt failed\n");

    DeleteDC( hdcDst );
    DeleteDC( hdcSrc );
    DeleteObject( bmpDst );
    DeleteObject( bmpSrc );
    DeleteObject( hRgn );
}

static void test_32bit_bitmap_blt(void)
{
    HBITMAP bmpSrc, bmpDst;
    HBITMAP oldSrc, oldDst;
    HDC hdcSrc, hdcDst, hdcScreen;
    DWORD dst;
    DWORD colorSrc = 0x11223344L;

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
    bmpDst = create_32bit_ddb(1, 1);
    oldDst = SelectObject(hdcDst, bmpDst);

    StretchBlt(hdcDst, 0, 0, 1, 1, hdcSrc, 0, 0, 1, 1, SRCCOPY);
    get_pixels(bmpDst, &dst, 1);
    ok(compare_buffers_no_alpha(&dst, &colorSrc, 1),
       "Expected color=%lx, received color=%lx\n", colorSrc, dst);

    /* Tidy up */
    SelectObject(hdcDst, oldDst);
    DeleteObject(bmpDst);
    DeleteDC(hdcDst);

    SelectObject(hdcSrc, oldSrc);
    DeleteObject(bmpSrc);
    DeleteDC(hdcSrc);

    DeleteDC(hdcScreen);
}

/* Each format's 4x2 DIB: the colours of its pixels from the top left. */
static const COLORREF dib_colors[8] = {
    RGB(0x10,0x20,0x30), RGB(0xff,0xff,0xff), RGB(0,0,0), RGB(0x80,0x40,0x20),
    RGB(0x01,0x02,0x03), RGB(0x80,0x40,0x20), RGB(0xff,0xff,0xff), RGB(0x10,0x20,0x30)
};

static struct
{
    BITMAPINFOHEADER bmiHeader;
    RGBQUAD bmiColors[256];
} dib_info;
static BYTE dib_data[32];

/* Make the 4x2 DIB of `bpp` bits, with a colour table that holds the
 * colours in a different order, where there is one. */
static void make_dib(int bpp)
{
    static const BYTE order[4] = { 3, 0, 2, 1 };
    int stride = ((4 * bpp + 31) / 32) * 4;
    int x, y, i;

    memset(&dib_info, 0, sizeof(dib_info));
    memset(dib_data, 0, sizeof(dib_data));
    dib_info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    dib_info.bmiHeader.biWidth = 4;
    dib_info.bmiHeader.biHeight = 2;
    dib_info.bmiHeader.biPlanes = 1;
    dib_info.bmiHeader.biBitCount = bpp;
    dib_info.bmiHeader.biCompression = BI_RGB;

    if (bpp == 1)
    {
        /* black and white only: the black and white pixels, and the
         * others as what their index's colour is */
        dib_info.bmiColors[0].rgbRed = 0x10;
        dib_info.bmiColors[0].rgbGreen = 0x20;
        dib_info.bmiColors[0].rgbBlue = 0x30;
        dib_info.bmiColors[1].rgbRed = 0xff;
        dib_info.bmiColors[1].rgbGreen = 0xff;
        dib_info.bmiColors[1].rgbBlue = 0xff;
        /* top row 0 1 0 0, bottom row 0 0 1 0, from the bottom up */
        dib_data[0] = 0x20;
        dib_data[stride] = 0x40;
        return;
    }

    /* The colour table: the distinct colours in `order`. */
    {
        static const COLORREF distinct[4] = {
            RGB(0x10,0x20,0x30), RGB(0xff,0xff,0xff), RGB(0,0,0), RGB(0x80,0x40,0x20)
        };
        if (bpp <= 8)
        {
            for (i = 0; i < 4; i++)
            {
                COLORREF c = distinct[order[i]];
                dib_info.bmiColors[i].rgbRed = GetRValue(c);
                dib_info.bmiColors[i].rgbGreen = GetGValue(c);
                dib_info.bmiColors[i].rgbBlue = GetBValue(c);
            }
            dib_info.bmiColors[4].rgbRed = 0x01;
            dib_info.bmiColors[4].rgbGreen = 0x02;
            dib_info.bmiColors[4].rgbBlue = 0x03;
            dib_info.bmiHeader.biClrUsed = 5;
        }
    }

    for (y = 0; y < 2; y++)
    {
        BYTE *row = dib_data + (1 - y) * stride;
        for (x = 0; x < 4; x++)
        {
            COLORREF c = dib_colors[y * 4 + x];
            int index = 4;
            for (i = 0; i < 4; i++)
                if (dib_info.bmiColors[i].rgbRed == GetRValue(c) &&
                    dib_info.bmiColors[i].rgbGreen == GetGValue(c) &&
                    dib_info.bmiColors[i].rgbBlue == GetBValue(c))
                    index = i;
            switch (bpp)
            {
            case 4:
                row[x / 2] |= (x & 1) ? index : index << 4;
                break;
            case 8:
                row[x] = (BYTE)index;
                break;
            case 24:
                row[x * 3] = GetBValue(c);
                row[x * 3 + 1] = GetGValue(c);
                row[x * 3 + 2] = GetRValue(c);
                break;
            }
        }
    }
}

/* The colour of the 4x2 DIB's pixel at (x,y). */
static COLORREF dib_color(int bpp, int x, int y)
{
    if (bpp == 1)
    {
        static const BYTE bits[8] = { 0, 1, 0, 0, 0, 0, 1, 0 };
        return bits[y * 4 + x] ? RGB(0xff,0xff,0xff) : RGB(0x10,0x20,0x30);
    }
    return dib_colors[y * 4 + x];
}

static void check_dib_pixels(HDC hdc, int bpp, int left, int top, int scale,
                             const char *what, int line)
{
    int x, y;
    for (y = 0; y < 2; y++)
        for (x = 0; x < 4; x++)
        {
            COLORREF want = dib_color(bpp, x, y);
            COLORREF got = GetPixel(hdc, left + x * scale, top + y * scale);
            COLORREF got2 = GetPixel(hdc, left + x * scale + scale - 1,
                                     top + y * scale + scale - 1);
            ok_(__FILE__, line)(got == want && got2 == want,
               "%s, %d bits: pixel (%d,%d) is %08lx and %08lx, expected %08lx\n",
               what, bpp, x, y, got, got2, want);
        }
}

static void test_dib_formats(void)
{
    static const int depths[] = { 1, 4, 8, 24 };
    static struct
    {
        BITMAPINFOHEADER bmiHeader;
        RGBQUAD bmiColors[256];
    } out_info;
    BYTE out[32];
    HDC screen, hdc;
    HBITMAP bmp, old, init;
    int i, ret, x, y;

    if (!have_32bit_display())
    {
        skip("the DIB colour tests need a 32-bit display\n");
        return;
    }

    screen = GetDC(0);
    hdc = CreateCompatibleDC(screen);
    bmp = CreateCompatibleBitmap(screen, 16, 8);
    old = SelectObject(hdc, bmp);

    for (i = 0; i < sizeof(depths)/sizeof(depths[0]); i++)
    {
        int bpp = depths[i];
        make_dib(bpp);

        /* SetDIBits into the bitmap, not selected */
        SelectObject(hdc, old);
        ret = SetDIBits(hdc, bmp, 0, 2, dib_data, (BITMAPINFO *)&dib_info, DIB_RGB_COLORS);
        ok(ret == 2, "%d bits: SetDIBits returned %d\n", bpp, ret);
        SelectObject(hdc, bmp);
        check_dib_pixels(hdc, bpp, 0, 0, 1, "SetDIBits", __LINE__);

        /* GetDIBits reads them back as 24-bit */
        memset(&out_info, 0, sizeof(out_info));
        out_info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        out_info.bmiHeader.biWidth = 4;
        out_info.bmiHeader.biHeight = 2;
        out_info.bmiHeader.biPlanes = 1;
        out_info.bmiHeader.biBitCount = 24;
        out_info.bmiHeader.biCompression = BI_RGB;
        memset(out, 0xaa, sizeof(out));
        SelectObject(hdc, old);
        ret = GetDIBits(hdc, bmp, 0, 2, out, (BITMAPINFO *)&out_info, DIB_RGB_COLORS);
        SelectObject(hdc, bmp);
        ok(ret == 2, "%d bits: GetDIBits returned %d\n", bpp, ret);
        for (y = 0; y < 2; y++)
            for (x = 0; x < 4; x++)
            {
                BYTE *p = out + (1 - y) * 12 + x * 3;
                COLORREF got = RGB(p[2], p[1], p[0]);
                ok(got == dib_color(bpp, x, y),
                   "%d bits: GetDIBits pixel (%d,%d) is %08lx, expected %08lx\n",
                   bpp, x, y, got, dib_color(bpp, x, y));
            }

        /* SetDIBitsToDevice, at (5,1), in a white bitmap */
        PatBlt(hdc, 0, 0, 16, 8, WHITENESS);
        ret = SetDIBitsToDevice(hdc, 5, 1, 4, 2, 0, 0, 0, 2, dib_data,
                                (BITMAPINFO *)&dib_info, DIB_RGB_COLORS);
        ok(ret == 2, "%d bits: SetDIBitsToDevice returned %d\n", bpp, ret);
        check_dib_pixels(hdc, bpp, 5, 1, 1, "SetDIBitsToDevice", __LINE__);
        ok(GetPixel(hdc, 4, 1) == RGB(255,255,255) && GetPixel(hdc, 9, 2) == RGB(255,255,255) &&
           GetPixel(hdc, 5, 0) == RGB(255,255,255) && GetPixel(hdc, 5, 3) == RGB(255,255,255),
           "%d bits: SetDIBitsToDevice drew outside its rectangle\n", bpp);

        /* StretchDIBits, twice the size, at (2,2) */
        PatBlt(hdc, 0, 0, 16, 8, WHITENESS);
        ret = StretchDIBits(hdc, 2, 2, 8, 4, 0, 0, 4, 2, dib_data,
                            (BITMAPINFO *)&dib_info, DIB_RGB_COLORS, SRCCOPY);
        ok(ret == 2, "%d bits: StretchDIBits returned %d\n", bpp, ret);
        check_dib_pixels(hdc, bpp, 2, 2, 2, "StretchDIBits", __LINE__);
        ok(GetPixel(hdc, 10, 2) == RGB(255,255,255) && GetPixel(hdc, 2, 6) == RGB(255,255,255),
           "%d bits: StretchDIBits drew outside its rectangle\n", bpp);

        /* CreateDIBitmap with the bits */
        init = CreateDIBitmap(screen, &dib_info.bmiHeader, CBM_INIT, dib_data,
                              (BITMAPINFO *)&dib_info, DIB_RGB_COLORS);
        ok(init != 0, "%d bits: CreateDIBitmap failed\n", bpp);
        if (init)
        {
            HDC hdc2 = CreateCompatibleDC(screen);
            HBITMAP old2 = SelectObject(hdc2, init);
            check_dib_pixels(hdc2, bpp, 0, 0, 1, "CreateDIBitmap", __LINE__);
            SelectObject(hdc2, old2);
            DeleteDC(hdc2);
            DeleteObject(init);
        }
    }

    SelectObject(hdc, old);
    DeleteObject(bmp);
    DeleteDC(hdc);
    ReleaseDC(0, screen);
}

START_TEST(bitmap)
{
    test_createdibitmap();
    test_bitmap();
    test_bmBits();
    test_GetDIBits_selected_DDB(TRUE);
    test_GetDIBits_selected_DDB(FALSE);
    test_GetDIBits();
    test_select_object();
    test_CreateBitmap();
    test_BitBlt();
    test_StretchBlt();
    test_StretchDIBits();
    test_32bit_bitmap_blt();
    test_get16dibits();
    test_clipping();
    test_dib_formats();
}
