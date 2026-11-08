/*
 * Tests of the DPMI services Windows gives its programs (INT 31h)
 *
 * Copyright 1995 Alexandre Julliard (Wine's DPMI 0.9 emulation,
 * dlls/krnl386.exe16/int31.c, whose functions these test)
 *
 * Wine has no conformance test of these: this one calls, from a
 * Windows 3.1 program, the DPMI functions that Wine's int31.c gives
 * 16-bit programs and that Windows 3.1's own DPMI host has, and checks
 * them against each other and against KERNEL's selector functions: LDT
 * descriptors (0000h to 0003h, 0006h to 0008h, 000Ah, 000Bh), DOS memory
 * (0100h to 0102h), interrupt vectors (0200h, 0204h), a real-mode
 * interrupt (0300h), the version (0400h), free memory (0500h), linear
 * memory (0501h, 0502h) and the page size (0604h). It also asks INT 2Fh
 * for a VxD's entry point (1684h).
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

/* The registers in and out of INT 31h. */
struct dpmiregs
{
    WORD ax, bx, cx, dx, si, di, es, flags;
};

#define CARRY(r) ((r)->flags & 1)

static void dpmi(struct dpmiregs *r)
{
    WORD ax_ = r->ax, bx_ = r->bx, cx_ = r->cx, dx_ = r->dx;
    WORD si_ = r->si, di_ = r->di, es_ = r->es, fl_;

    _asm {
        push es
        push si
        push di
        mov ax, ax_
        mov bx, bx_
        mov cx, cx_
        mov dx, dx_
        mov si, si_
        mov di, di_
        mov es, es_
        int 31h
        pushf
        pop fl_
        mov ax_, ax
        mov bx_, bx
        mov cx_, cx
        mov dx_, dx
        mov si_, si
        mov di_, di
        mov es_, es
        pop di
        pop si
        pop es
    }
    r->ax = ax_; r->bx = bx_; r->cx = cx_; r->dx = dx_;
    r->si = si_; r->di = di_; r->es = es_; r->flags = fl_;
}

static void dpmi_call(struct dpmiregs *r, WORD ax)
{
    _fmemset(r, 0, sizeof(*r));
    r->ax = ax;
}

static WORD ahincr(void)
{
    extern void FAR PASCAL __AHINCR(void);
    return OFFSETOF((void FAR *)__AHINCR);
}

static WORD current_ds(void)
{
    WORD ds_;
    _asm mov ds_, ds
    return ds_;
}

static WORD current_cs(void)
{
    WORD cs_;
    _asm mov cs_, cs
    return cs_;
}

static DWORD get_base(WORD sel, int *failed)
{
    struct dpmiregs r;
    dpmi_call(&r, 0x0006);
    r.bx = sel;
    dpmi(&r);
    *failed = CARRY(&r);
    return MAKELONG(r.dx, r.cx);
}

static void test_version(void)
{
    struct dpmiregs r;
    DWORD flags = GetWinFlags();

    dpmi_call(&r, 0x0400);
    dpmi(&r);
    ok(!CARRY(&r), "0400h failed\n");
    /* Windows 3.1's host is DPMI 0.9. */
    ok(r.ax == 0x005a || broken((r.ax >> 8) == 1),
       "DPMI version %x\n", r.ax);
    ok((r.cx & 0xff) >= 2 && (r.cx & 0xff) <= 6, "processor %x\n", r.cx & 0xff);
    if (flags & WF_CPU486)
        ok((r.cx & 0xff) == 4, "processor %x, GetWinFlags %lx\n", r.cx & 0xff, flags);
    else if (flags & WF_CPU386)
        ok((r.cx & 0xff) == 3, "processor %x, GetWinFlags %lx\n", r.cx & 0xff, flags);
    /* Real mode runs in virtual 8086 mode in enhanced mode. */
    if (flags & WF_ENHANCED)
        ok(r.bx & 2, "flags %x without virtual 8086 mode\n", r.bx);
    ok((r.dx >> 8) != 0 && (r.dx & 0xff) != 0, "PICs at %x\n", r.dx);
}

static void test_descriptors(void)
{
    struct dpmiregs r;
    WORD sel, i, incr;
    DWORD base;
    int failed;
    BYTE FAR *p;
    static BYTE data[64];
    BYTE desc[8];

    /* The increment between selectors, as KERNEL's __AHINCR. */
    dpmi_call(&r, 0x0003);
    dpmi(&r);
    incr = r.ax;
    ok(!CARRY(&r) && incr == ahincr(), "0003h gave %x, __AHINCR %x\n", r.ax,
       ahincr());

    /* Three descriptors, one after the other. */
    dpmi_call(&r, 0x0000);
    r.cx = 3;
    dpmi(&r);
    ok(!CARRY(&r) && r.ax != 0, "0000h failed: %x\n", r.ax);
    if (CARRY(&r)) return;
    sel = r.ax;
    ok((sel & 7) == 7, "selector %x isn't an LDT one of ring 3\n", sel);

    /* Base and limit, through DPMI and KERNEL. */
    base = GetSelectorBase(current_ds()) + OFFSETOF((BYTE FAR *)data);
    for (i = 0; i < 64; i++) data[i] = (BYTE)(i + 1);
    dpmi_call(&r, 0x0007);
    r.bx = sel;
    r.cx = HIWORD(base);
    r.dx = LOWORD(base);
    dpmi(&r);
    ok(!CARRY(&r), "0007h failed: %x\n", r.ax);
    dpmi_call(&r, 0x0008);
    r.bx = sel;
    r.cx = 0;
    r.dx = 63;
    dpmi(&r);
    ok(!CARRY(&r), "0008h failed: %x\n", r.ax);
    ok(get_base(sel, &failed) == base && !failed, "0006h gave %lx, set %lx\n",
       get_base(sel, &failed), base);
    ok(GetSelectorBase(sel) == base, "GetSelectorBase %lx, set %lx\n",
       GetSelectorBase(sel), base);
    ok(GetSelectorLimit(sel) == 63, "GetSelectorLimit %lx\n", GetSelectorLimit(sel));
    ok(get_base(current_ds(), &failed) == GetSelectorBase(current_ds()),
       "DS's base %lx, GetSelectorBase %lx\n", get_base(current_ds(), &failed),
       GetSelectorBase(current_ds()));

    /* The descriptor itself (000Bh). */
    _fmemset(desc, 0, sizeof(desc));
    dpmi_call(&r, 0x000b);
    r.bx = sel;
    r.es = SELECTOROF((BYTE FAR *)desc);
    r.di = OFFSETOF((BYTE FAR *)desc);
    dpmi(&r);
    ok(!CARRY(&r), "000Bh failed: %x\n", r.ax);
    ok(*(WORD *)desc == 63 && (desc[6] & 0x0f) == 0, "descriptor's limit %x\n",
       *(WORD *)desc);
    ok(MAKELONG(*(WORD *)(desc + 2), desc[4] | (desc[7] << 8)) == base,
       "descriptor's base %lx, set %lx\n",
       MAKELONG(*(WORD *)(desc + 2), desc[4] | (desc[7] << 8)), base);
    ok((desc[5] & 0x90) == 0x90 && (desc[5] & 0x60) == 0x60,
       "descriptor's access %x: not a present data segment of ring 3\n", desc[5]);

    /* Through the new selector. */
    p = (BYTE FAR *)MAKELP(sel, 0);
    ok(p[0] == 1 && p[63] == 64, "selector reads %x %x\n", p[0], p[63]);
    p[5] = 0x99;
    ok(data[5] == 0x99, "write through the selector not seen\n");

    /* The other two, at bases of their own. */
    for (i = 1; i < 3; i++)
    {
        dpmi_call(&r, 0x0007);
        r.bx = sel + i * incr;
        r.cx = HIWORD(base + i * 16);
        r.dx = LOWORD(base + i * 16);
        dpmi(&r);
        ok(!CARRY(&r), "0007h on %x failed: %x\n", sel + i * incr, r.ax);
        dpmi_call(&r, 0x0008);
        r.bx = sel + i * incr;
        r.dx = 15;
        dpmi(&r);
        p = (BYTE FAR *)MAKELP(sel + i * incr, 0);
        ok(p[0] == i * 16 + 1, "selector %x reads %x\n", sel + i * incr, p[0]);
    }

    /* An alias (000Ah) of our code segment is data. */
    dpmi_call(&r, 0x000a);
    r.bx = current_cs();
    dpmi(&r);
    ok(!CARRY(&r) && r.ax != 0, "000Ah failed: %x\n", r.ax);
    if (!CARRY(&r))
    {
        WORD alias = r.ax;
        ok(GetSelectorBase(alias) == GetSelectorBase(current_cs()),
           "alias's base %lx, CS's %lx\n", GetSelectorBase(alias),
           GetSelectorBase(current_cs()));
        ok(GetSelectorLimit(alias) == GetSelectorLimit(current_cs()),
           "alias's limit %lx, CS's %lx\n", GetSelectorLimit(alias),
           GetSelectorLimit(current_cs()));
        p = (BYTE FAR *)MAKELP(alias, OFFSETOF((void FAR *)test_descriptors));
        ok(!_fmemcmp(p, (void FAR *)test_descriptors, 16),
           "alias doesn't read our code\n");
        dpmi_call(&r, 0x0001);
        r.bx = alias;
        dpmi(&r);
        ok(!CARRY(&r), "0001h on the alias failed: %x\n", r.ax);
    }

    for (i = 0; i < 3; i++)
    {
        dpmi_call(&r, 0x0001);
        r.bx = sel + i * incr;
        dpmi(&r);
        ok(!CARRY(&r), "0001h on %x failed: %x\n", sel + i * incr, r.ax);
    }

    /* A real-mode segment's selector (0002h). */
    dpmi_call(&r, 0x0002);
    r.bx = 0x0040;
    dpmi(&r);
    ok(!CARRY(&r), "0002h failed: %x\n", r.ax);
    if (!CARRY(&r))
    {
        WORD bios = r.ax;
        ok(GetSelectorBase(bios) == 0x400, "segment 40h's selector has base %lx\n",
           GetSelectorBase(bios));
        ok(GetSelectorLimit(bios) == 0xffff, "segment 40h's selector has limit %lx\n",
           GetSelectorLimit(bios));
        p = (BYTE FAR *)MAKELP(bios, 0x13);
        ok(*(WORD FAR *)p >= 256 && *(WORD FAR *)p <= 640,
           "BIOS says %u KB of memory\n", *(WORD FAR *)p);
        dpmi_call(&r, 0x0002);
        r.bx = 0x0040;
        dpmi(&r);
        ok(r.ax == bios, "0002h again gave %x, not %x\n", r.ax, bios);
    }
}

static void test_dos_memory(void)
{
    struct dpmiregs r;
    WORD seg, sel, alias;
    BYTE FAR *p;
    BYTE FAR *q;

    dpmi_call(&r, 0x0100);
    r.bx = 64; /* 1 KB */
    dpmi(&r);
    ok(!CARRY(&r), "0100h failed: %x, %x paragraphs free\n", r.ax, r.bx);
    if (CARRY(&r)) return;
    seg = r.ax;
    sel = r.dx;
    ok(seg != 0 && seg < 0xa000, "DOS memory at segment %x\n", seg);
    ok(GetSelectorBase(sel) == (DWORD)seg << 4, "selector's base %lx, segment %x\n",
       GetSelectorBase(sel), seg);
    ok(GetSelectorLimit(sel) >= 1023, "selector's limit %lx\n", GetSelectorLimit(sel));
    p = (BYTE FAR *)MAKELP(sel, 0);
    p[0] = 0x5a;
    p[1023] = 0xa5;

    /* The same memory through a selector of the segment. */
    alias = AllocSelector(sel);
    SetSelectorBase(alias, (DWORD)seg << 4);
    q = (BYTE FAR *)MAKELP(alias, 0);
    ok(q[0] == 0x5a && q[1023] == 0xa5, "segment %x reads %x %x\n", seg, q[0],
       q[1023]);
    FreeSelector(alias);

    /* DOS's arena header before it gives its size. */
    alias = AllocSelector(sel);
    SetSelectorBase(alias, ((DWORD)seg - 1) << 4);
    q = (BYTE FAR *)MAKELP(alias, 0);
    ok((q[0] == 'M' || q[0] == 'Z') && *(WORD FAR *)(q + 3) >= 64,
       "arena header %x, %x paragraphs\n", q[0], *(WORD FAR *)(q + 3));
    FreeSelector(alias);

    /* Grow it in place (0102h). */
    dpmi_call(&r, 0x0102);
    r.bx = 128;
    r.dx = sel;
    dpmi(&r);
    ok(!CARRY(&r), "0102h failed: %x\n", r.ax);
    if (!CARRY(&r))
        ok(GetSelectorLimit(sel) >= 2047, "limit after growing %lx\n",
           GetSelectorLimit(sel));

    dpmi_call(&r, 0x0101);
    r.dx = sel;
    dpmi(&r);
    ok(!CARRY(&r), "0101h failed: %x\n", r.ax);

    /* Too much fails, and says how much there is. */
    dpmi_call(&r, 0x0100);
    r.bx = 0xffff;
    dpmi(&r);
    ok(CARRY(&r), "0100h of 1 MB succeeded\n");
    ok(r.ax == 8, "0100h of 1 MB gave error %x\n", r.ax);
    ok(r.bx > 64 && r.bx < 0xa000, "0100h says %x paragraphs are free\n", r.bx);
}

static void test_vectors(void)
{
    struct dpmiregs r;
    WORD sel;
    DWORD FAR *ivt;
    DWORD vector;

    /* The real-mode vector of INT 21h, as the vector table has it. */
    dpmi_call(&r, 0x0200);
    r.bx = 0x21;
    dpmi(&r);
    ok(!CARRY(&r), "0200h failed\n");
    vector = MAKELONG(r.dx, r.cx);
    ok(vector != 0, "real-mode INT 21h at 0\n");
    sel = AllocSelector(current_ds());
    SetSelectorBase(sel, 0);
    SetSelectorLimit(sel, 0x3ff);
    ivt = (DWORD FAR *)MAKELP(sel, 0);
    ok(ivt[0x21] == vector, "vector table has %lx, 0200h %lx\n", ivt[0x21], vector);
    FreeSelector(sel);

    /* The protected-mode one is there. */
    dpmi_call(&r, 0x0204);
    r.bx = 0x21;
    dpmi(&r);
    ok(!CARRY(&r), "0204h failed\n");
    ok(r.cx != 0, "protected-mode INT 21h at %x:%x\n", r.cx, r.dx);
    ok((r.cx & 4) != 0, "protected-mode INT 21h's selector %x isn't an LDT one\n",
       r.cx);
}

/* DPMI's real-mode call structure. */
struct rmcall
{
    DWORD edi, esi, ebp, reserved, ebx, edx, ecx, eax;
    WORD flags, es, ds, fs, gs, ip, cs, sp, ss;
};

static void test_real_mode_int(void)
{
    struct dpmiregs r;
    static struct rmcall rm;
    WORD bios;
    WORD dosver;

    /* INT 12h: the memory size the BIOS has at 40h:13h. */
    _fmemset(&rm, 0, sizeof(rm));
    dpmi_call(&r, 0x0300);
    r.bx = 0x12;
    r.es = SELECTOROF((void FAR *)&rm);
    r.di = OFFSETOF((void FAR *)&rm);
    dpmi(&r);
    ok(!CARRY(&r), "0300h of INT 12h failed: %x\n", r.ax);
    dpmi_call(&r, 0x0002);
    r.bx = 0x0040;
    dpmi(&r);
    bios = r.ax;
    ok((WORD)rm.eax == *(WORD FAR *)MAKELP(bios, 0x13),
       "INT 12h gave %lx, the BIOS has %x\n", rm.eax,
       *(WORD FAR *)MAKELP(bios, 0x13));

    /* INT 21h AH=30h in real mode: the version Windows has. */
    _fmemset(&rm, 0, sizeof(rm));
    rm.eax = 0x3000;
    dpmi_call(&r, 0x0300);
    r.bx = 0x21;
    r.es = SELECTOROF((void FAR *)&rm);
    r.di = OFFSETOF((void FAR *)&rm);
    dpmi(&r);
    ok(!CARRY(&r), "0300h of INT 21h failed: %x\n", r.ax);
    _asm {
        mov ax, 3000h
        int 21h
        mov dosver, ax
    }
    ok((WORD)rm.eax == dosver, "real-mode DOS version %lx, protected-mode %x\n",
       rm.eax, dosver);
    ok(LOBYTE(rm.eax) >= 3, "real-mode DOS version %lx\n", rm.eax);
}

static void test_memory(void)
{
    struct dpmiregs r;
    static DWORD info[12];
    DWORD linear, handle;
    WORD sel;
    volatile BYTE FAR *vp;

    _fmemset(info, 0xff, sizeof(info));
    dpmi_call(&r, 0x0500);
    r.es = SELECTOROF((void FAR *)info);
    r.di = OFFSETOF((void FAR *)info);
    dpmi(&r);
    ok(!CARRY(&r), "0500h failed\n");
    ok(info[0] != 0 && info[0] != 0xffffffffL, "largest free block %lx\n", info[0]);
    ok(info[0] >= 0x10000L, "largest free block %lx\n", info[0]);

    dpmi_call(&r, 0x0604);
    dpmi(&r);
    ok(!CARRY(&r) && r.bx == 0 && r.cx == 4096, "0604h gave %x:%x, carry %d\n",
       r.bx, r.cx, CARRY(&r));

    /* 64 KB of linear memory (0501h), reached through a selector. */
    dpmi_call(&r, 0x0501);
    r.bx = 1;
    r.cx = 0;
    dpmi(&r);
    ok(!CARRY(&r), "0501h failed: %x\n", r.ax);
    if (CARRY(&r)) return;
    linear = MAKELONG(r.cx, r.bx);
    handle = MAKELONG(r.di, r.si);
    ok(linear != 0, "0501h gave address 0\n");
    /* Windows's host gives whole pages. */
    ok((linear & 0xfff) == 0, "0501h gave unaligned address %lx\n", linear);

    dpmi_call(&r, 0x0000);
    r.cx = 1;
    dpmi(&r);
    sel = r.ax;
    dpmi_call(&r, 0x0007);
    r.bx = sel;
    r.cx = HIWORD(linear);
    r.dx = LOWORD(linear);
    dpmi(&r);
    dpmi_call(&r, 0x0008);
    r.bx = sel;
    r.cx = 0;
    r.dx = 0xffff;
    dpmi(&r);
    ok(!CARRY(&r), "0008h failed\n");
    /* Byte by byte: a word at FFFFh would be past the limit. */
    vp = (volatile BYTE FAR *)MAKELP(sel, 0);
    vp[0] = 0x11;
    vp[0x8000] = 0x33;
    vp[0xffff] = 0x22;
    ok(vp[0] == 0x11 && vp[0x8000] == 0x33 && vp[0xffff] == 0x22,
       "linear memory reads %x %x %x\n", vp[0], vp[0x8000], vp[0xffff]);

    dpmi_call(&r, 0x0001);
    r.bx = sel;
    dpmi(&r);
    dpmi_call(&r, 0x0502);
    r.si = HIWORD(handle);
    r.di = LOWORD(handle);
    dpmi(&r);
    ok(!CARRY(&r), "0502h failed: %x\n", r.ax);
}

static void test_errors(void)
{
    struct dpmiregs r;

    /* Freeing a selector that isn't allocated. */
    dpmi_call(&r, 0x0000);
    r.cx = 1;
    dpmi(&r);
    if (!CARRY(&r))
    {
        WORD sel = r.ax;
        /* A segment register that holds the selector is zeroed. */
        dpmi_call(&r, 0x0001);
        r.bx = sel;
        r.es = sel;
        dpmi(&r);
        ok(!CARRY(&r), "0001h failed\n");
        ok(r.es == 0, "ES is %x after 0001h freed it\n", r.es);
        dpmi_call(&r, 0x0001);
        r.bx = sel;
        dpmi(&r);
        ok(CARRY(&r), "0001h of a free selector succeeded\n");
    }
    /* A function there isn't. */
    dpmi_call(&r, 0x0e5e);
    dpmi(&r);
    ok(CARRY(&r), "INT 31h AX=0E5Eh succeeded\n");
}

/* INT 2Fh AX=1684h, the API entry point of a VxD that isn't there:
 * 0:0. Shadowgate's sound library asks for two and calls what it gets
 * unless that is 0:0. */
static void test_vxd_entry(void)
{
    WORD es_, di_;

    _asm {
        push es
        push di
        mov ax, ds
        mov es, ax
        mov di, ax
        mov ax, 1684h
        mov bx, 21E1h
        int 2Fh
        mov es_, es
        mov di_, di
        pop di
        pop es
    }
    ok(es_ == 0 && di_ == 0, "VxD 21E1h's entry point is %04x:%04x\n", es_, di_);
}

START_TEST(dpmi)
{
    test_version();
    test_descriptors();
    test_dos_memory();
    test_vectors();
    test_real_mode_int();
    test_memory();
    test_errors();
    test_vxd_entry();
}
