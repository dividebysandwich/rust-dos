/*
 * Unit test suite for fonts
 *
 * Copyright 2002 Mike McCormack
 * Copyright 2004 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/font.c: CreateFontIndirect
 * and what GetObject gives back of Windows 3.1's LOGFONT (16-bit
 * heights, widths, angles and weight); how GDI scales the metrics of a
 * bitmap font, through GetTextMetrics, GetTextExtentPoint and
 * GetCharWidth; the metrics of the bitmap fonts for the ANSI character
 * set; GetTextExtentPoint against GetTextMetrics; GetTextFace; and the
 * widest TrueType Arial CreateFont makes. The fonts are found with
 * EnumFontFamilies. Left out are the tests of what Windows 3.1 doesn't
 * have: the Unicode functions, EnumFontFamiliesEx,
 * TranslateCharsetInfo, GetCharABCWidthsI, GetGlyphIndices,
 * GetFontUnicodeRanges, GetTextExtentExPoint, GetTextCharset,
 * GetCurrentObject, GdiGetCharDimensions, GdiRealizationInfo and
 * CreateFontIndirectEx, and of the fonts other than the ANSI ones;
 * the tests of TrueType's outline metrics, kerning pairs, ABC widths
 * and glyph outlines aren't ported yet, and those that need Tahoma,
 * which Windows 3.1 doesn't have, are left out (test_GetTextFace uses
 * Arial instead).
 * test_bitmap_font, which only traces it in Wine, skips when there is
 * no System font; test_bitmap_font_metrics skips when it finds none of
 * its fonts. GetTextExtent, which returns the size in a DWORD, is
 * checked against GetTextExtentPoint, and a string's extent against
 * GetCharWidth's widths, which Wine's test doesn't do.
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

/* Do not allow more than 1 deviation here */
#define match_off_by_1(a, b) (iabs((a) - (b)) <= 1)

#define near_match(a, b) (iabs((a) - (b)) <= 6)

#ifndef TRUETYPE_FONTTYPE
#define TRUETYPE_FONTTYPE 4
#endif

/* The font enumeration callbacks load their own data segment, as
 * they're called from GDI with its own. */
#define ENUMPROC int FAR PASCAL __loadds

static ENUMPROC is_truetype_font_installed_proc(const LOGFONT FAR *elf, const TEXTMETRIC FAR *ntm, int type, LPARAM lParam)
{
    if (type != TRUETYPE_FONTTYPE) return 1;

    return 0;
}

static BOOL is_truetype_font_installed(const char *name)
{
    HDC hdc = GetDC(0);
    BOOL ret = FALSE;

    if (!EnumFontFamilies(hdc, name, (FONTENUMPROC)is_truetype_font_installed_proc, NULL))
        ret = TRUE;

    ReleaseDC(0, hdc);
    return ret;
}

static ENUMPROC is_font_installed_proc(const LOGFONT FAR *elf, const TEXTMETRIC FAR *ntm, int type, LPARAM lParam)
{
    return 0;
}

static BOOL is_font_installed(const char *name)
{
    HDC hdc = GetDC(0);
    BOOL ret = FALSE;

    if(!EnumFontFamilies(hdc, name, (FONTENUMPROC)is_font_installed_proc, NULL))
        ret = TRUE;

    ReleaseDC(0, hdc);
    return ret;
}

static void check_font(const char* test, const LOGFONT* lf, HFONT hfont)
{
    LOGFONT getobj_lf;
    int ret, minlen = 0;

    if (!hfont)
        return;

    ret = GetObject(hfont, sizeof(getobj_lf), &getobj_lf);
    /* NT4 tries to be clever and only returns the minimum length */
    while (lf->lfFaceName[minlen] && minlen < LF_FACESIZE-1)
        minlen++;
    minlen += (int)(((char *)lf->lfFaceName) - ((char *)lf)) + 1;
    ok(ret == sizeof(LOGFONT) || ret == minlen, "%s: GetObject returned %d\n", test, ret);
    ok(lf->lfHeight == getobj_lf.lfHeight,
       "lfHeight: expect %04x got %04x\n", lf->lfHeight, getobj_lf.lfHeight);
    ok(lf->lfWidth == getobj_lf.lfWidth,
       "lfWidth: expect %04x got %04x\n", lf->lfWidth, getobj_lf.lfWidth);
    ok(lf->lfEscapement == getobj_lf.lfEscapement,
       "lfEscapement: expect %04x got %04x\n", lf->lfEscapement, getobj_lf.lfEscapement);
    ok(lf->lfOrientation == getobj_lf.lfOrientation,
       "lfOrientation: expect %04x got %04x\n", lf->lfOrientation, getobj_lf.lfOrientation);
    ok(lf->lfWeight == getobj_lf.lfWeight,
       "lfWeight: expect %04x got %04x\n", lf->lfWeight, getobj_lf.lfWeight);
    ok(lf->lfItalic == getobj_lf.lfItalic, "lfItalic: expect %02x got %02x\n", lf->lfItalic, getobj_lf.lfItalic);
    ok(lf->lfUnderline == getobj_lf.lfUnderline, "lfUnderline: expect %02x got %02x\n", lf->lfUnderline, getobj_lf.lfUnderline);
    ok(lf->lfStrikeOut == getobj_lf.lfStrikeOut, "lfStrikeOut: expect %02x got %02x\n", lf->lfStrikeOut, getobj_lf.lfStrikeOut);
    ok(lf->lfCharSet == getobj_lf.lfCharSet, "lfCharSet: expect %02x got %02x\n", lf->lfCharSet, getobj_lf.lfCharSet);
    ok(lf->lfOutPrecision == getobj_lf.lfOutPrecision, "lfOutPrecision: expect %02x got %02x\n", lf->lfOutPrecision, getobj_lf.lfOutPrecision);
    ok(lf->lfClipPrecision == getobj_lf.lfClipPrecision, "lfClipPrecision: expect %02x got %02x\n", lf->lfClipPrecision, getobj_lf.lfClipPrecision);
    ok(lf->lfQuality == getobj_lf.lfQuality, "lfQuality: expect %02x got %02x\n", lf->lfQuality, getobj_lf.lfQuality);
    ok(lf->lfPitchAndFamily == getobj_lf.lfPitchAndFamily, "lfPitchAndFamily: expect %02x got %02x\n", lf->lfPitchAndFamily, getobj_lf.lfPitchAndFamily);
    ok(!lstrcmp(lf->lfFaceName, getobj_lf.lfFaceName) ||
       broken(!memcmp(lf->lfFaceName, getobj_lf.lfFaceName, LF_FACESIZE-1)), /* win9x doesn't ensure '\0' termination */
       "%s: font names don't match: %s != %s\n", test, (LPSTR)lf->lfFaceName, (LPSTR)getobj_lf.lfFaceName);
}

static HFONT create_font(const char* test, const LOGFONT* lf)
{
    HFONT hfont = CreateFontIndirect(lf);
    ok(hfont != 0, "%s: CreateFontIndirect failed\n", test);
    if (hfont)
        check_font(test, lf, hfont);
    return hfont;
}

static void test_logfont(void)
{
    LOGFONT lf;
    HFONT hfont;

    memset(&lf, 0, sizeof lf);

    lf.lfCharSet = ANSI_CHARSET;
    lf.lfClipPrecision = CLIP_DEFAULT_PRECIS;
    lf.lfWeight = FW_DONTCARE;
    lf.lfHeight = 16;
    lf.lfWidth = 16;
    lf.lfQuality = DEFAULT_QUALITY;

    lstrcpy(lf.lfFaceName, "Arial");
    hfont = create_font("Arial", &lf);
    DeleteObject(hfont);

    memset(&lf, 'A', sizeof(lf));
    hfont = CreateFontIndirect(&lf);
    ok(hfont != 0, "CreateFontIndirect with strange LOGFONT failed\n");

    lf.lfFaceName[LF_FACESIZE - 1] = 0;
    check_font("AAA...", &lf, hfont);
    DeleteObject(hfont);

    /* Negative and large 16-bit values come back as they went in */
    memset(&lf, 0, sizeof lf);
    lf.lfHeight = -1234;
    lf.lfWidth = -56;
    lf.lfEscapement = 2700;
    lf.lfOrientation = -900;
    lf.lfWeight = FW_BOLD;
    lf.lfItalic = 1;
    lf.lfUnderline = 1;
    lf.lfStrikeOut = 1;
    lf.lfCharSet = OEM_CHARSET;
    lf.lfOutPrecision = OUT_STROKE_PRECIS;
    lf.lfClipPrecision = CLIP_STROKE_PRECIS;
    lf.lfQuality = PROOF_QUALITY;
    lf.lfPitchAndFamily = FIXED_PITCH | FF_MODERN;
    lstrcpy(lf.lfFaceName, "Courier");
    hfont = create_font("negative", &lf);
    DeleteObject(hfont);
}

static ENUMPROC font_enum_proc(const LOGFONT FAR *elf, const TEXTMETRIC FAR *ntm, int type, LPARAM lParam)
{
    if (type & RASTER_FONTTYPE)
    {
        LOGFONT FAR *lf = (LOGFONT FAR *)lParam;
        *lf = *elf;
        return 0; /* stop enumeration */
    }

    return 1; /* continue enumeration */
}

static void test_font_metrics(HDC hdc, HFONT hfont, int lfHeight,
                              int lfWidth, const char *test_str,
                              int test_str_len, const TEXTMETRIC *tm_orig,
                              const SIZE *size_orig, int width_of_A_orig,
                              int scale_x, int scale_y)
{
    LOGFONT lf;
    TEXTMETRIC tm;
    SIZE size;
    int width_of_A, cx, cy;
    UINT ret;

    if (!hfont)
        return;

    GetObject(hfont, sizeof(lf), &lf);

    ret = GetTextMetrics(hdc, &tm);
    ok(ret, "GetTextMetrics failed\n");

    cx = tm.tmAveCharWidth / tm_orig->tmAveCharWidth;
    cy = tm.tmHeight / tm_orig->tmHeight;
    ok(cx == scale_x && cy == scale_y, "height %d: expected scale_x %d, scale_y %d, got cx %d, cy %d\n",
       lfHeight, scale_x, scale_y, cx, cy);
    ok(tm.tmHeight == tm_orig->tmHeight * scale_y, "height %d != %d\n", tm.tmHeight, tm_orig->tmHeight * scale_y);
    ok(tm.tmAscent == tm_orig->tmAscent * scale_y, "ascent %d != %d\n", tm.tmAscent, tm_orig->tmAscent * scale_y);
    ok(tm.tmDescent == tm_orig->tmDescent * scale_y, "descent %d != %d\n", tm.tmDescent, tm_orig->tmDescent * scale_y);
    ok(near_match(tm.tmAveCharWidth, tm_orig->tmAveCharWidth * scale_x), "ave width %d != %d\n", tm.tmAveCharWidth, tm_orig->tmAveCharWidth * scale_x);
    ok(near_match(tm.tmMaxCharWidth, tm_orig->tmMaxCharWidth * scale_x), "max width %d != %d\n", tm.tmMaxCharWidth, tm_orig->tmMaxCharWidth * scale_x);

    ok(lf.lfHeight == lfHeight, "lfHeight %d != %d\n", lf.lfHeight, lfHeight);
    if (lf.lfHeight)
    {
        if (lf.lfWidth)
            ok(lf.lfWidth == tm.tmAveCharWidth, "lfWidth %d != tm %d\n", lf.lfWidth, tm.tmAveCharWidth);
    }
    else
        ok(lf.lfWidth == lfWidth, "lfWidth %d != %d\n", lf.lfWidth, lfWidth);

    GetTextExtentPoint(hdc, test_str, test_str_len, &size);

    ok(near_match(size.cx, size_orig->cx * scale_x), "cx %d != %d\n", size.cx, size_orig->cx * scale_x);
    ok(size.cy == size_orig->cy * scale_y, "cy %d != %d\n", size.cy, size_orig->cy * scale_y);

    GetCharWidth(hdc, 'A', 'A', &width_of_A);

    ok(near_match(width_of_A, width_of_A_orig * scale_x), "width A %d != %d\n", width_of_A, width_of_A_orig * scale_x);
}

/* Test how GDI scales bitmap font metrics */
static void test_bitmap_font(void)
{
    static const char test_str[11] = "Test String";
    HDC hdc;
    LOGFONT bitmap_lf;
    HFONT hfont, old_hfont;
    TEXTMETRIC tm_orig;
    SIZE size_orig;
    int ret, i, width_orig, height_orig, scale, lfWidth;

    hdc = GetDC(0);

    /* "System" has only 1 pixel size defined, otherwise the test breaks */
    ret = EnumFontFamilies(hdc, "System", (FONTENUMPROC)font_enum_proc, (LPSTR)&bitmap_lf);
    if (ret)
    {
        ReleaseDC(0, hdc);
        skip("no bitmap fonts were found, skipping the test\n");
        return;
    }

    trace("found bitmap font %s, height %d\n", (LPSTR)bitmap_lf.lfFaceName, bitmap_lf.lfHeight);

    height_orig = bitmap_lf.lfHeight;
    lfWidth = bitmap_lf.lfWidth;

    hfont = create_font("bitmap", &bitmap_lf);
    old_hfont = SelectObject(hdc, hfont);
    ok(GetTextMetrics(hdc, &tm_orig), "GetTextMetrics failed\n");
    ok(GetTextExtentPoint(hdc, test_str, sizeof(test_str), &size_orig), "GetTextExtentPoint failed\n");
    ok(GetCharWidth(hdc, 'A', 'A', &width_orig), "GetCharWidth failed\n");
    SelectObject(hdc, old_hfont);
    DeleteObject(hfont);

    bitmap_lf.lfHeight = 0;
    bitmap_lf.lfWidth = 4;
    hfont = create_font("bitmap", &bitmap_lf);
    old_hfont = SelectObject(hdc, hfont);
    test_font_metrics(hdc, hfont, 0, 4, test_str, sizeof(test_str), &tm_orig, &size_orig, width_orig, 1, 1);
    SelectObject(hdc, old_hfont);
    DeleteObject(hfont);

    bitmap_lf.lfHeight = height_orig;
    bitmap_lf.lfWidth = lfWidth;

    /* test fractional scaling */
    for (i = 1; i <= height_orig * 6; i++)
    {
        int nearest_height;

        bitmap_lf.lfHeight = i;
        hfont = create_font("fractional", &bitmap_lf);
        scale = (i + height_orig - 1) / height_orig;
        nearest_height = scale * height_orig;
        /* Only jump to the next height if the difference <= 25% original height */
        if (scale > 2 && nearest_height - i > height_orig / 4) scale--;
        /* The jump between unscaled and doubled is delayed by 1 in winnt+ but not in win9x,
           so we'll not test this particular height. */
        else if(scale == 2 && nearest_height - i == (height_orig / 4)) { DeleteObject(hfont); continue; }
        else if(scale == 2 && nearest_height - i > (height_orig / 4 - 1)) scale--;
        old_hfont = SelectObject(hdc, hfont);
        test_font_metrics(hdc, hfont, bitmap_lf.lfHeight, 0, test_str, sizeof(test_str), &tm_orig, &size_orig, width_orig, 1, scale);
        SelectObject(hdc, old_hfont);
        DeleteObject(hfont);
    }

    /* test integer scaling 3x2 */
    bitmap_lf.lfHeight = height_orig * 2;
    bitmap_lf.lfWidth *= 3;
    hfont = create_font("3x2", &bitmap_lf);
    old_hfont = SelectObject(hdc, hfont);
    test_font_metrics(hdc, hfont, bitmap_lf.lfHeight, 0, test_str, sizeof(test_str), &tm_orig, &size_orig, width_orig, 3, 2);
    SelectObject(hdc, old_hfont);
    DeleteObject(hfont);

    /* test integer scaling 3x3 */
    bitmap_lf.lfHeight = height_orig * 3;
    bitmap_lf.lfWidth = 0;
    hfont = create_font("3x3", &bitmap_lf);
    old_hfont = SelectObject(hdc, hfont);
    test_font_metrics(hdc, hfont, bitmap_lf.lfHeight, 0, test_str, sizeof(test_str), &tm_orig, &size_orig, width_orig, 3, 3);
    SelectObject(hdc, old_hfont);
    DeleteObject(hfont);

    ReleaseDC(0, hdc);
}

static ENUMPROC find_font_proc(const LOGFONT FAR *elf, const TEXTMETRIC FAR *ntm, int type, LPARAM lParam)
{
    LOGFONT FAR *lf = (LOGFONT FAR *)lParam;

    if (elf->lfHeight == lf->lfHeight && !lstrcmp(elf->lfFaceName, lf->lfFaceName))
    {
        *lf = *elf;
        return 0; /* stop enumeration */
    }
    return 1; /* continue enumeration */
}

/* The rows of Wine's table for the ANSI character set (FS_LATIN1). */
static void test_bitmap_font_metrics(void)
{
    static const struct font_data
    {
        const char face_name[LF_FACESIZE];
        int weight, height, ascent, descent, int_leading, ext_leading;
        int ave_char_width, max_char_width, dpi;
    } fd[] =
    {
        { "MS Sans Serif", FW_NORMAL, 13, 11, 2, 2, 0, 5, 11, 96 },
        { "MS Sans Serif", FW_NORMAL, 16, 13, 3, 3, 0, 7, 14, 96 },
        { "MS Sans Serif", FW_NORMAL, 20, 16, 4, 4, 0, 8, 16, 96 },
        { "MS Sans Serif", FW_NORMAL, 24, 19, 5, 6, 0, 9, 19, 96 },
        { "MS Sans Serif", FW_NORMAL, 29, 23, 6, 5, 0, 12, 24, 96 },
        { "MS Sans Serif", FW_NORMAL, 37, 29, 8, 5, 0, 16, 32, 96 },

        { "MS Sans Serif", FW_NORMAL, 16, 13, 3, 3, 0, 7, 14, 120 },
        { "MS Sans Serif", FW_NORMAL, 20, 16, 4, 4, 0, 8, 18, 120 },
        { "MS Sans Serif", FW_NORMAL, 25, 20, 5, 5, 0, 10, 21, 120 },
        { "MS Sans Serif", FW_NORMAL, 29, 23, 6, 6, 0, 12, 24, 120 },
        { "MS Sans Serif", FW_NORMAL, 36, 29, 7, 6, 0, 15, 30, 120 },
        { "MS Sans Serif", FW_NORMAL, 46, 37, 9, 6, 0, 20, 40, 120 },

        { "MS Serif", FW_NORMAL, 10, 8, 2, 2, 0, 4, 8, 96 },
        { "MS Serif", FW_NORMAL, 11, 9, 2, 2, 0, 5, 9, 96 },
        { "MS Serif", FW_NORMAL, 13, 11, 2, 2, 0, 5, 11, 96 },
        { "MS Serif", FW_NORMAL, 16, 13, 3, 3, 0, 6, 14, 96 },
        { "MS Serif", FW_NORMAL, 19, 15, 4, 3, 0, 8, 18, 96 },
        { "MS Serif", FW_NORMAL, 21, 16, 5, 3, 0, 9, 17, 96 },
        { "MS Serif", FW_NORMAL, 27, 21, 6, 3, 0, 12, 23, 96 },
        { "MS Serif", FW_NORMAL, 35, 27, 8, 3, 0, 16, 33, 96 },

        { "MS Serif", FW_NORMAL, 16, 13, 3, 3, 0, 6, 14, 120 },
        { "MS Serif", FW_NORMAL, 20, 16, 4, 4, 0, 8, 18, 120 },
        { "MS Serif", FW_NORMAL, 23, 18, 5, 3, 0, 10, 21, 120 },
        { "MS Serif", FW_NORMAL, 27, 21, 6, 4, 0, 12, 23, 120 },
        { "MS Serif", FW_NORMAL, 33, 26, 7, 3, 0, 14, 30, 120 },
        { "MS Serif", FW_NORMAL, 43, 34, 9, 3, 0, 19, 39, 120 },

        { "Courier", FW_NORMAL, 13, 11, 2, 0, 0, 8, 8, 96 },
        { "Courier", FW_NORMAL, 16, 13, 3, 0, 0, 9, 9, 96 },
        { "Courier", FW_NORMAL, 20, 16, 4, 0, 0, 12, 12, 96 },

        { "Courier", FW_NORMAL, 16, 13, 3, 0, 0, 9, 9, 120 },
        { "Courier", FW_NORMAL, 20, 16, 4, 0, 0, 12, 12, 120 },
        { "Courier", FW_NORMAL, 25, 20, 5, 0, 0, 15, 15, 120 },

        { "System", FW_BOLD, 16, 13, 3, 3, 0, 7, 14, 96 },

        { "System", FW_BOLD, 20, 16, 4, 4, 0, 9, 14, 120 },

        { "Small Fonts", FW_NORMAL, 3, 2, 1, 0, 0, 1, 2, 96 },
        { "Small Fonts", FW_NORMAL, 5, 4, 1, 1, 0, 3, 4, 96 },
        { "Small Fonts", FW_NORMAL, 6, 5, 1, 1, 0, 3, 13, 96 },
        { "Small Fonts", FW_NORMAL, 8, 7, 1, 1, 0, 4, 7, 96 },
        { "Small Fonts", FW_NORMAL, 10, 8, 2, 2, 0, 4, 8, 96 },
        { "Small Fonts", FW_NORMAL, 11, 9, 2, 2, 0, 5, 9, 96 },

        { "Small Fonts", FW_NORMAL, 3, 2, 1, 0, 0, 1, 2, 120 },
        { "Small Fonts", FW_NORMAL, 6, 5, 1, 1, 0, 3, 5, 120 },
        { "Small Fonts", FW_NORMAL, 8, 7, 1, 1, 0, 4, 7, 120 },
        { "Small Fonts", FW_NORMAL, 10, 8, 2, 2, 0, 5, 9, 120 },
        { "Small Fonts", FW_NORMAL, 12, 10, 2, 2, 0, 5, 10, 120 },
        { "Small Fonts", FW_NORMAL, 13, 11, 2, 2, 0, 6, 12, 120 },

        { "Fixedsys", FW_NORMAL, 15, 12, 3, 3, 0, 8, 8, 96 },

        /* The 120dpi version still has its dpi marked as 96 */
        { "Fixedsys", FW_NORMAL, 20, 16, 4, 2, 0, 10, 10, 96 }
    };
    HDC hdc;
    LOGFONT lf;
    HFONT hfont, old_hfont;
    TEXTMETRIC tm;
    int ret, i, found = 0;

    hdc = CreateCompatibleDC(0);
    ok(hdc != 0, "CreateCompatibleDC failed\n");

    for (i = 0; i < sizeof(fd)/sizeof(fd[0]); i++)
    {
        memset(&lf, 0, sizeof(lf));

        lf.lfHeight = fd[i].height;
        lstrcpy(lf.lfFaceName, fd[i].face_name);
        lf.lfCharSet = ANSI_CHARSET;

        ret = EnumFontFamilies(hdc, lf.lfFaceName, (FONTENUMPROC)find_font_proc, (LPSTR)&lf);
        if (ret) continue;
        found++;

        hfont = create_font(lf.lfFaceName, &lf);
        old_hfont = SelectObject(hdc, hfont);
        ok(GetTextMetrics(hdc, &tm), "GetTextMetrics failed\n");
        if(fd[i].dpi == tm.tmDigitizedAspectX)
        {
            trace("found font %s, height %d charset %x dpi %d\n", (LPSTR)lf.lfFaceName, lf.lfHeight, lf.lfCharSet, fd[i].dpi);
            ok(tm.tmWeight == fd[i].weight, "%s(%d): tm.tmWeight %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmWeight, fd[i].weight);
            ok(tm.tmHeight == fd[i].height, "%s(%d): tm.tmHeight %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmHeight, fd[i].height);
            ok(tm.tmAscent == fd[i].ascent, "%s(%d): tm.tmAscent %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmAscent, fd[i].ascent);
            ok(tm.tmDescent == fd[i].descent, "%s(%d): tm.tmDescent %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmDescent, fd[i].descent);
            ok(tm.tmInternalLeading == fd[i].int_leading, "%s(%d): tm.tmInternalLeading %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmInternalLeading, fd[i].int_leading);
            ok(tm.tmExternalLeading == fd[i].ext_leading, "%s(%d): tm.tmExternalLeading %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmExternalLeading, fd[i].ext_leading);
            ok(tm.tmAveCharWidth == fd[i].ave_char_width, "%s(%d): tm.tmAveCharWidth %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmAveCharWidth, fd[i].ave_char_width);

            /* Don't run the max char width test on System/ANSI_CHARSET.  We have extra characters in our font
               that make the max width bigger */
            if(lstrcmp(lf.lfFaceName, "System") || lf.lfCharSet != ANSI_CHARSET)
                ok(tm.tmMaxCharWidth == fd[i].max_char_width, "%s(%d): tm.tmMaxCharWidth %d != %d\n", (LPSTR)fd[i].face_name, fd[i].height, tm.tmMaxCharWidth, fd[i].max_char_width);
        }
        SelectObject(hdc, old_hfont);
        DeleteObject(hfont);
    }
    if (!found)
        skip("none of the bitmap fonts were found\n");

    DeleteDC(hdc);
}

static void test_text_extents(void)
{
    LOGFONT lf;
    TEXTMETRIC tm;
    HDC hdc;
    HFONT hfont;
    SIZE sz, sz2;
    DWORD dw;
    int widths[3], i;

    memset(&lf, 0, sizeof(lf));
    lstrcpy(lf.lfFaceName, "Arial");
    lf.lfHeight = 20;

    hfont = CreateFontIndirect(&lf);
    hdc = GetDC(0);
    hfont = SelectObject(hdc, hfont);
    GetTextMetrics(hdc, &tm);
    GetTextExtentPoint(hdc, "o", 1, &sz);
    ok(sz.cy == tm.tmHeight, "cy %d tmHeight %d\n", sz.cy, tm.tmHeight);

    /* GetTextExtent gives the same size in a DWORD */
    dw = GetTextExtent(hdc, "o", 1);
    ok(LOWORD(dw) == sz.cx && HIWORD(dw) == sz.cy,
       "GetTextExtent gave %08lx, GetTextExtentPoint %d x %d\n", dw, sz.cx, sz.cy);
    ok(GetTextExtentPoint(hdc, "Two words", 9, &sz), "GetTextExtentPoint failed\n");
    dw = GetTextExtent(hdc, "Two words", 9);
    ok(LOWORD(dw) == sz.cx && HIWORD(dw) == sz.cy,
       "GetTextExtent gave %08lx, GetTextExtentPoint %d x %d\n", dw, sz.cx, sz.cy);
    ok(sz.cx > 0, "Two words have no width\n");

    /* The extent of a string is the sum of its characters' widths */
    ok(GetCharWidth(hdc, 'a', 'c', widths), "GetCharWidth failed\n");
    for (i = 0; i < 3; i++)
        ok(widths[i] > 0 && widths[i] <= tm.tmMaxCharWidth,
           "width of %c is %d\n", 'a' + i, widths[i]);
    ok(GetTextExtentPoint(hdc, "abc", 3, &sz2), "GetTextExtentPoint failed\n");
    ok(sz2.cx == widths[0] + widths[1] + widths[2] - 2 * tm.tmOverhang ||
       sz2.cx == widths[0] + widths[1] + widths[2],
       "abc is %d wide, its characters %d, %d and %d\n", sz2.cx, widths[0], widths[1], widths[2]);

    hfont = SelectObject(hdc, hfont);
    DeleteObject(hfont);
    ReleaseDC(NULL, hdc);
}

static void test_GetTextFace(void)
{
    static const char faceA[] = "Arial";
    LOGFONT fA;
    char bufA[LF_FACESIZE];
    HFONT f, g;
    HDC dc;
    int n;

    if(!is_font_installed("Arial"))
    {
        skip("Arial is not installed so skipping this test\n");
        return;
    }

    memset(&fA, 0, sizeof(fA));
    memcpy(fA.lfFaceName, faceA, sizeof faceA);
    f = CreateFontIndirect(&fA);
    ok(f != NULL, "CreateFontIndirect failed\n");

    dc = GetDC(NULL);
    g = SelectObject(dc, f);
    n = GetTextFace(dc, sizeof bufA, bufA);
    ok(n == sizeof faceA - 1, "GetTextFace returned %d\n", n);
    ok(lstrcmp(faceA, bufA) == 0, "GetTextFace gave %s\n", (LPSTR)bufA);

    /* Play with the count arg.  */
    bufA[0] = 'x';
    n = GetTextFace(dc, 0, bufA);
    ok(n == 0, "GetTextFace returned %d\n", n);
    ok(bufA[0] == 'x', "GetTextFace buf[0] == %d\n", bufA[0]);

    bufA[0] = 'x';
    n = GetTextFace(dc, 1, bufA);
    ok(n == 0, "GetTextFace returned %d\n", n);
    ok(bufA[0] == '\0', "GetTextFace buf[0] == %d\n", bufA[0]);

    bufA[0] = 'x'; bufA[1] = 'y';
    n = GetTextFace(dc, 2, bufA);
    ok(n == 1, "GetTextFace returned %d\n", n);
    ok(bufA[0] == faceA[0] && bufA[1] == '\0', "GetTextFace didn't copy\n");

    DeleteObject(SelectObject(dc, g));
    ReleaseDC(NULL, dc);
}

static void test_GetTextMetrics2(const char *fontname, int font_height)
{
    HFONT of, hf;
    HDC hdc;
    TEXTMETRIC tm;
    BOOL ret;
    int ave_width, height, width, ratio, scale;

    if (!is_truetype_font_installed( fontname)) {
        skip("%s is not installed\n", (LPSTR)fontname);
        return;
    }
    hdc = CreateCompatibleDC(0);
    ok( hdc != NULL, "CreateCompatibleDC failed\n");
    /* select width = 0 */
    hf = CreateFont(font_height, 0, 0, 0, FW_REGULAR, FALSE, FALSE, FALSE,
            DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_LH_ANGLES,
            DEFAULT_QUALITY, VARIABLE_PITCH,
            fontname);
    ok( hf != NULL, "CreateFont(%s, %d) failed\n", (LPSTR)fontname, font_height);
    of = SelectObject( hdc, hf);
    ret = GetTextMetrics( hdc, &tm);
    ok(ret, "GetTextMetrics failed\n");
    height = tm.tmHeight;
    ave_width = tm.tmAveCharWidth;
    SelectObject( hdc, of);
    DeleteObject( hf);

    trace("height %d, ave width %d\n", height, ave_width);

    for (width = ave_width * 2; /* nothing*/; width += ave_width)
    {
        hf = CreateFont(height, width, 0, 0, FW_REGULAR, FALSE, FALSE, FALSE,
                        DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_LH_ANGLES,
                        DEFAULT_QUALITY, VARIABLE_PITCH, fontname);
        ok(hf != 0, "CreateFont failed\n");
        of = SelectObject(hdc, hf);
        ret = GetTextMetrics(hdc, &tm);
        ok(ret, "GetTextMetrics failed\n");
        SelectObject(hdc, of);
        DeleteObject(hf);

        if (match_off_by_1(tm.tmAveCharWidth, ave_width) || width / height > 200)
            break;
    }

    DeleteDC(hdc);

    ratio = width / height;
    scale = width / ave_width;

    trace("max width/height ratio (%d / %d) %d, max width scale (%d / %d) %d\n",
          width, height, ratio, width, ave_width, scale);

    ok(ratio >= 90 && ratio <= 110, "expected width/height ratio 90-110, got %d\n", ratio);
}

static void test_CreateFontIndirect(void)
{
    LOGFONT lf, getobj_lf;
    int ret, i;
    HFONT hfont;
    static char TestName[][16] = {"Arial", "Arial Bold", "Arial Italic", "Arial Baltic"};

    memset(&lf, 0, sizeof(lf));
    lf.lfCharSet = ANSI_CHARSET;
    lf.lfClipPrecision = CLIP_DEFAULT_PRECIS;
    lf.lfHeight = 16;
    lf.lfWidth = 16;
    lf.lfQuality = DEFAULT_QUALITY;
    lf.lfItalic = FALSE;
    lf.lfWeight = FW_DONTCARE;

    for (i = 0; i < sizeof(TestName)/sizeof(TestName[0]); i++)
    {
        lstrcpy(lf.lfFaceName, TestName[i]);
        hfont = CreateFontIndirect(&lf);
        ok(hfont != 0, "CreateFontIndirect failed\n");
        ret = GetObject(hfont, sizeof(getobj_lf), &getobj_lf);
        ok(ret, "GetObject failed\n");
        ok(lf.lfItalic == getobj_lf.lfItalic, "lfItalic: expect %02x got %02x\n", lf.lfItalic, getobj_lf.lfItalic);
        ok(lf.lfWeight == getobj_lf.lfWeight,
           "lfWeight: expect %04x got %04x\n", lf.lfWeight, getobj_lf.lfWeight);
        ok(!lstrcmp(lf.lfFaceName, getobj_lf.lfFaceName) ||
           broken(!memcmp(lf.lfFaceName, getobj_lf.lfFaceName, LF_FACESIZE-1)), /* win9x doesn't ensure '\0' termination */
           "font names don't match: %s != %s\n", (LPSTR)lf.lfFaceName, (LPSTR)getobj_lf.lfFaceName);
        DeleteObject(hfont);
    }
}

START_TEST(font)
{
    test_logfont();
    test_bitmap_font();
    test_bitmap_font_metrics();
    test_text_extents();
    test_GetTextFace();
    test_GetTextMetrics2("Arial", -11);
    test_GetTextMetrics2("Arial", -55);
    test_GetTextMetrics2("Arial", -110);
    test_CreateFontIndirect();
}
