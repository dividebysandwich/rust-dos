/*
 * DrawText tests
 *
 * Copyright (c) 2004 Zach Gorman
 * Copyright 2007 Dmitry Timoshkov
 *
 * Ported to Win16 from Wine's dlls/user32/tests/text.c: DrawText with
 * DT_CALCRECT in MM_HIENGLISH and MM_TEXT and with empty strings, and
 * GetTabbedTextExtent. Windows 3.1 has no DrawTextEx, DrawTextW or
 * DrawState, so their checks are left out, as are the right-aligned
 * (negative) tab stops, which came with Windows 95, and the calls with
 * a null string, which Windows 3.1's parameter validation turns away.
 * Wine's test decides from what DrawText returns for an empty count
 * whether the system answers as Windows XP or as Windows 9x, which
 * Windows 3.1 is.
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

#define MODIFIED(rect) (rect.left == 10 && rect.right != 100 && rect.top == 10 && rect.bottom != 100)
#define EMPTY(rect) (rect.left == rect.right && rect.bottom == rect.top)

static HINSTANCE main_instance;

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

static void test_DrawTextCalcRect(void)
{
    HWND hwnd;
    HDC hdc;
    HFONT hFont, hOldFont;
    LOGFONT lf;
    static char text[] = "Example text for testing DrawText in "
      "MM_HIENGLISH mode";
    static char emptystring[] = "";
    int textlen, textheight;
    RECT rect = { 0, 0, 100, 0 };
    BOOL ret;
    BOOL conform_xp = TRUE;

    /* Initialization */
    hwnd = CreateWindowEx(0, "static", NULL, WS_POPUP,
                           0, 0, 200, 200, 0, 0, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    hdc = GetDC(hwnd);
    ok(hdc != 0, "GetDC failed\n");
    trace("hdc %04x\n", hdc);
    textlen = lstrlen(text);

    /* LOGFONT initialization */
    memset(&lf, 0, sizeof(lf));
    lf.lfCharSet = ANSI_CHARSET;
    lf.lfClipPrecision = CLIP_DEFAULT_PRECIS;
    lf.lfWeight = FW_DONTCARE;
    lf.lfHeight = 0; /* mapping mode dependent */
    lf.lfQuality = DEFAULT_QUALITY;
    lstrcpy(lf.lfFaceName, "Arial");

    /* DrawText in MM_HIENGLISH with DT_CALCRECT */
    SetMapMode(hdc, MM_HIENGLISH);
    lf.lfHeight = 100 * 9 / 72; /* 9 point */
    hFont = CreateFontIndirect(&lf);
    ok(hFont != 0, "CreateFontIndirect failed\n");
    hOldFont = SelectObject(hdc, hFont);

    textheight = DrawText(hdc, text, textlen, &rect, DT_CALCRECT |
       DT_EXTERNALLEADING | DT_WORDBREAK | DT_NOCLIP | DT_LEFT |
       DT_NOPREFIX);
    ok( textheight != 0, "DrawText failed\n");

    trace("MM_HIENGLISH rect.bottom %d\n", rect.bottom);
    ok(rect.bottom < 0, "In MM_HIENGLISH, DrawText with "
       "DT_CALCRECT should return a negative rectangle bottom. "
       "(bot=%d)\n", rect.bottom);

    SelectObject(hdc, hOldFont);
    ret = DeleteObject(hFont);
    ok( ret, "DeleteObject failed\n");


    /* DrawText in MM_TEXT with DT_CALCRECT */
    SetMapMode(hdc, MM_TEXT);
    lf.lfHeight = -MulDiv(9, GetDeviceCaps(hdc,
       LOGPIXELSY), 72); /* 9 point */
    hFont = CreateFontIndirect(&lf);
    ok(hFont != 0, "CreateFontIndirect failed\n");
    hOldFont = SelectObject(hdc, hFont);

    textheight = DrawText(hdc, text, textlen, &rect, DT_CALCRECT |
       DT_EXTERNALLEADING | DT_WORDBREAK | DT_NOCLIP | DT_LEFT |
       DT_NOPREFIX);
    ok( textheight != 0, "DrawText failed\n");

    trace("MM_TEXT rect.bottom %d\n", rect.bottom);
    ok(rect.bottom > 0, "In MM_TEXT, DrawText with DT_CALCRECT "
       "should return a positive rectangle bottom. (bot=%d)\n",
       rect.bottom);

    /* empty or null text should in some cases calc an empty rectangle */

    SetRect( &rect, 10,10, 100, 100);
    textheight = DrawText(hdc, text, 0, &rect, DT_CALCRECT);
    ok( !EMPTY(rect) && !MODIFIED(rect),
        "rectangle should NOT be empty and NOT modified got %d,%d-%d,%d\n",
        rect.left, rect.top, rect.right, rect.bottom );
    if (textheight != 0)  /* Windows 98 */
    {
        trace("XP conformity failed, skipping XP tests. Probably win9x\n");
        conform_xp = FALSE;
    }

    SetRect( &rect, 10,10, 100, 100);
    textheight = DrawText(hdc, emptystring, -1, &rect, DT_CALCRECT);
    ok( EMPTY(rect),
        "rectangle should be empty got %d,%d-%d,%d\n",
        rect.left, rect.top, rect.right, rect.bottom );
    ok(textheight!=0,"Failed to get textheight from DrawText\n");

    /* DT_SINGLELINE tests */

    SetRect( &rect, 10,10, 100, 100);
    textheight = DrawText(hdc, text, 0, &rect, DT_CALCRECT|DT_SINGLELINE);
    ok( !EMPTY(rect) && !MODIFIED(rect),
        "rectangle should NOT be empty and NOT modified got %d,%d-%d,%d\n",
        rect.left, rect.top, rect.right, rect.bottom );
    if (conform_xp)
        ok(textheight==0,"Got textheight from DrawText\n");

    SetRect( &rect, 10,10, 100, 100);
    textheight = DrawText(hdc, emptystring, -1, &rect, DT_CALCRECT|DT_SINGLELINE);
    ok( !EMPTY(rect) && MODIFIED (rect),
        "rectangle should be modified got %d,%d-%d,%d\n",
        rect.left, rect.top, rect.right, rect.bottom );
    ok(textheight!=0,"Failed to get textheight from DrawText\n");

    /* further tests with  0 count and empty strings */
    textheight = DrawText(hdc, text, 0, &rect, 0);
    if (conform_xp)
        ok(textheight==0,"Got textheight from DrawText\n");
    textheight = DrawText(hdc, emptystring, 0, &rect, 0);
    if (conform_xp)
        ok(textheight==0,"Got textheight from DrawText\n");
    textheight = DrawText(hdc, emptystring, -1, &rect, 0);
    ok(textheight!=0,"Failed to get textheight from DrawText\n");

    /* More test cases from bug 12226 */
    SetRect(&rect, 0, 0, 0, 0);
    textheight = DrawText(hdc, emptystring, -1, &rect, DT_CALCRECT | DT_LEFT | DT_SINGLELINE);
    ok(textheight != 0, "DrawText failed\n");
    ok(0 == rect.left, "expected 0, got %d\n", rect.left);
    ok(0 == rect.right, "expected 0, got %d\n", rect.right);
    ok(0 == rect.top, "expected 0, got %d\n", rect.top);
    ok(rect.bottom != 0, "rect.bottom should not be 0\n");

    SelectObject(hdc, hOldFont);
    ret = DeleteObject(hFont);
    ok( ret, "DeleteObject failed\n");

    /* Clean up */
    ret = ReleaseDC(hwnd, hdc);
    ok( ret, "ReleaseDC failed\n");
    ret = DestroyWindow(hwnd);
    ok( ret, "DestroyWindow failed\n");
}

/* replace tabs by \t */
static void strfmt( const char *str, char *strout)
{
    unsigned int i,j ;
    for(i=0,j=0;i<=strlen(str);i++,j++)
        if((strout[j]=str[i])=='\t') {
            strout[j++]='\\';
            strout[j]='t';
        }
}


#define TABTEST( tabval, tabcount, string, _exp) \
{ int i,x_act, x_exp; char strdisp[64];\
    for(i=0;i<8;i++) tabs[i]=(i+1)*(tabval); \
    extent = GetTabbedTextExtent( hdc, string, strlen( string), (tabcount), tabs); \
    strfmt( string, strdisp); \
    x_act = LOWORD( extent); \
    x_exp = (_exp); \
    ok( x_act == x_exp, "Test case \"%s\". Text extent is %d, expected %d tab %d tabcount %d\n", \
        (LPSTR)strdisp, x_act, x_exp, tabval, tabcount); \
} \


static void test_TabbedText(void)
{
    HWND hwnd;
    HDC hdc;
    BOOL ret;
    TEXTMETRIC tm;
    DWORD extent;
    int tabs[8], cx, cy, tab, tabcount,t,align;

    /* Initialization */
    hwnd = CreateWindowEx(0, "static", NULL, WS_POPUP,
                           0, 0, 200, 200, 0, 0, main_instance, NULL);
    ok(hwnd != 0, "CreateWindowEx failed\n");
    hdc = GetDC(hwnd);
    ok(hdc != 0, "GetDC failed\n");

    ret = GetTextMetrics( hdc, &tm);
    ok( ret, "GetTextMetrics failed\n");

    extent = GetTabbedTextExtent( hdc, "x", 1, 1, tabs);
    cx = LOWORD( extent);
    cy = HIWORD( extent);
    trace( "cx is %d cy is %d\n", cx, cy);
    ok( cy == tm.tmHeight, "cy is %d, not the font's height %d\n", cy, tm.tmHeight);

    align=1;
    for( t=-1; t<=1; t++) { /* slightly adjust the 4 char tabstop, to
                               catch the one off errors */
        tab =  (cx *4 + t);
        /* test the special case tabcount =1 and the general array (80 of tabs */
        for( tabcount = 1; tabcount <= 8; tabcount +=7) {
            TABTEST( align * tab, tabcount, "\t", tab)
            TABTEST( align * tab, tabcount, "xxx\t", tab)
            TABTEST( align * tab, tabcount, "\tx", tab+cx)
            TABTEST( align * tab, tabcount, "\t\t", tab*2)
            TABTEST( align * tab, tabcount, "\tx\t", tab*2)
            TABTEST( align * tab, tabcount, "x\tx", tab+cx)
            TABTEST( align * tab, tabcount, "xx\tx", tab+cx)
            TABTEST( align * tab, tabcount, "xxx\tx", tab+cx)
            TABTEST( align * tab, tabcount, "xxxx\tx", t>0 ? tab + cx : 2*tab+cx)
            TABTEST( align * tab, tabcount, "xxxxx\tx", 2*tab+cx)
        }
    }

    ReleaseDC( hwnd, hdc );
    DestroyWindow( hwnd );
}

START_TEST(text)
{
    /* A program's instance handle is its data segment's selector. */
    main_instance = (HINSTANCE)get_ds();

    test_TabbedText();
    test_DrawTextCalcRect();
}
