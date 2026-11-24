/* GDI colour mapping probe for an 8-bit desktop (issue 1715).
 * Draws 24-bit pictures that hold every 5-bit cell of the RGB cube
 * (cell n at pixel x = n % 256, y = n / 256: r5 = n >> 10, g5 =
 * (n >> 5) & 31, b5 = n & 31; channel value 8 * c5 + offset) into a
 * DC with the default palette, the halftone palette, and a custom
 * palette, reads each back as 24-bit RGB with the system palette, and
 * writes C:\DD2.BIN (log in C:\DD2.TXT).
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>

#define W 256
#define H 128

static FILE *logf, *out;
static HWND wnd;
static BITMAPINFOHEADER bih;
static BYTE *pic;
static unsigned long seed = 12345;

static int rnd(void)
{
    seed = seed * 1103515245UL + 12345UL;
    return (int)((seed >> 16) & 0xFF);
}

static void make_picture(int off)
{
    int x, y, stride = W * 3;
    for (y = 0; y < H; y++) {
        BYTE *row = pic + (H - 1 - y) * stride;
        for (x = 0; x < W; x++) {
            int n = y * W + x;
            row[x * 3 + 2] = (BYTE)(8 * (n >> 10) + off);
            row[x * 3 + 1] = (BYTE)(8 * ((n >> 5) & 31) + off);
            row[x * 3] = (BYTE)(8 * (n & 31) + off);
        }
    }
}

static void clear(void)
{
    HDC dc = GetDC(wnd);
    PatBlt(dc, 0, 0, 640, 480, WHITENESS);
    GdiFlush();
    ReleaseDC(wnd, dc);
}

static void capture(int id, int w, int h)
{
    HDC dc = GetDC(wnd), mdc = CreateCompatibleDC(dc);
    BITMAPINFO bi;
    void *bits;
    HBITMAP dib, old;
    PALETTEENTRY sys[256];
    int stride = (w * 3 + 3) & ~3, y;
    GdiFlush();
    memset(&bi, 0, sizeof bi);
    bi.bmiHeader.biSize = sizeof bi.bmiHeader;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 24;
    dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, NULL, 0);
    old = SelectObject(mdc, dib);
    BitBlt(mdc, 0, 0, w, h, dc, 0, 0, SRCCOPY);
    GdiFlush();
    GetSystemPaletteEntries(dc, 0, 256, sys);
    fwrite(&id, 4, 1, out);
    fwrite(&w, 4, 1, out);
    fwrite(&h, 4, 1, out);
    fwrite(sys, sizeof sys, 1, out);
    for (y = 0; y < h; y++) fwrite((BYTE *)bits + y * stride, w * 3, 1, out);
    fflush(out);
    SelectObject(mdc, old);
    DeleteObject(dib);
    DeleteDC(mdc);
    ReleaseDC(wnd, dc);
    fprintf(logf, "captured %d %dx%d fg %d\n", id, w, h, GetForegroundWindow() == wnd);
    fflush(logf);
}

static void pump(DWORD ms)
{
    DWORD end = GetTickCount() + ms;
    MSG m;
    while ((int)(end - GetTickCount()) > 0) {
        while (PeekMessage(&m, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&m);
            DispatchMessage(&m);
        }
        Sleep(10);
    }
}

/* pal: 0 default, 1 halftone, 2 custom. how: 0 StretchDIBits,
 * 1 SetDIBitsToDevice, 2 BitBlt from a 24-bit DIB section. */
static void test(int id, int pal, int how, HPALETTE custom)
{
    HDC dc;
    HPALETTE hp = NULL, op = NULL;
    int r = 0;
    clear();
    dc = GetDC(wnd);
    if (pal == 1) hp = CreateHalftonePalette(dc);
    if (pal == 2) hp = custom;
    if (hp) {
        op = SelectPalette(dc, hp, FALSE);
        SetForegroundWindow(wnd);
        fprintf(logf, "realized %u\n", RealizePalette(dc));
    }
    if (how == 0)
        r = StretchDIBits(dc, 0, 0, W, H, 0, 0, W, H, pic, (BITMAPINFO *)&bih, DIB_RGB_COLORS,
                          SRCCOPY);
    else if (how == 1)
        r = SetDIBitsToDevice(dc, 0, 0, W, H, 0, 0, 0, H, pic, (BITMAPINFO *)&bih, DIB_RGB_COLORS);
    else {
        void *bits;
        HDC mdc = CreateCompatibleDC(dc);
        HBITMAP dib = CreateDIBSection(dc, (BITMAPINFO *)&bih, DIB_RGB_COLORS, &bits, NULL, 0);
        HBITMAP old;
        memcpy(bits, pic, W * 3 * H);
        old = SelectObject(mdc, dib);
        r = BitBlt(dc, 0, 0, W, H, mdc, 0, 0, SRCCOPY);
        SelectObject(mdc, old);
        DeleteObject(dib);
        DeleteDC(mdc);
    }
    fprintf(logf, "test %d pal %d how %d -> %d\n", id, pal, how, r);
    GdiFlush();
    capture(id, W, H);
    if (hp) {
        SelectPalette(dc, op, FALSE);
        if (pal == 1) DeleteObject(hp);
    }
    ReleaseDC(wnd, dc);
}

static LRESULT CALLBACK proc(HWND h, UINT m, WPARAM w, LPARAM l)
{
    if (m == WM_ERASEBKGND) return 1;
    if (m == WM_PAINT) {
        PAINTSTRUCT ps;
        BeginPaint(h, &ps);
        EndPaint(h, &ps);
        return 0;
    }
    return DefWindowProc(h, m, w, l);
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmdline, int show)
{
    HDC sdc;
    DEVMODE dm;
    WNDCLASS wc;
    LONG r;
    struct {
        WORD ver, n;
        PALETTEENTRY e[64];
    } lp;
    HPALETTE custom;
    int k;
    logf = fopen("C:\\DD2.TXT", "w");
    memset(&dm, 0, sizeof dm);
    dm.dmSize = sizeof dm;
    dm.dmBitsPerPel = 8;
    dm.dmPelsWidth = 640;
    dm.dmPelsHeight = 480;
    dm.dmFields = DM_BITSPERPEL | DM_PELSWIDTH | DM_PELSHEIGHT;
    r = ChangeDisplaySettings(&dm, 0);
    fprintf(logf, "ChangeDisplaySettings %ld\n", r);
    if (r == DISP_CHANGE_RESTART) {
        fprintf(logf, "UPDATEREGISTRY %ld\n", ChangeDisplaySettings(&dm, CDS_UPDATEREGISTRY));
        fclose(logf);
        return 0;
    }
    sdc = GetDC(NULL);
    fprintf(logf, "bpp %d\n", GetDeviceCaps(sdc, BITSPIXEL));
    ReleaseDC(NULL, sdc);
    pic = (BYTE *)GlobalAlloc(GMEM_FIXED, W * 3 * H);
    memset(&bih, 0, sizeof bih);
    bih.biSize = sizeof bih;
    bih.biWidth = W;
    bih.biHeight = H;
    bih.biPlanes = 1;
    bih.biBitCount = 24;
    bih.biCompression = BI_RGB;
    bih.biSizeImage = W * 3 * H;
    lp.ver = 0x300;
    lp.n = 64;
    for (k = 0; k < 64; k++) {
        lp.e[k].peRed = (BYTE)rnd();
        lp.e[k].peGreen = (BYTE)rnd();
        lp.e[k].peBlue = (BYTE)rnd();
        lp.e[k].peFlags = 0;
        fprintf(logf, "custom %d %d %d %d\n", k, lp.e[k].peRed, lp.e[k].peGreen, lp.e[k].peBlue);
    }
    custom = CreatePalette((LOGPALETTE *)&lp);
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.lpszClassName = "DD2PROBE";
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    RegisterClass(&wc);
    wnd = CreateWindowEx(WS_EX_TOPMOST, "DD2PROBE", "", WS_POPUP | WS_VISIBLE, 0, 0, 640, 480,
                         NULL, NULL, inst, NULL);
    SetCursorPos(639, 479);
    SystemParametersInfo(0x2001, 0, (PVOID)0, 3);
    keybd_event(VK_MENU, 0, 0, 0);
    SetForegroundWindow(wnd);
    keybd_event(VK_MENU, 0, KEYEVENTF_KEYUP, 0);
    pump(300);
    UpdateWindow(wnd);
    pump(1000);
    out = fopen("C:\\DD2.BIN", "wb");
    fwrite("DDB1", 4, 1, out);
    make_picture(0);
    test(0, 0, 0, custom);
    test(1, 1, 0, custom);
    test(2, 2, 0, custom);
    test(3, 1, 2, custom);
    test(4, 0, 2, custom);
    make_picture(7);
    test(10, 0, 0, custom);
    test(11, 1, 0, custom);
    test(12, 2, 0, custom);
    test(13, 1, 1, custom);
    make_picture(4);
    test(20, 0, 0, custom);
    test(21, 1, 0, custom);
    test(22, 2, 0, custom);
    fclose(out);
    fprintf(logf, "done\n");
    fclose(logf);
    DestroyWindow(wnd);
    return 0;
}
