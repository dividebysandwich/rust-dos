/*
 * The C runtime the conformance tests link instead of MSVCRT.DLL, which
 * the built-in Windows doesn't have yet: the startup code, and the parts
 * of the C library that Wine's test framework and the tests use, written
 * on KERNEL32's functions alone. Formatting follows MSVCRT's printf for
 * what the tests print.
 *
 * Part of rust-dos, GPL-2.0-or-later.
 */

#include <windows.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>

int main(int argc, char **argv);

/* The C library's memory, on the process heap. */

void *malloc(size_t size)
{
    return HeapAlloc(GetProcessHeap(), 0, size);
}

void *calloc(size_t count, size_t size)
{
    return HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, count * size);
}

void *realloc(void *block, size_t size)
{
    if (!block) return malloc(size);
    return HeapReAlloc(GetProcessHeap(), 0, block, size);
}

void free(void *block)
{
    if (block) HeapFree(GetProcessHeap(), 0, block);
}

/* Strings and memory. */

void *memcpy(void *to, const void *from, size_t n)
{
    char *t = to;
    const char *f = from;
    while (n--) *t++ = *f++;
    return to;
}

void *memmove(void *to, const void *from, size_t n)
{
    char *t = to;
    const char *f = from;
    if (t < f)
        while (n--) *t++ = *f++;
    else
        while (n--) t[n] = f[n];
    return to;
}

void *memset(void *to, int c, size_t n)
{
    unsigned char *t = to;
    while (n--) *t++ = c;
    return to;
}

int memcmp(const void *a, const void *b, size_t n)
{
    const unsigned char *x = a, *y = b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x - *y;
    return 0;
}

void *memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;
    for (; n; n--, p++)
        if (*p == (unsigned char)c) return (void *)p;
    return NULL;
}

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p) p++;
    return p - s;
}

int strcmp(const char *a, const char *b)
{
    while (*a && *a == *b) a++, b++;
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n)
{
    for (; n; n--, a++, b++)
        if (*a != *b || !*a) return (unsigned char)*a - (unsigned char)*b;
    return 0;
}

int _stricmp(const char *a, const char *b)
{
    while (*a && tolower((unsigned char)*a) == tolower((unsigned char)*b)) a++, b++;
    return tolower((unsigned char)*a) - tolower((unsigned char)*b);
}

int _strnicmp(const char *a, const char *b, size_t n)
{
    for (; n; n--, a++, b++)
    {
        int x = tolower((unsigned char)*a), y = tolower((unsigned char)*b);
        if (x != y || !x) return x - y;
    }
    return 0;
}

char *strcpy(char *to, const char *from)
{
    char *t = to;
    while ((*t++ = *from++));
    return to;
}

char *strncpy(char *to, const char *from, size_t n)
{
    size_t i = 0;
    for (; i < n && from[i]; i++) to[i] = from[i];
    for (; i < n; i++) to[i] = 0;
    return to;
}

char *strcat(char *to, const char *from)
{
    strcpy(to + strlen(to), from);
    return to;
}

char *strchr(const char *s, int c)
{
    for (;; s++)
    {
        if (*s == (char)c) return (char *)s;
        if (!*s) return NULL;
    }
}

char *strrchr(const char *s, int c)
{
    const char *found = NULL;
    for (;; s++)
    {
        if (*s == (char)c) found = s;
        if (!*s) return (char *)found;
    }
}

char *strstr(const char *s, const char *part)
{
    size_t n = strlen(part);
    for (; *s; s++)
        if (!strncmp(s, part, n)) return (char *)s;
    return n ? NULL : (char *)s;
}

char *strdup(const char *s)
{
    char *copy = malloc(strlen(s) + 1);
    if (copy) strcpy(copy, s);
    return copy;
}

char *_strdup(const char *s)
{
    return strdup(s);
}

size_t wcslen(const wchar_t *s)
{
    const wchar_t *p = s;
    while (*p) p++;
    return p - s;
}

int wcscmp(const wchar_t *a, const wchar_t *b)
{
    while (*a && *a == *b) a++, b++;
    return *a - *b;
}

wchar_t *wcscpy(wchar_t *to, const wchar_t *from)
{
    wchar_t *t = to;
    while ((*t++ = *from++));
    return to;
}

int isdigit(int c) { return c >= '0' && c <= '9'; }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int isalpha(int c) { return isupper(c) || islower(c); }
int isalnum(int c) { return isalpha(c) || isdigit(c); }
int isspace(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }
int isxdigit(int c) { return isdigit(c) || ((c | 0x20) >= 'a' && (c | 0x20) <= 'f'); }
int isprint(int c) { return c >= ' ' && c < 0x7F; }
int toupper(int c) { return islower(c) ? c - 0x20 : c; }
int tolower(int c) { return isupper(c) ? c + 0x20 : c; }

unsigned long strtoul(const char *s, char **end, int base)
{
    unsigned long value = 0;
    int negative = 0;
    while (isspace((unsigned char)*s)) s++;
    if (*s == '-' || *s == '+') negative = *s++ == '-';
    if ((!base || base == 16) && s[0] == '0' && (s[1] | 0x20) == 'x') s += 2, base = 16;
    if (!base) base = *s == '0' ? 8 : 10;
    for (;; s++)
    {
        int c = (unsigned char)*s, digit;
        if (isdigit(c)) digit = c - '0';
        else if (isalpha(c)) digit = (c | 0x20) - 'a' + 10;
        else break;
        if (digit >= base) break;
        value = value * base + digit;
    }
    if (end) *end = (char *)s;
    return negative ? -value : value;
}

long strtol(const char *s, char **end, int base)
{
    return (long)strtoul(s, end, base);
}

int atoi(const char *s)
{
    return (int)strtol(s, NULL, 10);
}

/* Formatting, to a buffer or a file. */

struct sink
{
    char *buf;      /* where the text goes, or NULL to count it only */
    size_t size;    /* the buffer's size */
    size_t len;     /* the length of the text */
    HANDLE file;    /* a file the buffer is written to when full */
};

static void flush_sink(struct sink *out)
{
    DWORD written;
    if (out->file && out->len)
    {
        WriteFile(out->file, out->buf, out->len, &written, NULL);
        out->len = 0;
    }
}

static void put(struct sink *out, char c)
{
    if (out->file && out->len == out->size) flush_sink(out);
    if (out->buf && out->len < out->size) out->buf[out->len] = c;
    out->len++;
}

static void put_padded(struct sink *out, const char *s, size_t n, int width, int left, char pad)
{
    int fill = width > (int)n ? width - (int)n : 0;
    if (!left)
        while (fill--) put(out, pad);
    while (n--) put(out, *s++);
    if (left)
        while (fill-- > 0) put(out, ' ');
}

/* A number with its sign or prefix, digits and precision. */
static void put_number(struct sink *out, unsigned long long value, int negative, int base, int upper,
                       const char *prefix, int width, int precision, int left, int zero)
{
    char digits[32], text[64];
    const char *set = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    int n = 0, len = 0;
    do digits[n++] = set[value % base]; while (value /= base);
    if (precision == 0 && n == 1 && digits[0] == '0') n = 0;
    if (negative) text[len++] = '-';
    while (*prefix) text[len++] = *prefix++;
    if (zero && !left && precision < 0)
        precision = width - len;
    while (precision-- > n) text[len++] = '0';
    while (n) text[len++] = digits[--n];
    put_padded(out, text, len, width, left, ' ');
}

/* A double in %f's form, with `precision` digits after the point. */
static void put_fixed(struct sink *out, double value, int precision, int width, int left, int plus)
{
    char text[400];
    int len = 0, n = 0;
    char digits[400];
    unsigned long long whole;
    double scale = 1;
    int i;
    if (value < 0) text[len++] = '-', value = -value;
    else if (plus) text[len++] = '+';
    for (i = 0; i < precision; i++) scale *= 10;
    value = value * scale + 0.5;
    whole = (unsigned long long)(value / scale);
    value -= (double)whole * scale;
    do digits[n++] = '0' + whole % 10; while (whole /= 10);
    while (n) text[len++] = digits[--n];
    if (precision > 0)
    {
        unsigned long long frac = (unsigned long long)value;
        text[len++] = '.';
        for (i = precision - 1; i >= 0; i--)
        {
            text[len + i] = '0' + frac % 10;
            frac /= 10;
        }
        len += precision;
    }
    put_padded(out, text, len, width, left, ' ');
}

static void format(struct sink *out, const char *fmt, va_list ap)
{
    for (; *fmt; fmt++)
    {
        int left = 0, plus = 0, space = 0, alt = 0, zero = 0, width = 0, precision = -1, size = 0;
        if (*fmt != '%')
        {
            put(out, *fmt);
            continue;
        }
        for (;; fmt++)
        {
            if (fmt[1] == '-') left = 1;
            else if (fmt[1] == '+') plus = 1;
            else if (fmt[1] == ' ') space = 1;
            else if (fmt[1] == '#') alt = 1;
            else if (fmt[1] == '0') zero = 1;
            else break;
        }
        fmt++;
        if (*fmt == '*')
        {
            width = va_arg(ap, int);
            if (width < 0) left = 1, width = -width;
            fmt++;
        }
        else
            while (isdigit((unsigned char)*fmt)) width = width * 10 + *fmt++ - '0';
        if (*fmt == '.')
        {
            fmt++;
            precision = 0;
            if (*fmt == '*') precision = va_arg(ap, int), fmt++;
            else
                while (isdigit((unsigned char)*fmt)) precision = precision * 10 + *fmt++ - '0';
        }
        /* The size: h, l, ll, I64, I32, and the ones of the same size. */
        for (;; fmt++)
        {
            if (*fmt == 'h') size = -1;
            else if (*fmt == 'l' && fmt[1] == 'l') size = 2, fmt++;
            else if (*fmt == 'l' || *fmt == 'z' || *fmt == 't' || *fmt == 'j' || *fmt == 'L') size = size ? size : 1;
            else if (*fmt == 'I' && fmt[1] == '6' && fmt[2] == '4') size = 2, fmt += 2;
            else if (*fmt == 'I' && fmt[1] == '3' && fmt[2] == '2') size = 1, fmt += 2;
            else break;
        }
        switch (*fmt)
        {
        case 'd':
        case 'i':
        {
            long long value = size == 2 ? va_arg(ap, long long) : va_arg(ap, int);
            if (size < 0) value = (short)value;
            put_number(out, value < 0 ? -(unsigned long long)value : (unsigned long long)value, value < 0, 10, 0,
                       value >= 0 && plus ? "+" : value >= 0 && space ? " " : "", width, precision, left, zero);
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o':
        {
            unsigned long long value = size == 2 ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned int);
            int base = *fmt == 'o' ? 8 : *fmt == 'u' ? 10 : 16;
            const char *prefix = "";
            if (size < 0) value = (unsigned short)value;
            if (alt && value && base == 16) prefix = *fmt == 'X' ? "0X" : "0x";
            if (alt && base == 8) prefix = "0";
            put_number(out, value, 0, base, *fmt == 'X', prefix, width, precision, left, zero);
            break;
        }
        case 'p':
            put_number(out, (unsigned long long)(ULONG_PTR)va_arg(ap, void *), 0, 16, 1, "", 8, 8, left, 0);
            break;
        case 'c':
        {
            char c = (char)va_arg(ap, int);
            put_padded(out, &c, 1, width, left, ' ');
            break;
        }
        case 'C':
        {
            char c = (char)(WCHAR)va_arg(ap, int);
            put_padded(out, &c, 1, width, left, ' ');
            break;
        }
        case 's':
        case 'S':
            if ((*fmt == 's') == (size <= 0))
            {
                const char *s = va_arg(ap, const char *);
                size_t n = 0;
                if (!s) s = "(null)";
                while (s[n] && (precision < 0 || (int)n < precision)) n++;
                put_padded(out, s, n, width, left, ' ');
            }
            else
            {
                const WCHAR *s = va_arg(ap, const WCHAR *);
                char text[512];
                size_t n = 0;
                if (!s) s = L"(null)";
                while (s[n] && n < sizeof(text) && (precision < 0 || (int)n < precision))
                {
                    text[n] = s[n] < 0x100 ? (char)s[n] : '?';
                    n++;
                }
                put_padded(out, text, n, width, left, ' ');
            }
            break;
        case 'f':
        case 'F':
        case 'e':
        case 'E':
        case 'g':
        case 'G':
            /* All as %f: the tests print few doubles, and only to read. */
            put_fixed(out, va_arg(ap, double), precision < 0 ? 6 : precision, width, left, plus);
            break;
        case 'n':
            *va_arg(ap, int *) = (int)out->len;
            break;
        case '%':
            put(out, '%');
            break;
        default:
            put(out, '%');
            if (*fmt) put(out, *fmt);
            else fmt--;
        }
    }
}

/* stdin, stdout and stderr: their DOS handles are 0, 1 and 2. */
static FILE files[3];

static FILE *iob_func(unsigned index)
{
    return &files[index];
}

/* The headers import these from MSVCRT.DLL, through pointers. */
FILE *(*imp_acrt_iob_func)(unsigned) __asm__("__imp____acrt_iob_func") = iob_func;

static HANDLE file_handle(FILE *f)
{
    return GetStdHandle(f == &files[2] ? STD_ERROR_HANDLE : f == &files[0] ? STD_INPUT_HANDLE : STD_OUTPUT_HANDLE);
}

int vfprintf(FILE *f, const char *fmt, va_list ap)
{
    char buffer[512];
    struct sink out = {buffer, sizeof(buffer), 0, file_handle(f)};
    size_t total;
    format(&out, fmt, ap);
    total = out.len;
    flush_sink(&out);
    return (int)total;
}

int fprintf(FILE *f, const char *fmt, ...)
{
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = vfprintf(f, fmt, ap);
    va_end(ap);
    return n;
}

int vprintf(const char *fmt, va_list ap)
{
    return vfprintf(&files[1], fmt, ap);
}

int printf(const char *fmt, ...)
{
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = vprintf(fmt, ap);
    va_end(ap);
    return n;
}

/* MinGW's stdio.h has vsnprintf and snprintf inline, calling these. */
int __ms_vsnprintf(char *buf, size_t size, const char *fmt, va_list ap)
{
    struct sink out = {buf, size, 0, NULL};
    format(&out, fmt, ap);
    if (size) buf[out.len < size ? out.len : size - 1] = 0;
    return (int)out.len;
}

int _vsnprintf(char *buf, size_t size, const char *fmt, va_list ap)
{
    struct sink out = {buf, size, 0, NULL};
    format(&out, fmt, ap);
    if (out.len < size) buf[out.len] = 0;
    return out.len <= size ? (int)out.len : -1;
}

int vsprintf(char *buf, const char *fmt, va_list ap)
{
    return __ms_vsnprintf(buf, (size_t)-1 >> 1, fmt, ap);
}

int sprintf(char *buf, const char *fmt, ...)
{
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = vsprintf(buf, fmt, ap);
    va_end(ap);
    return n;
}

int __ms_snprintf(char *buf, size_t size, const char *fmt, ...)
{
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = __ms_vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}

int _snprintf(char *buf, size_t size, const char *fmt, ...)
{
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = _vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}

size_t fwrite(const void *data, size_t size, size_t count, FILE *f)
{
    DWORD written = 0;
    WriteFile(file_handle(f), data, size * count, &written, NULL);
    return size ? written / size : 0;
}

int fputs(const char *s, FILE *f)
{
    return fwrite(s, 1, strlen(s), f) == strlen(s) ? 0 : EOF;
}

int puts(const char *s)
{
    return fputs(s, &files[1]) || fputs("\n", &files[1]) ? EOF : 0;
}

int fputc(int c, FILE *f)
{
    char ch = (char)c;
    return fwrite(&ch, 1, 1, f) ? (unsigned char)c : EOF;
}

int putchar(int c)
{
    return fputc(c, &files[1]);
}

/* Output isn't buffered, so there is nothing to flush or set. */
int fflush(FILE *f)
{
    (void)f;
    return 0;
}

int setvbuf(FILE *f, char *buf, int mode, size_t size)
{
    (void)f, (void)buf, (void)mode, (void)size;
    return 0;
}

void exit(int code)
{
    ExitProcess(code);
}

void abort(void)
{
    fputs("abnormal program termination\n", &files[2]);
    ExitProcess(3);
}

static void assert_failed(const char *what, const char *file, unsigned line)
{
    fprintf(&files[2], "Assertion failed: %s, file %s, line %u\n", what, file, line);
    abort();
}

void (*imp_assert)(const char *, const char *, unsigned) __asm__("__imp___assert") = assert_failed;

/* GCC calls this from main for the program's constructors; there are none. */
void __main(void)
{
}

/* The start: the command line split into arguments as MSVCRT splits it
 * (spaces part them, double quotes group them), then main. */
void mainCRTStartup(void)
{
    const char *line = GetCommandLineA();
    char *copy = malloc(strlen(line) + 1), *to = copy;
    char **argv = malloc(sizeof(char *) * (strlen(line) / 2 + 2));
    int argc = 0;
    while (*line)
    {
        int quoted = 0;
        while (*line == ' ' || *line == '\t') line++;
        if (!*line) break;
        argv[argc++] = to;
        for (; *line && (quoted || (*line != ' ' && *line != '\t')); line++)
        {
            if (*line == '"') quoted = !quoted;
            else *to++ = *line;
        }
        *to++ = 0;
    }
    argv[argc] = NULL;
    ExitProcess(main(argc, argv));
}
