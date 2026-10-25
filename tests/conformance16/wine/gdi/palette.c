/*
 * Unit test suite for palettes
 *
 * Copyright 2005 Glenn Wurster
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/palette.c: CreatePalette,
 * SetPaletteEntries and GetPaletteEntries. test_DIB_PAL_COLORS is left
 * out, as it draws on a DIB section, which Windows 3.1 doesn't have.
 * test_palette_contents isn't Wine's: it checks what GetPaletteEntries
 * gives back of CreatePalette's colours, what GetNearestPaletteIndex
 * finds in them and what SelectPalette returns.
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

static const PALETTEENTRY logpalettedata[8] = {
    { 0x10, 0x20, 0x30, PC_NOCOLLAPSE },
    { 0x20, 0x30, 0x40, PC_NOCOLLAPSE },
    { 0x30, 0x40, 0x50, PC_NOCOLLAPSE },
    { 0x40, 0x50, 0x60, PC_NOCOLLAPSE },
    { 0x50, 0x60, 0x70, PC_NOCOLLAPSE },
    { 0x60, 0x70, 0x80, PC_NOCOLLAPSE },
    { 0x70, 0x80, 0x90, PC_NOCOLLAPSE },
    { 0x80, 0x90, 0xA0, PC_NOCOLLAPSE },
};

static HPALETTE create_test_palette(void)
{
    char logpalettebuf[sizeof(LOGPALETTE) + sizeof(logpalettedata)];
    LOGPALETTE *logpalette = (LOGPALETTE *)logpalettebuf;

    /* Initalize the logical palette with a few colours */
    logpalette->palVersion = 0x300;
    logpalette->palNumEntries = 8;
    memcpy( logpalette->palPalEntry, logpalettedata, sizeof(logpalettedata) );
    return CreatePalette( logpalette );
}

static void test_palette_entries(void)
{
    HPALETTE hpal;
    UINT res=0;
    PALETTEENTRY palEntry = { 0x1, 0x2, 0x3, 0xff };
    PALETTEENTRY getEntryResult;

    hpal = create_test_palette();

    /* Set a new entry with peFlags to 0xff */
    SetPaletteEntries(hpal, 0, 1, &palEntry);

    /* Retrieve the entry to see if GDI performs any filtering on peFlags */
    res = GetPaletteEntries(hpal, 0, 1, &getEntryResult);
    ok(res == 1, "GetPaletteEntries should have returned 1 but returned %u\n", res);

    ok( palEntry.peFlags == getEntryResult.peFlags, "palEntry.peFlags (%#x) != getEntryResult.peFlags (%#x)\n", palEntry.peFlags, getEntryResult.peFlags );

    DeleteObject( hpal );
}

static void test_palette_contents(void)
{
    HPALETTE hpal, old;
    PALETTEENTRY entries[8];
    WORD count;
    UINT res, i;
    HDC hdc;

    hpal = create_test_palette();
    ok(hpal != 0, "CreatePalette failed\n");

    /* GetObject on a palette gives its number of entries */
    count = 0;
    res = GetObject(hpal, sizeof(count), &count);
    ok(res == sizeof(count), "GetObject returned %u\n", res);
    ok(count == 8, "GetObject gave %u entries\n", count);

    memset(entries, 0xcc, sizeof(entries));
    res = GetPaletteEntries(hpal, 0, 8, entries);
    ok(res == 8, "GetPaletteEntries returned %u\n", res);
    for (i = 0; i < 8; i++)
        ok(entries[i].peRed == logpalettedata[i].peRed &&
           entries[i].peGreen == logpalettedata[i].peGreen &&
           entries[i].peBlue == logpalettedata[i].peBlue &&
           entries[i].peFlags == logpalettedata[i].peFlags,
           "entry %u is %02x %02x %02x %02x\n", i, entries[i].peRed,
           entries[i].peGreen, entries[i].peBlue, entries[i].peFlags);

    /* Only the entries there are */
    res = GetPaletteEntries(hpal, 6, 8, entries);
    ok(res == 2, "GetPaletteEntries past the end returned %u\n", res);
    ok(entries[0].peRed == 0x70 && entries[1].peRed == 0x80,
       "entries 6 and 7 are %02x and %02x\n", entries[0].peRed, entries[1].peRed);

    for (i = 0; i < 8; i++) {
        res = GetNearestPaletteIndex(hpal, RGB(logpalettedata[i].peRed,
                                               logpalettedata[i].peGreen,
                                               logpalettedata[i].peBlue));
        ok(res == i, "GetNearestPaletteIndex of entry %u gave %u\n", i, res);
    }
    res = GetNearestPaletteIndex(hpal, RGB(0x21, 0x2f, 0x41));
    ok(res == 1, "GetNearestPaletteIndex of a colour near entry 1 gave %u\n", res);
    res = GetNearestPaletteIndex(hpal, RGB(0xff, 0xff, 0xff));
    ok(res == 7, "GetNearestPaletteIndex of white gave %u\n", res);
    res = GetNearestPaletteIndex(hpal, RGB(0, 0, 0));
    ok(res == 0, "GetNearestPaletteIndex of black gave %u\n", res);

    hdc = CreateCompatibleDC(0);
    old = SelectPalette(hdc, hpal, FALSE);
    ok(old == GetStockObject(DEFAULT_PALETTE), "the first palette is %04x\n", old);
    old = SelectPalette(hdc, old, FALSE);
    ok(old == hpal, "SelectPalette gave back %04x, not %04x\n", old, hpal);
    DeleteDC(hdc);

    DeleteObject(hpal);
}

START_TEST(palette)
{
    test_palette_entries();
    test_palette_contents();
}
