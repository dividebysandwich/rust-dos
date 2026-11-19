/* What Windows 98's GDI gives its fonts, for tools/gen_truetype_metrics.py
   and for comparing the built-in Windows with it.

   Run with no argument, it writes FONTS.TXT beside the exe: the TrueType
   faces' TEXTMETRICA, GetCharWidthA and GetCharABCWidthsA widths of 20h to
   FFh and some strings' extents at heights -80 to 80, and with widths
   asked for; MS Sans Serif's and System's; the LOGFONTs EnumFontFamiliesA
   gives; DEFAULT_GUI_FONT; and copies of the TrueType files of
   C:\WINDOWS\FONTS. Run with the argument 2, it writes FONT2.TXT: the
   TrueType faces' metrics and widths at every height from -1 to -128.
   With the argument 3, it writes FONT3.TXT: the raster faces' metrics
   and widths at heights -40 to 40. With the argument 4, it writes
   FONT4.TXT: what FONT2.TXT has, in the character sets EASTEUROPE,
   RUSSIAN, GREEK, TURKISH and BALTIC (Windows 98 needs multilanguage
   support installed for them); their lines name the character set.

   Build with Visual C++ (no C library) and set the PE's OS and subsystem
   versions to 4.0 so that Windows 98 loads it:

     cl /O1 /GS- /c win98_font_probe.c
     link /NODEFAULTLIB /ENTRY:start /SUBSYSTEM:WINDOWS win98_font_probe.obj
          kernel32.lib user32.lib gdi32.lib /OUT:FPROBE.EXE */
#include <windows.h>

#pragma function(memset)
void *memset(void *d, int c, size_t n)
{
    unsigned char *p = d;
    while (n--) *p++ = (unsigned char)c;
    return d;
}

#pragma function(memcpy)
void *memcpy(void *d, const void *s, size_t n)
{
    unsigned char *p = d;
    const unsigned char *q = s;
    while (n--) *p++ = *q++;
    return d;
}

static HANDLE out;
static char dir[MAX_PATH];
static char obuf[65536];
static int olen;

static void flush(void)
{
    DWORD n;
    if (olen) WriteFile(out, obuf, olen, &n, NULL);
    olen = 0;
}

static void say(const char *fmt, ...)
{
    char buf[1100];
    int n;
    va_list ap;
    va_start(ap, fmt);
    n = wvsprintfA(buf, fmt, ap);
    va_end(ap);
    buf[n++] = '\r';
    buf[n++] = '\n';
    if (olen + n > sizeof obuf) flush();
    CopyMemory(obuf + olen, buf, n);
    olen += n;
}

static HDC dc;
static int charset = ANSI_CHARSET;

static void tm_line(const char *tag, TEXTMETRICA *t)
{
    say("%s h %d asc %d desc %d il %d el %d avg %d max %d wt %d oh %d "
        "first %d last %d def %d brk %d it %d pf %d cs %d",
        tag, t->tmHeight, t->tmAscent, t->tmDescent, t->tmInternalLeading,
        t->tmExternalLeading, t->tmAveCharWidth, t->tmMaxCharWidth, t->tmWeight,
        t->tmOverhang, t->tmFirstChar, t->tmLastChar, t->tmDefaultChar,
        t->tmBreakChar, t->tmItalic, t->tmPitchAndFamily, t->tmCharSet);
}

static void lf_line(const char *tag, LOGFONTA *l)
{
    say("%s lf h %d w %d wt %d it %d cs %d op %d cp %d q %d pf %d face %s", tag,
        l->lfHeight, l->lfWidth, l->lfWeight, l->lfItalic, l->lfCharSet,
        l->lfOutPrecision, l->lfClipPrecision, l->lfQuality, l->lfPitchAndFamily,
        l->lfFaceName);
}

static const char *samples[] = {
    "Sound Effects", "Extreme Confirmation", "Extreme Information", "Play Intro",
    "CUC (Celestine UltraCryo Inc.)", "The quick brown fox jumps over the lazy dog",
    "Hercbase Ambient Animations", "WWWWW iiiii", 0
};

static void measure(const char *face, int h, int w, int weight, int italic, int full)
{
    LOGFONTA l;
    HFONT f, old;
    TEXTMETRICA t;
    static char tag[128], line[1100];
    static int widths[256];
    static ABC abc[256];
    int i, n;
    SIZE s;
    memset(&l, 0, sizeof l);
    l.lfHeight = h;
    l.lfWidth = w;
    l.lfWeight = weight;
    l.lfItalic = (BYTE)italic;
    l.lfCharSet = (BYTE)charset;
    lstrcpyA(l.lfFaceName, face);
    f = CreateFontIndirectA(&l);
    old = SelectObject(dc, f);
    GetTextMetricsA(dc, &t);
    GetTextFaceA(dc, sizeof line, line);
    if (charset != ANSI_CHARSET)
        wsprintfA(tag, "F %s|%d|%d|%d|%d|%d", face, h, w, weight, italic, charset);
    else
        wsprintfA(tag, "F %s|%d|%d|%d|%d", face, h, w, weight, italic);
    tm_line(tag, &t);
    say("  face %s", line);
    if (GetCharWidthA(dc, 0x20, 0xFF, widths)) {
        n = 0;
        for (i = 0; i < 224; i++) n += wsprintfA(line + n, "%d ", widths[i]);
        say("  W %s", line);
    }
    if (full && GetCharABCWidthsA(dc, 0x20, 0xFF, abc)) {
        n = 0;
        for (i = 0; i < 224; i++) n += wsprintfA(line + n, "%d ", abc[i].abcA);
        say("  A %s", line);
        n = 0;
        for (i = 0; i < 224; i++) n += wsprintfA(line + n, "%d ", abc[i].abcB);
        say("  B %s", line);
    }
    if (full)
        for (i = 0; samples[i]; i++) {
            GetTextExtentPoint32A(dc, samples[i], lstrlenA(samples[i]), &s);
            say("  X %d %d %s", s.cx, s.cy, samples[i]);
        }
    SelectObject(dc, old);
    DeleteObject(f);
}

static int CALLBACK fam_proc(const LOGFONTA *l, const TEXTMETRICA *t, DWORD type, LPARAM p)
{
    const ENUMLOGFONTA *e = (const ENUMLOGFONTA *)l;
    const NEWTEXTMETRICA *n = (const NEWTEXTMETRICA *)t;
    char tag[200];
    wsprintfA(tag, "E%d type %d full %s style %s", (int)p, type, e->elfFullName, e->elfStyle);
    lf_line(tag, (LOGFONTA *)l);
    tm_line("  tm", (TEXTMETRICA *)t);
    say("  ntm flags %x em %d cell %d avg %d", n->ntmFlags, n->ntmSizeEM, n->ntmCellHeight,
        n->ntmAvgWidth);
    return 1;
}

static int CALLBACK all_proc(const LOGFONTA *l, const TEXTMETRICA *t, DWORD type, LPARAM p)
{
    say("FAMILY %s type %d", l->lfFaceName, type);
    return 1;
}

static void copy_font(const char *name)
{
    char src[MAX_PATH], dst[MAX_PATH];
    GetWindowsDirectoryA(src, MAX_PATH);
    lstrcatA(src, "\\FONTS\\");
    lstrcatA(src, name);
    lstrcpyA(dst, dir);
    lstrcatA(dst, name);
    say("copy %s %d", name, CopyFileA(src, dst, FALSE));
}

static const char *tt[] = {"Arial", "Times New Roman", "Courier New", 0};

void start(void)
{
    char path[MAX_PATH], *p;
    int n, i, h, w, s;
    LOGFONTA l;
    HFONT g;
    TEXTMETRICA t;
    n = GetModuleFileNameA(NULL, dir, MAX_PATH);
    for (p = dir + n; p > dir && p[-1] != '\\'; p--)
        ;
    *p = 0;
    lstrcpyA(path, dir);
    lstrcatA(path, "FONTS.TXT");
    if (lstrlenA(GetCommandLineA()) > 2 && GetCommandLineA()[lstrlenA(GetCommandLineA()) - 1] == '2')
        path[lstrlenA(path) - 5] = '2';
    if (lstrlenA(GetCommandLineA()) > 2 && GetCommandLineA()[lstrlenA(GetCommandLineA()) - 1] == '3')
        path[lstrlenA(path) - 5] = '3';
    if (lstrlenA(GetCommandLineA()) > 2 && GetCommandLineA()[lstrlenA(GetCommandLineA()) - 1] == '4')
        path[lstrlenA(path) - 5] = '4';
    out = CreateFileA(path, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS, 0, NULL);
    say("probe start, version %08x", GetVersion());
    dc = GetDC(NULL);
    if (path[lstrlenA(path) - 5] == '4') {
        static const int sets[] = {EASTEUROPE_CHARSET, RUSSIAN_CHARSET, GREEK_CHARSET,
                                   TURKISH_CHARSET, BALTIC_CHARSET, 0};
        int c;
        for (c = 0; sets[c]; c++) {
            charset = sets[c];
            for (i = 0; tt[i]; i++)
                for (s = 0; s < 4; s++)
                    for (h = -1; h >= -128; h--) {
                        measure(tt[i], h, 0, s & 1 ? 700 : 400, s >> 1, 0);
                        flush();
                    }
        }
        say("probe done");
        flush();
        CloseHandle(out);
        ExitProcess(0);
    }
    if (path[lstrlenA(path) - 5] == '3') {
        static const char *raster[] = {"MS Serif", "Small Fonts", "Courier", "Terminal",
                                       "Fixedsys", "MS Sans Serif", "System", 0};
        for (i = 0; raster[i]; i++)
            for (h = -40; h <= 40; h++) {
                measure(raster[i], h, 0, 400, 0, 0);
                measure(raster[i], h, 0, 700, 0, 0);
                flush();
            }
        for (i = 0; raster[i]; i++) {
            measure(raster[i], 0, 0, 400, 0, 1);
            for (w = 1; w <= 16; w++) measure(raster[i], -13, w, 400, 0, 0);
            flush();
        }
        say("probe done");
        flush();
        CloseHandle(out);
        ExitProcess(0);
    }
    if (path[lstrlenA(path) - 5] == '2') {
        for (i = 0; tt[i]; i++)
            for (s = 0; s < 4; s++)
                for (h = -1; h >= -128; h--) {
                    measure(tt[i], h, 0, s & 1 ? 700 : 400, s >> 1, 0);
                    flush();
                }
        for (i = 0; i < 3; i++) {
            static const int cs[3] = {238, 204, 161};
            memset(&l, 0, sizeof l);
            l.lfHeight = -13;
            l.lfCharSet = (BYTE)cs[i];
            lstrcpyA(l.lfFaceName, "Arial");
            g = CreateFontIndirectA(&l);
            SelectObject(dc, g);
            {
                static int wd[256];
                static char line[1100];
                int k, m = 0;
                GetTextMetricsA(dc, &t);
                tm_line("CS", &t);
                GetCharWidthA(dc, 0x20, 0xFF, wd);
                for (k = 0; k < 224; k++) m += wsprintfA(line + m, "%d ", wd[k]);
                say("  W %s", line);
            }
            SelectObject(dc, GetStockObject(SYSTEM_FONT));
            DeleteObject(g);
        }
        say("probe done");
        flush();
        CloseHandle(out);
        ExitProcess(0);
    }
    say("logpixels %d %d", GetDeviceCaps(dc, LOGPIXELSX), GetDeviceCaps(dc, LOGPIXELSY));
    copy_font("ARIAL.TTF");
    copy_font("ARIALBD.TTF");
    copy_font("ARIALI.TTF");
    copy_font("ARIALBI.TTF");
    copy_font("TIMES.TTF");
    copy_font("TIMESBD.TTF");
    copy_font("TIMESI.TTF");
    copy_font("TIMESBI.TTF");
    copy_font("COUR.TTF");
    copy_font("COURBD.TTF");
    copy_font("COURI.TTF");
    copy_font("COURBI.TTF");
    flush();

    EnumFontFamiliesA(dc, NULL, (FONTENUMPROCA)all_proc, 0);
    EnumFontFamiliesA(dc, "Arial", (FONTENUMPROCA)fam_proc, 1);
    EnumFontFamiliesA(dc, "Times New Roman", (FONTENUMPROCA)fam_proc, 2);
    EnumFontFamiliesA(dc, "Courier New", (FONTENUMPROCA)fam_proc, 3);
    EnumFontFamiliesA(dc, "MS Sans Serif", (FONTENUMPROCA)fam_proc, 4);
    EnumFontFamiliesA(dc, "System", (FONTENUMPROCA)fam_proc, 5);
    flush();

    g = GetStockObject(DEFAULT_GUI_FONT);
    GetObjectA(g, sizeof l, &l);
    lf_line("GUI", &l);
    SelectObject(dc, g);
    GetTextMetricsA(dc, &t);
    tm_line("GUI tm", &t);
    SelectObject(dc, GetStockObject(SYSTEM_FONT));
    g = GetStockObject(ANSI_VAR_FONT);
    GetObjectA(g, sizeof l, &l);
    lf_line("ANSIVAR", &l);
    flush();

    /* The TrueType faces at every height from 4 to 50 pixels, both
       signs, and some bigger ones; regular, bold, italic, bold italic. */
    for (i = 0; tt[i]; i++)
        for (s = 0; s < 4; s++)
            for (h = -80; h <= 80; h++) {
                if (h > -4 && h < 4) continue;
                if ((h > 50 || h < -50) && h % 8) continue;
                measure(tt[i], h, 0, s & 1 ? 700 : 400, s >> 1, 1);
                flush();
            }
    /* Height 0 and the default. */
    for (i = 0; tt[i]; i++) measure(tt[i], 0, 0, 400, 0, 1);
    /* Width asked for. */
    for (i = 0; tt[i]; i++)
        for (h = -21; h <= 16; h++) {
            if (h != -21 && h != -16 && h != -13 && h != -11 && h != 16 && h != 13) continue;
            for (w = 1; w <= 16; w++) measure(tt[i], h, w, 400, 0, 0);
            for (w = 1; w <= 16; w++) measure(tt[i], h, w, 700, 0, 0);
            flush();
        }
    /* The raster faces. */
    for (h = -40; h <= 40; h++) {
        if (h == 0) continue;
        measure("MS Sans Serif", h, 0, 400, 0, 0);
        measure("MS Sans Serif", h, 0, 700, 0, 0);
        measure("System", h, 0, 400, 0, 0);
        flush();
    }
    for (w = 1; w <= 16; w++) measure("MS Sans Serif", -11, w, 400, 0, 0);
    say("probe done");
    flush();
    CloseHandle(out);
    ExitProcess(0);
}
