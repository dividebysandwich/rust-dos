/*
 * The pixels MSIMG32's AlphaBlend, TransparentBlt and GradientFill draw
 * into small DIBs, and what they return, as Windows 98 SE's MSIMG32.DLL
 * (5.00.1693.1) drew and returned them. rust-dos's own test, written
 * with Wine's test framework; Wine's tests check few of these pixels.
 *
 * Copyright the rust-dos authors
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 */

#include <stdarg.h>
#include <string.h>

#include "windef.h"
#include "winbase.h"
#include "wingdi.h"
#include "winuser.h"

#include "wine/test.h"

typedef BOOL (WINAPI *ALPHA)(HDC, int, int, int, int, HDC, int, int, int, int, BLENDFUNCTION);
typedef BOOL (WINAPI *TRANS)(HDC, int, int, int, int, HDC, int, int, int, int, UINT);
typedef BOOL (WINAPI *GRAD)(HDC, PTRIVERTEX, ULONG, PVOID, ULONG, ULONG);
static ALPHA pAlpha;
static TRANS pTrans;
static GRAD pGrad;

struct dib { HDC dc; HBITMAP bmp; void *bits; int w, h, bpp, stride; };

static const DWORD m565[3] = { 0xf800, 0x07e0, 0x001f };

/* A top-down DIB section of w by h pixels selected into a memory DC. */
static struct dib make(int w, int h, int bpp, const DWORD *masks, const RGBQUAD *pal, int ncol)
{
    struct dib d;
    char buf[sizeof(BITMAPINFOHEADER) + 256 * 4];
    BITMAPINFO *bi = (BITMAPINFO *)buf;
    memset(buf, 0, sizeof(buf));
    bi->bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi->bmiHeader.biWidth = w;
    bi->bmiHeader.biHeight = -h;
    bi->bmiHeader.biPlanes = 1;
    bi->bmiHeader.biBitCount = bpp;
    bi->bmiHeader.biCompression = masks ? BI_BITFIELDS : BI_RGB;
    if (masks) memcpy(bi->bmiColors, masks, 12);
    if (pal) { memcpy(bi->bmiColors, pal, ncol * 4); bi->bmiHeader.biClrUsed = ncol; }
    d.dc = CreateCompatibleDC(0);
    d.bmp = CreateDIBSection(d.dc, bi, DIB_RGB_COLORS, &d.bits, NULL, 0);
    SelectObject(d.dc, d.bmp);
    d.w = w; d.h = h; d.bpp = bpp;
    d.stride = ((w * bpp + 31) / 32) * 4;
    return d;
}

static void drop(struct dib *d) { DeleteDC(d->dc); DeleteObject(d->bmp); }

static DWORD px(struct dib *d, int x, int y)
{
    BYTE *row = (BYTE *)d->bits + y * d->stride;
    switch (d->bpp) {
    case 8: return row[x];
    case 16: return ((WORD *)row)[x];
    case 24: return row[3 * x] | row[3 * x + 1] << 8 | row[3 * x + 2] << 16;
    default: return ((DWORD *)row)[x];
    }
}

static void setpx(struct dib *d, int x, int y, DWORD v)
{
    BYTE *row = (BYTE *)d->bits + y * d->stride;
    switch (d->bpp) {
    case 8: row[x] = (BYTE)v; break;
    case 16: ((WORD *)row)[x] = (WORD)v; break;
    case 24:
        row[3 * x] = (BYTE)v;
        row[3 * x + 1] = (BYTE)(v >> 8);
        row[3 * x + 2] = (BYTE)(v >> 16);
        break;
    default: ((DWORD *)row)[x] = v;
    }
}

static void fill(struct dib *d, DWORD v)
{
    int x, y;
    for (y = 0; y < d->h; y++) for (x = 0; x < d->w; x++) setpx(d, x, y, v);
}

/* What a call returned and drew against Windows 98 SE's: its result,
   no error when it failed, and each pixel of the destination. */
static void check(const char *name, BOOL ret, struct dib *d, BOOL want_ret,
                  const DWORD *want)
{
    int x, y, bad = 0;
    ok(ret == want_ret, "%s: returned %d\n", name, ret);
    if (!ret)
        ok(GetLastError() == 0xdeadbeef, "%s: error %u\n", name, GetLastError());
    for (y = 0; y < d->h; y++)
        for (x = 0; x < d->w; x++)
            if (px(d, x, y) != want[y * d->w + x] && bad++ < 4)
                ok(0, "%s: pixel (%d, %d) is %x, not %x\n", name, x, y,
                   px(d, x, y), want[y * d->w + x]);
}

static BLENDFUNCTION bf(BYTE sca, BYTE fmt)
{
    BLENDFUNCTION b;
    b.BlendOp = AC_SRC_OVER; b.BlendFlags = 0; b.SourceConstantAlpha = sca; b.AlphaFormat = fmt;
    return b;
}

static void test_AlphaBlend(void)
{
    BOOL ret;
    struct dib s = make(4, 1, 32, NULL, NULL, 0), d = make(4, 1, 32, NULL, NULL, 0);
    struct dib d16 = make(4, 1, 16, m565, NULL, 0), d24 = make(4, 1, 24, NULL, NULL, 0);
    struct dib s24 = make(4, 1, 24, NULL, NULL, 0), s2 = make(2, 1, 32, NULL, NULL, 0);
    struct dib big = make(5, 2, 32, NULL, NULL, 0);

    setpx(&s, 0, 0, 0x40201008); setpx(&s, 1, 0, 0x80ff0000);
    setpx(&s, 2, 0, 0xff00ff00); setpx(&s, 3, 0, 0x00123456);
    setpx(&s24, 0, 0, 0x201008); setpx(&s24, 1, 0, 0xff0000);
    setpx(&s24, 2, 0, 0x00ff00); setpx(&s24, 3, 0, 0x123456);
    setpx(&s2, 0, 0, 0xff0000ff); setpx(&s2, 1, 0, 0x8000ff00);
    {
        static const DWORD want[] = {
            0x70686462, 0x80a06060, 0xa060a060, 0x60646d75
        };
        fill(&d, 0x80808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(64, 0));
        check("ab32 const 64", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x88807c7a, 0x90b07070, 0xa060a060, 0x80808080
        };
        fill(&d, 0x80808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(64, AC_SRC_ALPHA));
        check("ab32 pp 64", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x40201008, 0x80ff0000, 0xff00ff00, 0x123456
        };
        fill(&d, 0x80808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(255, 0));
        check("ab32 const 255", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xa0807068, 0xc13f4040, 0xff00ff00, 0x80808080
        };
        fill(&d, 0x80808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(255, AC_SRC_ALPHA));
        check("ab32 pp 255", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x504844, 0xc04040, 0x40c040, 0x495a6b
        };
        fill(&d24, 0x808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d24.dc, 0, 0, 4, 1, s24.dc, 0, 0, 4, 1, bf(128, 0));
        check("ab24 const", ret, &d24, 1, want);
    }
    {
        static const DWORD want[] = {
            0xc0504844, 0xc0c04040, 0xc040c040, 0xc0495a6b
        };
        fill(&d, 0x80808080);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s24.dc, 0, 0, 4, 1, bf(128, 0));
        check("ab24to32 const", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000ff, 0xff0000ff, 0xff0000ff, 0x8000ff00, 0x8000ff00,
            0xff0000ff, 0xff0000ff, 0xff0000ff, 0x8000ff00, 0x8000ff00
        };
        fill(&big, 0);
        SetLastError(0xdeadbeef);
        ret = pAlpha(big.dc, 0, 0, 5, 2, s2.dc, 0, 0, 2, 1, bf(255,
            AC_SRC_ALPHA));
        check("ab stretch", ret, &big, 1, want);
    }
    /* premultiplied colors with alpha below them */
    setpx(&s, 0, 0, 0x80404040); setpx(&s, 1, 0, 0x10ffffff);
    setpx(&s, 2, 0, 0x01010101); setpx(&s, 3, 0, 0xfe7f7f7f);
    {
        static const DWORD want[] = {
            0x80506070, 0x111e3c59, 0x1214161, 0xfe7f7f7f
        };
        fill(&d, 0x00204060);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(255, AC_SRC_ALPHA));
        check("ab32 pp2", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff334c66, 0xff83a2c2, 0xff204060, 0xff45596c
        };
        fill(&d, 0xff204060);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(100, AC_SRC_ALPHA));
        check("ab32 pp3", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x630c, 0xc618, 0x4208, 0x7bef
        };
        fill(&d16, 0x8410);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d16.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(128, 0));
        check("ab565 const", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0x8410, 0x7bce, 0x8410, 0x8410
        };
        fill(&d16, 0x8410);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d16.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, bf(255,
            AC_SRC_ALPHA));
        check("ab565 pp", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0x10ffffff, 0x10ffffff, 0x1010101, 0x1010101
        };
        fill(&d, 0);
        SetLastError(0xdeadbeef);
        ret = pAlpha(d.dc, 0, 0, 4, 1, s.dc, 1, 0, 2, 1, bf(255, 0));
        check("ab stretch2", ret, &d, 1, want);
    }
    drop(&s); drop(&d); drop(&d16); drop(&d24); drop(&s24); drop(&s2); drop(&big);
}

static void test_TransparentBlt(void)
{
    BOOL ret;
    RGBQUAD pal[4] = { {0, 0, 0, 0}, {255, 0, 255, 0}, {0, 0, 255, 0}, {0, 255, 0, 0} };
    struct dib s = make(4, 1, 32, NULL, NULL, 0), d = make(4, 1, 32, NULL, NULL, 0);
    struct dib s8 = make(4, 1, 8, NULL, pal, 4), big = make(8, 2, 32, NULL, NULL, 0);
    struct dib d16 = make(4, 1, 16, m565, NULL, 0), s16 = make(4, 1, 16, m565, NULL, 0);
    struct dib s24 = make(4, 1, 24, NULL, NULL, 0);

    setpx(&s, 0, 0, 0xff00ff); setpx(&s, 1, 0, 0x00ff0000);
    setpx(&s, 2, 0, 0xffff00ff); setpx(&s, 3, 0, 0x0000ff);
    setpx(&s8, 0, 0, 1); setpx(&s8, 1, 0, 2); setpx(&s8, 2, 0, 3); setpx(&s8, 3, 0, 1);
    setpx(&s16, 0, 0, 0xf81f); setpx(&s16, 1, 0, 0x001f);
    setpx(&s16, 2, 0, 0xf800); setpx(&s16, 3, 0, 0xf81e);
    setpx(&s24, 0, 0, 0xff00ff); setpx(&s24, 1, 0, 0x123456);
    setpx(&s24, 2, 0, 0xfe00ff); setpx(&s24, 3, 0, 0);
    {
        static const DWORD want[] = {
            0x11111111, 0xff0000, 0x11111111, 0xff
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pTrans(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb32", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff00ff, 0xff0000, 0xffff00ff, 0x11111111
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pTrans(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, RGB(0, 0, 255));
        check("tb32 key2", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff00ff, 0xff0000, 0xffff00ff, 0xff
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pTrans(d.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, 0x01ff00ff);
        check("tb32 keyhi", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x22222222, 0x22222222, 0xff0000, 0xff0000, 0x22222222,
            0x22222222, 0xff, 0xff, 0x22222222, 0x22222222, 0xff0000,
            0xff0000, 0x22222222, 0x22222222, 0xff, 0xff
        };
        fill(&big, 0x22222222);
        SetLastError(0xdeadbeef);
        ret = pTrans(big.dc, 0, 0, 8, 2, s.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb stretch", ret, &big, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0xff0000, 0xff00, 0x11111111
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pTrans(d.dc, 0, 0, 4, 1, s8.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb8to32", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0x1234, 0x1f, 0xf800, 0xf81e
        };
        fill(&d16, 0x1234);
        SetLastError(0xdeadbeef);
        ret = pTrans(d16.dc, 0, 0, 4, 1, s16.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb16", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0xf81f, 0x1f, 0xf800, 0xf81e
        };
        fill(&d16, 0x1234);
        SetLastError(0xdeadbeef);
        ret = pTrans(d16.dc, 0, 0, 4, 1, s16.dc, 0, 0, 4, 1, RGB(248, 0, 248));
        check("tb16 key248", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0x1234, 0xf800, 0x1234, 0x1f
        };
        fill(&d16, 0x1234);
        SetLastError(0xdeadbeef);
        ret = pTrans(d16.dc, 0, 0, 4, 1, s.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb32to16", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x123456, 0xfe00ff, 0x0
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pTrans(d.dc, 0, 0, 4, 1, s24.dc, 0, 0, 4, 1, RGB(255, 0, 255));
        check("tb24to32", ret, &d, 1, want);
    }
    drop(&s); drop(&d); drop(&s8); drop(&big); drop(&d16); drop(&s16); drop(&s24);
}

static void test_GradientFill(void)
{
    BOOL ret;
    struct dib d = make(8, 1, 32, NULL, NULL, 0), v = make(1, 8, 32, NULL, NULL, 0);
    struct dib t = make(8, 8, 32, NULL, NULL, 0), d16 = make(8, 2, 16, m565, NULL, 0);
    struct dib d24 = make(8, 1, 24, NULL, NULL, 0), g565 = make(16, 4, 16, m565, NULL, 0);
    TRIVERTEX vx[4] = {
        { 0, 0, 0xff00, 0x0000, 0x0000, 0x0000 },
        { 8, 1, 0x0000, 0x0000, 0xff00, 0xff00 },
        { 0, 8, 0x0000, 0xff00, 0x0000, 0x8000 },
        { 8, 8, 0xffff, 0xffff, 0xffff, 0xffff } };
    TRIVERTEX gv[2] = {
        { 0, 0, 0x0000, 0x0000, 0x0000, 0x0000 },
        { 16, 4, 0x8000, 0xff00, 0x4000, 0xff00 } };
    /* the last two at one point, the fourth on the line from the first
       to the second */
    TRIVERTEX tv[5] = {
        { 1, 1, 0xff00, 0, 0, 0 }, { 5, 5, 0, 0xff00, 0, 0 }, { 7, 5, 0, 0, 0xff00, 0 },
        { 3, 3, 0xff00, 0xff00, 0, 0 }, { 1, 1, 0, 0, 0xff00, 0 } };
    GRADIENT_RECT gr = { 0, 1 }, back = { 1, 0 };
    GRADIENT_RECT rr[2] = { { 0, 1 }, { 2, 9 } };
    GRADIENT_TRIANGLE gt[2] = { { 0, 1, 2 }, { 1, 2, 3 } };
    GRADIENT_TRIANGLE tri[7] = { { 2, 1, 0 }, { 0, 1, 1 }, { 1, 1, 1 }, { 0, 3, 1 },
                                 { 1, 2, 1 }, { 0, 1, 2 }, { 0, 1, 1 } };

    {
        static const DWORD want[] = {
            0xff0000, 0x1fdf001f, 0x3fbf003f, 0x5f9f005f, 0x7f7f007f,
            0x9f5f009f, 0xbf3f00bf, 0xdf1f00df
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_H);
        check("gf h", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0xff0000, 0xff0000, 0xff0000, 0xff0000, 0xff0000,
            0xff0000, 0xff0000
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_V);
        check("gf v on row", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0x1fdf001f, 0x3fbf003f, 0x5f9f005f, 0x7f7f007f,
            0x9f5f009f, 0xbf3f00bf, 0xdf1f00df
        };
        fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d.dc, vx, 2, &back, 1, GRADIENT_FILL_RECT_H);
        check("gf h back", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0xdf001f, 0xbf003f, 0x9f005f, 0x7f007f, 0x5f009f,
            0x3f00bf, 0x1f00df
        };
        fill(&d24, 0x111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d24.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_H);
        check("gf h24", ret, &d24, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0x1fdf001f, 0x3fbf003f, 0x5f9f005f, 0x7f7f007f,
            0x9f5f009f, 0xbf3f00bf, 0xdf1f00df
        };
        vx[1].x = 1; vx[1].y = 8; fill(&v, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(v.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_V);
        check("gf v", ret, &v, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0x2ad4002a, 0x55aa0055, 0x7f7f007f, 0xaa5500aa,
            0xd42a00d4, 0x11111111, 0x11111111
        };
        vx[1].x = 6; vx[1].y = 1; fill(&d, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_H);
        check("gf h6", ret, &d, 1, want);
    }
    {
        static const DWORD want[] = {
            0xf800, 0xe004, 0xc008, 0xa00c, 0x780f, 0x6014, 0x4018, 0x201c,
            0xf800, 0xe004, 0xc008, 0xa00c, 0x8010, 0x6014, 0x4018, 0x201c
        };
        vx[1].x = 8; vx[1].y = 2; fill(&d16, 0x1111);
        SetLastError(0xdeadbeef);
        ret = pGrad(d16.dc, vx, 2, &gr, 1, GRADIENT_FILL_RECT_H);
        check("gf h565", ret, &d16, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0x1fdf001f, 0x3fbf003f, 0x5f9f005f, 0x7f7f007f,
            0x9f5f009f, 0xbf3f00bf, 0xdf1f00df, 0x10df1f00, 0x2fbf1f1f,
            0x4f9f1f3f, 0x6f7f1f5f, 0x8f5f1f7f, 0xaf3f1f9f, 0xcf1f1fbf,
            0x11111111, 0x20bf3f00, 0x3f9f3f1f, 0x5f7f3f3f, 0x7f5f3f5f,
            0x9f3f3f7f, 0xbf1f3f9f, 0x11111111, 0x11111111, 0x309f5f00,
            0x4f7f5f1f, 0x6f5f5f3f, 0x8f3f5f5f, 0xaf1f5f7f, 0x11111111,
            0x11111111, 0x11111111, 0x407f7f00, 0x5f5f7f1f, 0x7f3f7f3f,
            0x9f1f7f5f, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x505f9f00, 0x6f3f9f1f, 0x8f1f9f3f, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x603fbf00, 0x7f1fbf1f,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x701fdf00, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        vx[1].x = 8; vx[1].y = 0; vx[2].x = 0; vx[2].y = 8; fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, vx, 3, gt, 1, GRADIENT_FILL_TRIANGLE);
        check("gf tri", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0xff0000, 0x1fdf001f, 0x3fbf003f, 0x5f9f005f, 0x7f7f007f,
            0x9f5f009f, 0xbf3f00bf, 0xdf1f00df, 0x10df1f00, 0x2fbf1f1f,
            0x4f9f1f3f, 0x6f7f1f5f, 0x8f5f1f7f, 0xaf3f1f9f, 0xcf1f1fbf,
            0xef001fdf, 0x20bf3f00, 0x3f9f3f1f, 0x5f7f3f3f, 0x7f5f3f5f,
            0x9f3f3f7f, 0xbf1f3f9f, 0xdf003fbf, 0xef1f3fdf, 0x309f5f00,
            0x4f7f5f1f, 0x6f5f5f3f, 0x8f3f5f5f, 0xaf1f5f7f, 0xcf005f9f,
            0xdf1f5fbf, 0xef3f5fdf, 0x407f7f00, 0x5f5f7f1f, 0x7f3f7f3f,
            0x9f1f7f5f, 0xbf007f7f, 0xcf1f7f9f, 0xdf3f7fbf, 0xef5f7fdf,
            0x505f9f00, 0x6f3f9f1f, 0x8f1f9f3f, 0xaf009f5f, 0xbf1f9f7f,
            0xcf3f9f9f, 0xdf5f9fbf, 0xef7f9fdf, 0x603fbf00, 0x7f1fbf1f,
            0x9f00bf3f, 0xaf1fbf5f, 0xbf3fbf7f, 0xcf5fbf9f, 0xdf7fbfbf,
            0xef9fbfdf, 0x701fdf00, 0x8f00df1f, 0x9f1fdf3f, 0xaf3fdf5f,
            0xbf5fdf7f, 0xcf7fdf9f, 0xdf9fdfbf, 0xefbfdfdf
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, vx, 4, gt, 2, GRADIENT_FILL_TRIANGLE);
        check("gf tri2", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x36ba1d27, 0x67930962,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x3b9c4e13, 0x6c753a4e, 0x9d4e2789, 0xce2713c4,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x407f7f00,
            0x71586b3a, 0xa2315875, 0xd30944b0, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x763a9c27, 0xa7138962,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x7b1dcd13, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        vx[0].x = 1; vx[0].y = 1; vx[1].x = 6; vx[1].y = 3; vx[2].x = 3; vx[2].y = 7;
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, vx, 3, gt, 1, GRADIENT_FILL_TRIANGLE);
        check("gf tri3", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0x0, 0x881, 0x1101, 0x1982, 0x21e2, 0x2a83, 0x3303, 0x3b84,
            0x43e4, 0x4c85, 0x54e5, 0x5d86, 0x65e6, 0x6e87, 0x76e7, 0x7f88,
            0x0, 0x880, 0x1101, 0x1981, 0x2202, 0x2a82, 0x3303, 0x3b83,
            0x4404, 0x4c84, 0x5505, 0x5d85, 0x6606, 0x6e86, 0x7707, 0x7f87,
            0x0, 0x881, 0x1101, 0x1982, 0x2202, 0x2a83, 0x32e3, 0x3b84,
            0x4404, 0x4c85, 0x54e5, 0x5d86, 0x6606, 0x6e87, 0x76e7, 0x7f88,
            0x0, 0x880, 0x1101, 0x1981, 0x2202, 0x2a82, 0x3303, 0x3b83,
            0x4404, 0x4c84, 0x5505, 0x5d85, 0x6606, 0x6e86, 0x7707, 0x7f87
        };
        fill(&g565, 0);
        SetLastError(0xdeadbeef);
        ret = pGrad(g565.dc, gv, 2, &gr, 1, GRADIENT_FILL_RECT_H);
        check("gf2 565", ret, &g565, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0xbf3f00, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x7f7f00, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x3fbf00, 0x3f3f7f, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, &tri[0], 1, GRADIENT_FILL_TRIANGLE);
        check("t210", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, &tri[1], 1, GRADIENT_FILL_TRIANGLE);
        check("t011", ret, &t, 0, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, &tri[2], 1, GRADIENT_FILL_TRIANGLE);
        check("t111", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 5, &tri[3], 1, GRADIENT_FILL_TRIANGLE);
        check("tline", ret, &t, 0, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, &tri[4], 1, GRADIENT_FILL_TRIANGLE);
        check("tflat", ret, &t, 1, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0xbf3f00, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x7f7f00, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x3fbf00, 0x3f3f7f, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, &tri[5], 2, GRADIENT_FILL_TRIANGLE);
        check("t2mesh", ret, &t, 0, want);
    }
    {
        static const DWORD want[] = {
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0xff0000,
            0xbf3f00, 0x7f7f00, 0x3fbf00, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0xff0000, 0xbf3f00, 0x7f7f00, 0x3fbf00,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0xff0000,
            0xbf3f00, 0x7f7f00, 0x3fbf00, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0xff0000, 0xbf3f00, 0x7f7f00, 0x3fbf00,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111, 0x11111111, 0x11111111, 0x11111111,
            0x11111111, 0x11111111
        };
        fill(&t, 0x11111111);
        SetLastError(0xdeadbeef);
        ret = pGrad(t.dc, tv, 3, rr, 2, GRADIENT_FILL_RECT_H);
        check("r2bad", ret, &t, 0, want);
    }
    drop(&d); drop(&v); drop(&t); drop(&d16); drop(&d24); drop(&g565);
}

START_TEST(pixels)
{
    HMODULE msimg32 = LoadLibraryA("msimg32.dll");

    if (!msimg32)
    {
        win_skip("There is no MSIMG32.DLL\n");
        return;
    }
    pAlpha = (ALPHA)GetProcAddress(msimg32, "AlphaBlend");
    pTrans = (TRANS)GetProcAddress(msimg32, "TransparentBlt");
    pGrad = (GRAD)GetProcAddress(msimg32, "GradientFill");
    test_AlphaBlend();
    test_TransparentBlt();
    test_GradientFill();
}
