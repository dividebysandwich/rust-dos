/*
 * Unit test suite for memory allocation functions.
 *
 * Copyright 2002 Geoffrey Hausheer
 *
 * Ported to Win16 from Wine's dlls/kernel32/tests/alloc.c: its Global
 * and Local tests, on KERNEL's global heap and on the local heap in the
 * task's DS, without the Heap and Virtual functions, SetLastError and
 * GetLastError, which Windows 3.1 doesn't have. Local blocks are at
 * most 64 KB there, so the local test asks for less. Added are checks
 * of what Windows 3.1 programs rely on: handles and selectors of fixed
 * and moveable blocks, lock counts in GlobalFlags and LocalFlags,
 * discardable blocks, blocks of more than 64 KB reached through
 * __AHINCR, LocalInit on a heap in a segment of its own, the selector
 * functions, ES zeroed when the selector it holds is freed,
 * GlobalDosAlloc and GetFreeSpace.
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

/* KERNEL's constants: their values are the offsets of far pointers. */
extern void FAR PASCAL __AHINCR(void);
extern void FAR PASCAL __AHSHIFT(void);

static WORD ahincr(void)
{
    return OFFSETOF((void FAR *)__AHINCR);
}

/* The byte at offset `off` of the block whose first selector is `sel`. */
static BYTE FAR *huge_at(WORD sel, DWORD off)
{
    return (BYTE FAR *)MAKELP(sel + (WORD)(off >> 16) * ahincr(),
                              (WORD)off);
}

/* Whether the `len` bytes from `off` in the block at `sel` are all
 * zero; a segment at a time. */
static int all_zero(WORD sel, DWORD off, DWORD len)
{
    while (len)
    {
        BYTE FAR *p = huge_at(sel, off);
        DWORD n = 0x10000L - (WORD)off;
        WORD i;
        if (n > len) n = len;
        for (i = 0; i < (WORD)(n - 1); i++)
            if (p[i]) return 0;
        if (p[(WORD)(n - 1)]) return 0;
        off += n;
        len -= n;
    }
    return 1;
}

static void test_Global(void)
{
    DWORD memchunk;
    HGLOBAL mem1, mem2, mem2a;
    BYTE FAR *mem2ptr;
    DWORD handle;
    memchunk = 100000L;

/* Check that a normal alloc works */
    mem1 = GlobalAlloc(0, memchunk);
    ok(mem1 != NULL, "GlobalAlloc failed\n");
    if (mem1) {
      ok(GlobalSize(mem1) >= memchunk,
         "GlobalAlloc should return a big enough memory block\n");
    }

/* Check that a 'zeroing' alloc works */
    mem2 = GlobalAlloc(GMEM_ZEROINIT, memchunk);
    ok(mem2 != NULL, "GlobalAlloc failed\n");
    if (mem2) {
      ok(GlobalSize(mem2) >= memchunk,
         "GlobalAlloc should return a big enough memory block\n");
      mem2ptr = GlobalLock(mem2);
      /* A fixed block's handle is its selector. */
      ok(SELECTOROF(mem2ptr) == (WORD)mem2 && OFFSETOF(mem2ptr) == 0,
         "GlobalLock should have returned the same memory as was allocated"
         " (%x, got %x:%x)\n", mem2, SELECTOROF(mem2ptr), OFFSETOF(mem2ptr));
      if (mem2ptr) {
        ok(all_zero(SELECTOROF(mem2ptr), 0, memchunk),
           "GlobalAlloc should have zeroed out its allocated memory\n");
      }
      GlobalUnlock(mem2);
    }
/* Check that GlobalReAlloc works */
/* Check that we can change GMEM_FIXED to GMEM_MOVEABLE */
    mem2a = GlobalReAlloc(mem2, 0, GMEM_MODIFY | GMEM_MOVEABLE);
    if (mem2a != NULL) {
      mem2 = mem2a;
      mem2ptr = GlobalLock(mem2a);
      ok(mem2ptr != NULL, "Converting from FIXED to MOVEABLE didn't REALLY work\n");
      GlobalUnlock(mem2a);
    }

/* Check that ReAllocing memory works as expected */
    mem2a = GlobalReAlloc(mem2, 2 * memchunk, GMEM_MOVEABLE | GMEM_ZEROINIT);
    ok(mem2a != NULL, "GlobalReAlloc failed\n");
    if (mem2a) {
      ok(GlobalSize(mem2a) >= 2 * memchunk, "GlobalReAlloc failed\n");
      mem2ptr = GlobalLock(mem2a);
      ok(mem2ptr != NULL, "GlobalLock Failed\n");
      if (mem2ptr) {
        ok(all_zero(SELECTOROF(mem2ptr), memchunk, memchunk),
           "GlobalReAlloc should have zeroed out its allocated memory\n");

/* Check that GlobalHandle works */
        handle = GlobalHandle(SELECTOROF(mem2ptr));
        ok(LOWORD(handle) == (WORD)mem2a,
           "GlobalHandle didn't return the correct memory handle (%lx for %x)\n",
           handle, mem2a);
        ok(HIWORD(handle) == SELECTOROF(mem2ptr),
           "GlobalHandle didn't return the selector (%lx for %x)\n",
           handle, SELECTOROF(mem2ptr));

/* Check that we can't discard locked memory */
        if (GlobalDiscard(mem2a) == NULL) {
          GlobalUnlock(mem2a);
        }
      }
    }
    if (mem1) {
      ok(GlobalFree(mem1) == NULL, "GlobalFree failed\n");
    }
    if (mem2a) {
      ok(GlobalFree(mem2a) == NULL, "GlobalFree failed\n");
    } else {
      ok(GlobalFree(mem2) == NULL, "GlobalFree failed\n");
    }
}

/* Handles, selectors and flags of fixed and moveable global blocks. */
static void test_global_handles(void)
{
    HGLOBAL fixed, moveable;
    BYTE FAR *p;
    DWORD handle;
    UINT flags;

    fixed = GlobalAlloc(GMEM_FIXED, 100);
    ok(fixed != NULL, "GlobalAlloc(GMEM_FIXED) failed\n");
    ok(GlobalSize(fixed) >= 100, "GlobalSize %lx\n", GlobalSize(fixed));
    p = GlobalLock(fixed);
    ok(SELECTOROF(p) == (WORD)fixed && OFFSETOF(p) == 0,
       "fixed block %x locked at %x:%x\n", fixed, SELECTOROF(p), OFFSETOF(p));
    handle = GlobalHandle((WORD)fixed);
    ok(LOWORD(handle) == (WORD)fixed && HIWORD(handle) == (WORD)fixed,
       "GlobalHandle(%x) = %lx\n", fixed, handle);
    flags = GlobalFlags(fixed);
    ok(!(flags & (GMEM_DISCARDABLE | GMEM_DISCARDED)),
       "GlobalFlags of a fixed block %x\n", flags);
    GlobalUnlock(fixed);
    ok(GlobalFree(fixed) == NULL, "GlobalFree failed\n");

    moveable = GlobalAlloc(GMEM_MOVEABLE, 100);
    ok(moveable != NULL, "GlobalAlloc(GMEM_MOVEABLE) failed\n");
    p = GlobalLock(moveable);
    ok(p != NULL && OFFSETOF(p) == 0, "GlobalLock gave %x:%x\n",
       SELECTOROF(p), OFFSETOF(p));
    /* A moveable block's handle is one less than its selector. */
    ok(SELECTOROF(p) == (WORD)moveable + 1,
       "moveable block %x locked at %x:%x\n", moveable, SELECTOROF(p),
       OFFSETOF(p));
    handle = GlobalHandle(SELECTOROF(p));
    ok(LOWORD(handle) == (WORD)moveable && HIWORD(handle) == SELECTOROF(p),
       "GlobalHandle(%x) = %lx\n", SELECTOROF(p), handle);
    handle = GlobalHandle((WORD)moveable);
    ok(LOWORD(handle) == (WORD)moveable && HIWORD(handle) == SELECTOROF(p),
       "GlobalHandle(%x) = %lx\n", moveable, handle);
    flags = GlobalFlags(moveable);
    /* Windows 3.1 counts locks of discardable blocks only. */
    ok((flags & GMEM_LOCKCOUNT) == 1 || broken((flags & GMEM_LOCKCOUNT) == 0),
       "GlobalFlags of a locked moveable block %x\n", flags);
    ok(!(flags & (GMEM_DISCARDABLE | GMEM_DISCARDED)),
       "GlobalFlags of a moveable block %x\n", flags);
    p[0] = 0x5a;
    p[99] = 0xa5;
    GlobalUnlock(moveable);
    ok((GlobalFlags(moveable) & GMEM_LOCKCOUNT) == 0,
       "GlobalFlags after GlobalUnlock %x\n", GlobalFlags(moveable));

    /* Growing a moveable block keeps its contents. */
    moveable = GlobalReAlloc(moveable, 5000, GMEM_MOVEABLE | GMEM_ZEROINIT);
    ok(moveable != NULL, "GlobalReAlloc failed\n");
    ok(GlobalSize(moveable) >= 5000, "GlobalSize %lx\n", GlobalSize(moveable));
    p = GlobalLock(moveable);
    ok(p != NULL && p[0] == 0x5a && p[99] == 0xa5,
       "contents not kept: %x %x\n", p ? p[0] : 0, p ? p[99] : 0);
    ok(p != NULL && all_zero(SELECTOROF(p), 128, 5000 - 128),
       "GlobalReAlloc didn't zero the new part\n");
    GlobalUnlock(moveable);

    /* Shrinking too. */
    moveable = GlobalReAlloc(moveable, 50, GMEM_MOVEABLE);
    ok(moveable != NULL, "GlobalReAlloc failed\n");
    ok(GlobalSize(moveable) >= 50 && GlobalSize(moveable) < 5000,
       "GlobalSize %lx\n", GlobalSize(moveable));
    p = GlobalLock(moveable);
    ok(p != NULL && p[0] == 0x5a, "contents not kept\n");
    GlobalUnlock(moveable);
    ok(GlobalFree(moveable) == NULL, "GlobalFree failed\n");

    /* A moveable block of no size is made discarded. */
    moveable = GlobalAlloc(GMEM_MOVEABLE, 0);
    ok(moveable != NULL, "GlobalAlloc(GMEM_MOVEABLE, 0) failed\n");
    if (moveable)
    {
        ok(GlobalFlags(moveable) & GMEM_DISCARDED,
           "GlobalFlags of an empty block %x\n", GlobalFlags(moveable));
        ok(GlobalSize(moveable) == 0, "GlobalSize %lx\n", GlobalSize(moveable));
        ok(GlobalLock(moveable) == NULL, "GlobalLock of an empty block\n");
        moveable = GlobalReAlloc(moveable, 64, GMEM_MOVEABLE);
        ok(moveable != NULL, "GlobalReAlloc of an empty block failed\n");
        ok(GlobalSize(moveable) >= 64, "GlobalSize %lx\n", GlobalSize(moveable));
        ok(!(GlobalFlags(moveable) & GMEM_DISCARDED),
           "GlobalFlags %x\n", GlobalFlags(moveable));
        p = GlobalLock(moveable);
        ok(p != NULL, "GlobalLock failed\n");
        GlobalUnlock(moveable);
        ok(GlobalFree(moveable) == NULL, "GlobalFree failed\n");
    }
}

/* Lock counts and discarding of discardable global blocks. */
static void test_global_discard(void)
{
    HGLOBAL h, h2;
    BYTE FAR *p;
    WORD sel;
    UINT flags;

    h = GlobalAlloc(GMEM_MOVEABLE | GMEM_DISCARDABLE, 1000);
    ok(h != NULL, "GlobalAlloc failed\n");
    if (!h) return;
    flags = GlobalFlags(h);
    ok(flags == GMEM_DISCARDABLE, "GlobalFlags %x\n", flags);

    p = GlobalLock(h);
    ok(p != NULL, "GlobalLock failed\n");
    sel = SELECTOROF(p);
    ok(GlobalLock(h) == (void FAR *)p, "second GlobalLock moved the block\n");
    flags = GlobalFlags(h);
    ok(flags == (GMEM_DISCARDABLE | 2), "GlobalFlags after two locks %x\n",
       flags);

    /* A locked block can't be discarded. */
    ok(GlobalDiscard(h) == NULL, "discarded a locked block\n");
    ok(!(GlobalFlags(h) & GMEM_DISCARDED), "locked block discarded\n");

    ok(GlobalUnlock(h) != 0, "GlobalUnlock with a lock left gave 0\n");
    ok((GlobalFlags(h) & GMEM_LOCKCOUNT) == 1, "GlobalFlags %x\n",
       GlobalFlags(h));
    ok(GlobalUnlock(h) == 0, "last GlobalUnlock gave nonzero\n");
    ok((GlobalFlags(h) & GMEM_LOCKCOUNT) == 0, "GlobalFlags %x\n",
       GlobalFlags(h));

    h2 = GlobalDiscard(h);
    ok(h2 == h, "GlobalDiscard gave %x for %x\n", h2, h);
    flags = GlobalFlags(h);
    ok(flags == (GMEM_DISCARDABLE | GMEM_DISCARDED),
       "GlobalFlags of a discarded block %x\n", flags);
    ok(GlobalSize(h) == 0, "GlobalSize of a discarded block %lx\n",
       GlobalSize(h));
    ok(GlobalLock(h) == NULL, "GlobalLock of a discarded block\n");
    /* The handle stays valid, with its selector. */
    ok(LOWORD(GlobalHandle(sel)) == (WORD)h,
       "GlobalHandle(%x) of a discarded block %lx\n", sel,
       GlobalHandle(sel));

    /* Reallocating brings it back, under the same handle. */
    h2 = GlobalReAlloc(h, 2000, GMEM_MOVEABLE);
    ok(h2 == h, "GlobalReAlloc of a discarded block gave %x for %x\n", h2, h);
    ok(GlobalSize(h) >= 2000, "GlobalSize %lx\n", GlobalSize(h));
    ok(GlobalFlags(h) == GMEM_DISCARDABLE, "GlobalFlags %x\n", GlobalFlags(h));
    p = GlobalLock(h);
    ok(p != NULL && SELECTOROF(p) == sel, "GlobalLock gave %x:%x, not %x\n",
       SELECTOROF(p), OFFSETOF(p), sel);
    GlobalUnlock(h);

    /* GMEM_MODIFY makes a block discardable and not. */
    h2 = GlobalReAlloc(h, 0, GMEM_MODIFY | GMEM_MOVEABLE);
    ok(h2 == h, "GlobalReAlloc(GMEM_MODIFY) gave %x for %x\n", h2, h);
    ok(!(GlobalFlags(h) & GMEM_DISCARDABLE), "GlobalFlags %x\n",
       GlobalFlags(h));
    h2 = GlobalReAlloc(h, 0, GMEM_MODIFY | GMEM_MOVEABLE | GMEM_DISCARDABLE);
    ok(h2 == h, "GlobalReAlloc(GMEM_MODIFY) gave %x for %x\n", h2, h);
    ok(GlobalFlags(h) & GMEM_DISCARDABLE, "GlobalFlags %x\n", GlobalFlags(h));

    ok(GlobalFree(h) == NULL, "GlobalFree failed\n");
}

/* Blocks of more than 64 KB: a selector for each 64 KB, __AHINCR apart. */
static void test_global_huge(void)
{
    HGLOBAL h;
    BYTE FAR *p;
    BYTE FAR *q;
    WORD sel, incr, shift, i;
    DWORD base;
    char buf[16];
    int good;

    incr = ahincr();
    shift = OFFSETOF((void FAR *)__AHSHIFT);
    ok(incr == 8, "__AHINCR %x\n", incr);
    ok(shift == 3, "__AHSHIFT %x\n", shift);
    ok((1 << shift) == incr, "__AHSHIFT %x and __AHINCR %x disagree\n",
       shift, incr);

    h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 0x28000L);
    ok(h != NULL, "GlobalAlloc of 160 KB failed\n");
    if (!h) return;
    ok(GlobalSize(h) >= 0x28000L, "GlobalSize %lx\n", GlobalSize(h));
    p = GlobalLock(h);
    ok(p != NULL && OFFSETOF(p) == 0, "GlobalLock gave %x:%x\n",
       SELECTOROF(p), OFFSETOF(p));
    if (!p) return;
    sel = SELECTOROF(p);

    base = GetSelectorBase(sel);
    ok(GetSelectorBase(sel + incr) == base + 0x10000L,
       "second selector's base %lx, first's %lx\n",
       GetSelectorBase(sel + incr), base);
    ok(GetSelectorBase(sel + 2 * incr) == base + 0x20000L,
       "third selector's base %lx, first's %lx\n",
       GetSelectorBase(sel + 2 * incr), base);
    ok(GetSelectorLimit(sel) >= 0xffffL, "first selector's limit %lx\n",
       GetSelectorLimit(sel));
    ok(GetSelectorLimit(sel + incr) >= 0xffffL,
       "second selector's limit %lx\n", GetSelectorLimit(sel + incr));
    ok(GetSelectorLimit(sel + 2 * incr) >= 0x7fffL,
       "third selector's limit %lx\n", GetSelectorLimit(sel + 2 * incr));

    /* Write across the boundaries through each segment's selector. */
    for (i = 0; i < 16; i++)
    {
        *huge_at(sel, 0xfff8L + i) = (BYTE)(0x10 + i);
        *huge_at(sel, 0x1fff8L + i) = (BYTE)(0x40 + i);
    }
    q = (BYTE FAR *)MAKELP(sel + incr, 0);
    ok(q[0] == 0x18 && q[7] == 0x1f, "second segment starts with %x %x\n",
       q[0], q[7]);
    q = (BYTE FAR *)MAKELP(sel, 0xfff8);
    ok(q[0] == 0x10 && q[7] == 0x17, "first segment ends with %x %x\n",
       q[0], q[7]);
    q = (BYTE FAR *)MAKELP(sel + 2 * incr, 0);
    ok(q[0] == 0x48 && q[7] == 0x4f, "third segment starts with %x %x\n",
       q[0], q[7]);

    /* hmemcpy follows huge pointers across segments. */
    _fmemset(buf, 0, sizeof(buf));
    hmemcpy(buf, MAKELP(sel, 0xfff8), 16);
    good = 1;
    for (i = 0; i < 16; i++)
        if (buf[i] != (char)(0x10 + i)) good = 0;
    ok(good, "hmemcpy across the boundary read %x %x %x\n", buf[0], buf[8],
       buf[15]);
    for (i = 0; i < 16; i++) buf[i] = (char)(0x70 + i);
    hmemcpy(MAKELP(sel, 0xfffc), buf, 16);
    q = (BYTE FAR *)MAKELP(sel + incr, 0);
    ok(q[0] == 0x74 && q[11] == 0x7f, "hmemcpy across the boundary wrote %x %x\n",
       q[0], q[11]);

    /* The rest stays zero. */
    ok(all_zero(sel, 0x10010L, 0xffe8L), "second segment not zero\n");
    ok(all_zero(sel, 0x20008L, 0x7ff8L), "third segment not zero\n");

    /* GlobalHandle of the first selector. */
    ok(LOWORD(GlobalHandle(sel)) == (WORD)h, "GlobalHandle(%x) %lx\n", sel,
       GlobalHandle(sel));

    /* Shrinking below 64 KB. */
    GlobalUnlock(h);
    h = GlobalReAlloc(h, 0x8000L, GMEM_MOVEABLE);
    ok(h != NULL, "GlobalReAlloc to 32 KB failed\n");
    if (h)
    {
        ok(GlobalSize(h) >= 0x8000L && GlobalSize(h) < 0x10000L,
           "GlobalSize %lx\n", GlobalSize(h));
        p = GlobalLock(h);
        ok(p != NULL && p[0x7ff0] == 0, "GlobalLock failed\n");
        GlobalUnlock(h);
        ok(GlobalFree(h) == NULL, "GlobalFree failed\n");
    }
}

/* AllocSelector and friends, on a block of our own. */
static void test_selectors(void)
{
    HGLOBAL h;
    BYTE FAR *p;
    BYTE FAR *q;
    WORD sel, alias, i;
    DWORD base;

    h = GlobalAlloc(GMEM_FIXED, 256);
    ok(h != NULL, "GlobalAlloc failed\n");
    if (!h) return;
    p = GlobalLock(h);
    for (i = 0; i < 256; i++) p[i] = (BYTE)i;
    sel = SELECTOROF(p);
    base = GetSelectorBase(sel);
    ok(base != 0, "GetSelectorBase(%x) gave 0\n", sel);
    ok(GetSelectorLimit(sel) >= 255, "GetSelectorLimit(%x) %lx\n", sel,
       GetSelectorLimit(sel));

    /* A copy of the selector. */
    alias = AllocSelector(sel);
    ok(alias != 0 && alias != sel, "AllocSelector(%x) gave %x\n", sel, alias);
    if (!alias) return;
    ok(GetSelectorBase(alias) == base, "alias's base %lx, not %lx\n",
       GetSelectorBase(alias), base);
    ok(GetSelectorLimit(alias) == GetSelectorLimit(sel),
       "alias's limit %lx, not %lx\n", GetSelectorLimit(alias),
       GetSelectorLimit(sel));
    q = (BYTE FAR *)MAKELP(alias, 0);
    ok(q[0] == 0 && q[200] == 200, "alias reads %x %x\n", q[0], q[200]);
    q[1] = 0x99;
    ok(p[1] == 0x99, "write through the alias not seen: %x\n", p[1]);

    ok(SetSelectorBase(alias, base + 16) == alias,
       "SetSelectorBase didn't return the selector\n");
    ok(GetSelectorBase(alias) == base + 16, "base %lx after setting %lx\n",
       GetSelectorBase(alias), base + 16);
    q = (BYTE FAR *)MAKELP(alias, 0);
    ok(q[0] == 16 && q[100] == 116, "moved alias reads %x %x\n", q[0], q[100]);

    SetSelectorLimit(alias, 15);
    ok(GetSelectorLimit(alias) == 15, "limit %lx after setting 15\n",
       GetSelectorLimit(alias));
    SetSelectorLimit(alias, 0x1ffffL);
    ok(GetSelectorLimit(alias) == 0x1ffffL, "limit %lx after setting 1ffff\n",
       GetSelectorLimit(alias));

    ok(FreeSelector(alias) == 0, "FreeSelector(%x) failed\n", alias);

    /* A selector of no particular block. */
    alias = AllocSelector(0);
    ok(alias != 0, "AllocSelector(0) failed\n");
    if (alias)
    {
        SetSelectorBase(alias, base);
        SetSelectorLimit(alias, 255);
        ok(GetSelectorBase(alias) == base, "base %lx after setting %lx\n",
           GetSelectorBase(alias), base);
        ok(GetSelectorLimit(alias) == 255, "limit %lx after setting 255\n",
           GetSelectorLimit(alias));
        q = (BYTE FAR *)MAKELP(alias, 0);
        ok(q[1] == 0x99 && q[255] == 255, "new selector reads %x %x\n", q[1],
           q[255]);
        ok(FreeSelector(alias) == 0, "FreeSelector(%x) failed\n", alias);
    }

    GlobalUnlock(h);
    ok(GlobalFree(h) == NULL, "GlobalFree failed\n");
}

/* What ES holds after `release` (GlobalFree or FreeSelector) is
 * called with `arg` while ES holds `sel`. */
static WORD es_after(FARPROC release, WORD arg, WORD sel)
{
    WORD es_;
    _asm {
        push es
        mov es, sel
        push arg
        call dword ptr release
        mov es_, es
        pop es
    }
    return es_;
}

/* Freeing a block or a selector zeroes ES when it holds the block's
 * selector, as DPMI's function 0001h does: Johnny Castaway frees a
 * block with its selector in ES and then pushes and pops ES. */
static void test_free_clears_es(void)
{
    HGLOBAL h;
    WORD sel, es_;

    h = GlobalAlloc(GMEM_MOVEABLE, 256);
    ok(h != NULL, "GlobalAlloc failed\n");
    if (!h) return;
    sel = SELECTOROF(GlobalLock(h));
    GlobalUnlock(h);
    es_ = es_after((FARPROC)GlobalFree, (WORD)h, sel);
    ok(es_ == 0, "ES is %x after GlobalFree freed %x\n", es_, sel);

    sel = AllocSelector(0);
    ok(sel != 0, "AllocSelector(0) failed\n");
    if (!sel) return;
    SetSelectorLimit(sel, 15);
    es_ = es_after((FARPROC)FreeSelector, sel, sel);
    ok(es_ == 0, "ES is %x after FreeSelector freed %x\n", es_, sel);
}

/* Conventional memory, for real-mode code. */
static void test_dos_alloc(void)
{
    DWORD r;
    WORD sel, seg, alias;
    BYTE FAR *p;
    BYTE FAR *q;

    r = GlobalDosAlloc(1024);
    ok(r != 0, "GlobalDosAlloc failed\n");
    if (!r) return;
    sel = LOWORD(r);
    seg = HIWORD(r);
    ok(seg != 0 && seg < 0xa000, "GlobalDosAlloc's segment %x\n", seg);
    ok(GetSelectorBase(sel) == (DWORD)seg << 4,
       "selector's base %lx, segment %x\n", GetSelectorBase(sel), seg);
    ok(GetSelectorLimit(sel) >= 1023, "selector's limit %lx\n",
       GetSelectorLimit(sel));
    ok(HIWORD(GlobalHandle(sel)) == sel, "GlobalHandle(%x) %lx\n", sel,
       GlobalHandle(sel));
    ok(GlobalSize((HGLOBAL)sel) >= 1024, "GlobalSize %lx\n",
       GlobalSize((HGLOBAL)sel));

    p = (BYTE FAR *)MAKELP(sel, 0);
    p[0] = 0x12;
    p[1023] = 0x34;

    /* The same memory through a selector of our own at the segment. */
    alias = AllocSelector(sel);
    ok(alias != 0, "AllocSelector failed\n");
    if (alias)
    {
        SetSelectorBase(alias, (DWORD)seg << 4);
        q = (BYTE FAR *)MAKELP(alias, 0);
        ok(q[0] == 0x12 && q[1023] == 0x34, "segment %x holds %x %x\n", seg,
           q[0], q[1023]);
        FreeSelector(alias);
    }

    ok(GlobalDosFree(sel) == 0, "GlobalDosFree(%x) failed\n", sel);
}

static void test_free_space(void)
{
    DWORD space, largest;
    DWORD flags;

    flags = GetWinFlags();
    ok(flags & WF_PMODE, "GetWinFlags %lx without WF_PMODE\n", flags);
    space = GetFreeSpace(0);
    ok(space > 0x10000L, "GetFreeSpace(0) %lx\n", space);
    largest = GlobalCompact(0);
    ok(largest > 0x10000L, "GlobalCompact(0) %lx\n", largest);
    ok(largest <= space || broken(flags & WF_ENHANCED),
       "GlobalCompact(0) %lx more than GetFreeSpace(0) %lx\n", largest, space);
}

static void test_Local(void)
{
    UINT memchunk;
    HLOCAL mem1, mem2, mem2a, mem2b;
    BYTE NEAR *mem2ptr;
    UINT error, i;
    memchunk = 1000;

/* Check that a normal alloc works */
    mem1 = LocalAlloc(0, memchunk);
    ok(mem1 != NULL, "LocalAlloc failed\n");
    if (mem1) {
      ok(LocalSize(mem1) >= memchunk,
         "LocalAlloc should return a big enough memory block\n");
    }

/* Check that a 'zeroing' and lock alloc works */
    mem2 = LocalAlloc(LMEM_ZEROINIT | LMEM_MOVEABLE, memchunk);
    ok(mem2 != NULL, "LocalAlloc failed\n");
    if (mem2) {
      ok(LocalSize(mem2) >= memchunk,
         "LocalAlloc should return a big enough memory block\n");
      mem2ptr = (BYTE NEAR *)LocalLock(mem2);
      ok(mem2ptr != NULL, "LocalLock failed\n");
      if (mem2ptr) {
        error = 0;
        for (i = 0; i < memchunk; i++) {
          if (mem2ptr[i] != 0) {
            error = 1;
          }
        }
        ok(!error, "LocalAlloc should have zeroed out its allocated memory\n");
        error = LocalUnlock(mem2);
        ok(error == 0, "LocalUnlock Failed: rc=%d\n", error);
      }
    }
   mem2a = LocalFree(mem2);
   ok(mem2a == NULL, "LocalFree failed: %x\n", mem2a);

/* Reallocate mem2 as moveable memory */
   mem2 = LocalAlloc(LMEM_MOVEABLE | LMEM_ZEROINIT, memchunk);
   ok(mem2 != NULL, "LocalAlloc failed to create moveable memory\n");

/* Check that ReAllocing memory works as expected */
    mem2a = LocalReAlloc(mem2, 2 * memchunk, LMEM_MOVEABLE | LMEM_ZEROINIT);
    ok(mem2a != NULL, "LocalReAlloc failed\n");
    if (mem2a) {
      ok(LocalSize(mem2a) >= 2 * memchunk, "LocalReAlloc failed\n");
      mem2ptr = (BYTE NEAR *)LocalLock(mem2a);
      ok(mem2ptr != NULL, "LocalLock Failed\n");
      if (mem2ptr) {
        error = 0;
        for (i = 0; i < memchunk; i++) {
          if (mem2ptr[memchunk + i] != 0) {
            error = 1;
          }
        }
        ok(!error, "LocalReAlloc should have zeroed out its allocated memory\n");
/* Check that LocalHandle works */
        mem2b = LocalHandle((UINT)mem2ptr);
        ok(mem2b == mem2a, "LocalHandle didn't return the correct memory handle\n");
/* Check that we can't discard locked memory */
        mem2b = LocalDiscard(mem2a);
        ok(mem2b == NULL, "Discarded memory we shouldn't have\n");
        ok(!LocalUnlock(mem2a), "LocalUnlock Failed\n");
      }
    }
    if (mem1) {
      ok(LocalFree(mem1) == NULL, "LocalFree failed\n");
    }
    if (mem2a) {
      ok(LocalFree(mem2a) == NULL, "LocalFree failed\n");
    } else {
      ok(LocalFree(mem2) == NULL, "LocalFree failed\n");
    }
}

/* Fixed and moveable local blocks: handles, lock counts, discarding. */
static void test_local_handles(void)
{
    HLOCAL fixed, moveable, h2;
    char NEAR *p;
    char NEAR *p2;
    UINT flags;

    fixed = LocalAlloc(LMEM_FIXED, 40);
    ok(fixed != NULL, "LocalAlloc(LMEM_FIXED) failed\n");
    /* A fixed block's handle is its address. */
    p = LocalLock(fixed);
    ok(p == (char NEAR *)fixed, "LocalLock(%x) gave %x\n", fixed, (UINT)p);
    ok(((UINT)fixed & 3) == 0, "fixed block at %x\n", fixed);
    ok(LocalHandle((UINT)p) == fixed, "LocalHandle(%x) %x\n", (UINT)p,
       LocalHandle((UINT)p));
    ok(LocalFlags(fixed) == 0, "LocalFlags of a fixed block %x\n",
       LocalFlags(fixed));
    LocalUnlock(fixed);

    /* Growing a fixed block in place or not at all, unless it may move. */
    lstrcpy(p, "fixed data");
    h2 = LocalReAlloc(fixed, 3000, LMEM_MOVEABLE);
    ok(h2 != NULL, "LocalReAlloc(LMEM_MOVEABLE) of a fixed block failed\n");
    if (h2)
    {
        fixed = h2;
        ok(LocalSize(fixed) >= 3000, "LocalSize %x\n", LocalSize(fixed));
        ok(!lstrcmp((char NEAR *)fixed, "fixed data"), "contents not kept\n");
        ok(LocalFlags(fixed) == 0, "LocalFlags %x\n", LocalFlags(fixed));
    }
    ok(LocalFree(fixed) == NULL, "LocalFree failed\n");

    moveable = LocalAlloc(LMEM_MOVEABLE, 40);
    ok(moveable != NULL, "LocalAlloc(LMEM_MOVEABLE) failed\n");
    ok(((UINT)moveable & 3) == 2, "moveable handle %x\n", moveable);
    ok(LocalFlags(moveable) == 0, "LocalFlags %x\n", LocalFlags(moveable));
    p = LocalLock(moveable);
    ok(p != NULL && p != (char NEAR *)moveable, "LocalLock(%x) gave %x\n",
       moveable, (UINT)p);
    /* The handle points at the block's address. */
    ok(*(UINT NEAR *)moveable == (UINT)p, "handle %x holds %x, block at %x\n",
       moveable, *(UINT NEAR *)moveable, (UINT)p);
    ok(LocalHandle((UINT)p) == moveable, "LocalHandle(%x) %x\n", (UINT)p,
       LocalHandle((UINT)p));
    p2 = LocalLock(moveable);
    ok(p2 == p, "second LocalLock gave %x, not %x\n", (UINT)p2, (UINT)p);
    flags = LocalFlags(moveable);
    ok(flags == 2, "LocalFlags after two locks %x\n", flags);
    ok(LocalUnlock(moveable) != 0, "LocalUnlock with a lock left gave 0\n");
    ok(LocalFlags(moveable) == 1, "LocalFlags %x\n", LocalFlags(moveable));
    ok(LocalUnlock(moveable) == 0, "last LocalUnlock gave nonzero\n");
    ok(LocalFlags(moveable) == 0, "LocalFlags %x\n", LocalFlags(moveable));
    ok(LocalUnlock(moveable) == 0, "LocalUnlock of an unlocked block\n");
    ok(LocalSize(moveable) >= 40, "LocalSize %x\n", LocalSize(moveable));
    ok(LocalFree(moveable) == NULL, "LocalFree failed\n");

    /* Discardable. */
    moveable = LocalAlloc(LMEM_MOVEABLE | LMEM_DISCARDABLE, 100);
    ok(moveable != NULL, "LocalAlloc(LMEM_DISCARDABLE) failed\n");
    if (!moveable) return;
    flags = LocalFlags(moveable);
    ok(flags == LMEM_DISCARDABLE, "LocalFlags of a discardable block %x\n",
       flags);
    p = LocalLock(moveable);
    ok(p != NULL, "LocalLock failed\n");
    ok(LocalFlags(moveable) == (LMEM_DISCARDABLE | 1), "LocalFlags %x\n",
       LocalFlags(moveable));
    ok(LocalDiscard(moveable) == NULL, "discarded a locked block\n");
    LocalUnlock(moveable);
    h2 = LocalDiscard(moveable);
    ok(h2 == moveable, "LocalDiscard gave %x for %x\n", h2, moveable);
    flags = LocalFlags(moveable);
    /* Whether the discardable bits stay with the discarded one is left
     * open: Wine drops them. */
    ok((flags & ~LMEM_DISCARDABLE) == LMEM_DISCARDED,
       "LocalFlags of a discarded block %x\n", flags);
    ok(LocalSize(moveable) == 0, "LocalSize of a discarded block %x\n",
       LocalSize(moveable));
    ok(LocalLock(moveable) == NULL, "LocalLock of a discarded block\n");
    h2 = LocalReAlloc(moveable, 100, LMEM_MOVEABLE);
    ok(h2 == moveable, "LocalReAlloc of a discarded block gave %x for %x\n",
       h2, moveable);
    ok(LocalSize(moveable) >= 100, "LocalSize %x\n", LocalSize(moveable));
    ok(!(LocalFlags(moveable) & LMEM_DISCARDED), "LocalFlags %x\n",
       LocalFlags(moveable));
    ok(LocalLock(moveable) != NULL, "LocalLock failed\n");
    LocalUnlock(moveable);
    ok(LocalFree(moveable) == NULL, "LocalFree failed\n");

    ok(LocalCompact(0) > 0, "LocalCompact(0) gave 0\n");
}

/* The local heap functions work on the heap in the caller's DS: call
 * them with DS set to another segment. */
typedef HLOCAL (FAR PASCAL *LOCALALLOCPROC)(UINT, UINT);
typedef UINT (FAR PASCAL *LOCALSIZEPROC)(HLOCAL);

static HLOCAL local_alloc_in(WORD segsel, UINT flags, UINT size)
{
    LOCALALLOCPROC proc = LocalAlloc;
    HLOCAL result;
    _asm {
        push ds
        push flags
        push size
        mov ax, segsel
        mov ds, ax
        call dword ptr proc
        pop ds
        mov result, ax
    }
    return result;
}

static UINT local_size_in(WORD segsel, HLOCAL h)
{
    LOCALSIZEPROC proc = LocalSize;
    UINT result;
    _asm {
        push ds
        push h
        mov ax, segsel
        mov ds, ax
        call dword ptr proc
        pop ds
        mov result, ax
    }
    return result;
}

static HLOCAL local_free_in(WORD segsel, HLOCAL h)
{
    LOCALSIZEPROC proc = (LOCALSIZEPROC)LocalFree;
    HLOCAL result;
    _asm {
        push ds
        push h
        mov ax, segsel
        mov ds, ax
        call dword ptr proc
        pop ds
        mov result, ax
    }
    return result;
}

static void test_local_init(void)
{
    HGLOBAL h;
    BYTE FAR *p;
    WORD sel;
    HLOCAL a, b;

    h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4096);
    ok(h != NULL, "GlobalAlloc failed\n");
    if (!h) return;
    p = GlobalLock(h);
    sel = SELECTOROF(p);
    ok(LocalInit(sel, 16, 4095), "LocalInit(%x) failed\n", sel);

    a = local_alloc_in(sel, LMEM_FIXED | LMEM_ZEROINIT, 100);
    ok(a != NULL, "LocalAlloc in segment %x failed\n", sel);
    ok((UINT)a >= 16 && (UINT)a < 4096, "LocalAlloc gave %x\n", a);
    ok(local_size_in(sel, a) >= 100, "LocalSize %x\n", local_size_in(sel, a));
    b = local_alloc_in(sel, LMEM_FIXED, 200);
    ok(b != NULL && b != a, "LocalAlloc in segment %x gave %x after %x\n",
       sel, b, a);
    ok((UINT)b >= (UINT)a + 100 || (UINT)b + 200 <= (UINT)a,
       "blocks %x and %x overlap\n", a, b);
    p[(UINT)a] = 0x11;
    ok(local_free_in(sel, a) == NULL, "LocalFree failed\n");
    ok(local_free_in(sel, b) == NULL, "LocalFree failed\n");

    /* The heap in our own DS wasn't touched. */
    a = LocalAlloc(LMEM_FIXED, 10);
    ok(a != NULL, "LocalAlloc in DS failed\n");
    LocalFree(a);

    GlobalUnlock(h);
    ok(GlobalFree(h) == NULL, "GlobalFree failed\n");
}

START_TEST(alloc)
{
    test_Global();
    test_global_handles();
    test_global_discard();
    test_global_huge();
    test_selectors();
    test_free_clears_es();
    test_dos_alloc();
    test_free_space();
    test_Local();
    test_local_handles();
    test_local_init();
}
