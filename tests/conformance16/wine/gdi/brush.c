/*
 * Unit test suite for brushes
 *
 * Copyright 2004 Kevin Koltzau
 *
 * Ported to Win16 from Wine's dlls/gdi32/tests/brush.c: CreateSolidBrush
 * and GetObject on brushes and on the stock brushes, with Windows 3.1's
 * LOGBRUSH (a 16-bit style and hatch). GetLastError, which Windows 3.1
 * doesn't have, is left out of the messages. Windows 9x's answers are
 * the ones Windows 3.1 gives: a solid brush of a stock brush's colour
 * may be that stock brush, and GetObject may still answer for a deleted
 * brush. test_hatchbrush isn't Wine's: it checks the LOGBRUSH that
 * GetObject gives for CreateHatchBrush's and CreateBrushIndirect's
 * brushes in the same way.
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

typedef struct _STOCK_BRUSH {
    COLORREF color;
    int stockobj;
    const char *name;
} STOCK_BRUSH;

static void test_solidbrush(void)
{
    static const STOCK_BRUSH stock[] = {
        {RGB(255,255,255), WHITE_BRUSH, "white"},
        {RGB(192,192,192), LTGRAY_BRUSH, "ltgray"},
        {RGB(128,128,128), GRAY_BRUSH, "gray"},
        {RGB(0,0,0), BLACK_BRUSH, "black"},
        {RGB(0,0,255), -1, "blue"}
    };
    HBRUSH solidBrush;
    HBRUSH stockBrush;
    LOGBRUSH br;
    int i;
    int ret;

    for(i=0; i<sizeof(stock)/sizeof(stock[0]); i++) {
        solidBrush = CreateSolidBrush(stock[i].color);

        if(stock[i].stockobj != -1) {
            stockBrush = GetStockObject(stock[i].stockobj);
            ok(stockBrush!=solidBrush ||
               broken(stockBrush==solidBrush), /* win9x does return stock object */
               "Stock %s brush equals solid %s brush\n", stock[i].name, stock[i].name);
        }
        else
            stockBrush = NULL;
        memset(&br, 0, sizeof(br));
        ret = GetObject(solidBrush, sizeof(br), &br);
        ok( ret !=0, "GetObject on solid %s brush failed\n", stock[i].name);
        ok(br.lbStyle==BS_SOLID, "%s brush has wrong style, got %u expected %u\n", stock[i].name, br.lbStyle, BS_SOLID);
        ok(br.lbColor==stock[i].color, "%s brush has wrong color, got 0x%08lx expected 0x%08lx\n", stock[i].name, br.lbColor, stock[i].color);

        if(stockBrush) {
            /* Sanity check, make sure the colors being compared do in fact have a stock brush */
            ret = GetObject(stockBrush, sizeof(br), &br);
            ok( ret !=0, "GetObject on stock %s brush failed\n", stock[i].name);
            ok(br.lbColor==stock[i].color, "stock %s brush unexpected color, got 0x%08lx expected 0x%08lx\n", stock[i].name, br.lbColor, stock[i].color);
        }

        DeleteObject(solidBrush);
        ret = GetObject(solidBrush, sizeof(br), &br);
        ok(ret==0 ||
           broken(ret!=0), /* win9x */
           "GetObject succeeded on a deleted %s brush\n", stock[i].name);
    }
}

static void test_hatchbrush(void)
{
    static const LOGBRUSH brushes[] = {
        {BS_SOLID, RGB(0x12,0x34,0x56), 0},
        {BS_HATCHED, RGB(0x65,0x43,0x21), HS_CROSS},
        {BS_HATCHED, RGB(0,0,0xff), HS_DIAGCROSS},
        {BS_NULL, 0, 0}
    };
    HBRUSH brush;
    LOGBRUSH br, in;
    int i, ret;

    for (i = 0; i < sizeof(brushes)/sizeof(brushes[0]); i++) {
        in = brushes[i];
        brush = CreateBrushIndirect(&in);
        ok(brush != 0, "%d: CreateBrushIndirect failed\n", i);
        memset(&br, 0xb0, sizeof(br));
        ret = GetObject(brush, sizeof(br), &br);
        ok(ret == sizeof(br), "%d: GetObject returned %d\n", i, ret);
        ok(br.lbStyle == brushes[i].lbStyle, "%d: style %u, expected %u\n",
           i, br.lbStyle, brushes[i].lbStyle);
        if (brushes[i].lbStyle != BS_NULL) {
            ok(br.lbColor == brushes[i].lbColor, "%d: color %08lx, expected %08lx\n",
               i, br.lbColor, brushes[i].lbColor);
            ok(br.lbHatch == brushes[i].lbHatch, "%d: hatch %d, expected %d\n",
               i, br.lbHatch, brushes[i].lbHatch);
        }
        DeleteObject(brush);

        if (brushes[i].lbStyle != BS_HATCHED) continue;
        brush = CreateHatchBrush(brushes[i].lbHatch, brushes[i].lbColor);
        ok(brush != 0, "%d: CreateHatchBrush failed\n", i);
        memset(&br, 0xb0, sizeof(br));
        ret = GetObject(brush, sizeof(br), &br);
        ok(ret == sizeof(br), "%d: GetObject returned %d\n", i, ret);
        ok(br.lbStyle == BS_HATCHED, "%d: style %u\n", i, br.lbStyle);
        ok(br.lbColor == brushes[i].lbColor, "%d: color %08lx, expected %08lx\n",
           i, br.lbColor, brushes[i].lbColor);
        ok(br.lbHatch == brushes[i].lbHatch, "%d: hatch %d, expected %d\n",
           i, br.lbHatch, brushes[i].lbHatch);
        DeleteObject(brush);
    }
}

START_TEST(brush)
{
    test_solidbrush();
    test_hatchbrush();
}
