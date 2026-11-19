//! Win16 TOOLHELP: the walk of the global heap (GlobalEntryHandle,
//! GlobalEntryModule, GlobalFirst, GlobalNext and GlobalInfo) as a
//! Windows 3.1 program sees it on the built-in Windows. The programs are
//! made byte by byte (tests/common/win16) and end with the number of the
//! first check that failed, 0 when all held.

use crate::win16;

use win16::{CODE, Code, program, run};

/// KERNEL's ordinals the programs call.
const GLOBAL_ALLOC: u16 = 15;
const GLOBAL_LOCK: u16 = 18;
const GLOBAL_SIZE: u16 = 20;
const GET_CURRENT_TASK: u16 = 36;
const GET_EXE_PTR: u16 = 133;
const GET_SELECTOR_BASE: u16 = 186;
const LOCAL_INIT: u16 = 4;

/// TOOLHELP's.
const GLOBAL_FIRST: u16 = 51;
const GLOBAL_NEXT: u16 = 52;
const GLOBAL_INFO: u16 = 53;
const GLOBAL_ENTRY_HANDLE: u16 = 54;
const GLOBAL_ENTRY_MODULE: u16 = 55;

/// GlobalAlloc's flags.
const GMEM_MOVEABLE: u16 = 0x0002;
const GMEM_DISCARDABLE: u16 = 0x0100;

/// GlobalFirst's flags, and GLOBALENTRY's wType values and its wFlags'
/// GF_PDB_OWNER.
const GLOBAL_ALL: u16 = 0;
const GLOBAL_LRU: u16 = 1;
const GLOBAL_FREE: u16 = 2;
const GT_UNKNOWN: u16 = 0;
const GT_DGROUP: u16 = 1;
const GT_CODE: u16 = 3;
const GT_TASK: u16 = 4;
const GT_MODULE: u16 = 6;
const GT_FREE: u16 = 7;
const GF_PDB_OWNER: u16 = 0x0100;

/// Where the programs keep things in their data segment: a block's
/// handle and selector, its size and base, the task, the module, DS,
/// counters, a GLOBALINFO at 30h and a GLOBALENTRY at 40h.
const HANDLE: u16 = 0x10;
const SEL: u16 = 0x12;
const SIZE: u16 = 0x14;
const BASE: u16 = 0x18;
const TASK: u16 = 0x1C;
const MODULE: u16 = 0x1E;
const DS: u16 = 0x20;
const FOUND: u16 = 0x22;
const COUNT: u16 = 0x24;
const FREE: u16 = 0x26;
const INFO: u16 = 0x30;
const ENTRY: u16 = 0x40;

/// GLOBALINFO's and GLOBALENTRY's fields, at their offsets.
const WC_ITEMS: u16 = INFO + 4;
const WC_ITEMS_FREE: u16 = INFO + 6;
const WC_ITEMS_LRU: u16 = INFO + 8;
const DW_ADDRESS: u16 = ENTRY + 4;
const DW_BLOCK_SIZE: u16 = ENTRY + 8;
const H_BLOCK: u16 = ENTRY + 12;
const WC_LOCK: u16 = ENTRY + 14;
const WC_PAGE_LOCK: u16 = ENTRY + 16;
const W_FLAGS: u16 = ENTRY + 18;
const W_HEAP_PRESENT: u16 = ENTRY + 20;
const H_OWNER: u16 = ENTRY + 22;
const W_TYPE: u16 = ENTRY + 24;
const W_DATA: u16 = ENTRY + 26;

fn lo(v: u16) -> u8 {
    v as u8
}

fn hi(v: u16) -> u8 {
    (v >> 8) as u8
}

/// mov word [at],value.
fn set(code: &mut Code, at: u16, value: u16) {
    code.emit(&[0xC7, 0x06, lo(at), hi(at), lo(value), hi(value)]);
}

/// mov [at],ax.
fn store_ax(code: &mut Code, at: u16) {
    code.emit(&[0xA3, lo(at), hi(at)]);
}

/// mov [at],dx.
fn store_dx(code: &mut Code, at: u16) {
    code.emit(&[0x89, 0x16, lo(at), hi(at)]);
}

/// push word [at].
fn push_word(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// End with `fail` unless the last comparison found them equal.
fn or_fail(code: &mut Code, fail: u8) {
    code.emit(&[0x74, 0x06]) // je over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    code.emit(&[0x3D, lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the byte at `at` is `value`.
fn expect_byte(code: &mut Code, at: u16, value: u8, fail: u8) {
    code.emit(&[0x80, 0x3E, lo(at), hi(at), value]);
    or_fail(code, fail);
}

/// End with `fail` unless the words at `at` and `other` are equal.
fn expect_same(code: &mut Code, at: u16, other: u16, fail: u8) {
    code.emit(&[0xA1, lo(at), hi(at)])
        .emit(&[0x3B, 0x06, lo(other), hi(other)]);
    or_fail(code, fail);
}

/// Set GLOBALENTRY's dwSize to `size`.
fn entry_size(code: &mut Code, size: u16) {
    set(code, ENTRY, size);
    set(code, ENTRY + 2, 0);
}

/// GlobalEntryHandle(ds:ENTRY, word [at]).
fn entry_handle(code: &mut Code, at: u16) {
    code.emit(&[0x1E]).push(ENTRY);
    push_word(code, at);
    code.call("TOOLHELP", GLOBAL_ENTRY_HANDLE);
}

/// GlobalEntryModule(ds:ENTRY, word [MODULE], `seg`).
fn entry_module(code: &mut Code, seg: u16) {
    code.emit(&[0x1E]).push(ENTRY);
    push_word(code, MODULE);
    code.push(seg).call("TOOLHELP", GLOBAL_ENTRY_MODULE);
}

/// GlobalAlloc(`flags`, 100h) into HANDLE, GlobalLock's selector into
/// SEL, GlobalSize into SIZE and the selector's base into BASE.
fn alloc_block(code: &mut Code, flags: u16) {
    code.push(flags)
        .push(0)
        .push(0x100)
        .call("KERNEL", GLOBAL_ALLOC);
    store_ax(code, HANDLE);
    push_word(code, HANDLE);
    code.call("KERNEL", GLOBAL_LOCK);
    store_dx(code, SEL);
    push_word(code, HANDLE);
    code.call("KERNEL", GLOBAL_SIZE);
    store_ax(code, SIZE);
    store_dx(code, SIZE + 2);
    push_word(code, SEL);
    code.call("KERNEL", GET_SELECTOR_BASE);
    store_ax(code, BASE);
    store_dx(code, BASE + 2);
}

/// Walk the list `flags` names with GlobalFirst and GlobalNext: COUNT
/// counts its items, FREE those of GT_FREE, and FOUND is 1 once the
/// block HANDLE comes.
fn walk(code: &mut Code, flags: u16) {
    set(code, COUNT, 0);
    set(code, FREE, 0);
    set(code, FOUND, 0);
    entry_size(code, 36);
    code.emit(&[0x1E])
        .push(ENTRY)
        .push(flags)
        .call("TOOLHELP", GLOBAL_FIRST);
    let top = code.here();
    // test ax,ax; jz near (the end, patched below).
    code.emit(&[0x85, 0xC0, 0x0F, 0x84, 0, 0]);
    let end_at = code.here() - 2;
    code.emit(&[0xFF, 0x06, lo(COUNT), hi(COUNT)]); // inc word [COUNT]
    code.emit(&[0x83, 0x3E, lo(W_TYPE), hi(W_TYPE), GT_FREE as u8]) // cmp word [W_TYPE],7
        .emit(&[0x75, 0x04]) // jne over the next
        .emit(&[0xFF, 0x06, lo(FREE), hi(FREE)]); // inc word [FREE]
    code.emit(&[0xA1, lo(H_BLOCK), hi(H_BLOCK)]) // mov ax,[H_BLOCK]
        .emit(&[0x3B, 0x06, lo(HANDLE), hi(HANDLE)]) // cmp ax,[HANDLE]
        .emit(&[0x75, 0x06]); // jne over the next
    set(code, FOUND, 1);
    code.emit(&[0x1E])
        .push(ENTRY)
        .push(flags)
        .call("TOOLHELP", GLOBAL_NEXT);
    // jmp near top.
    let back = top.wrapping_sub(code.here() + 3);
    code.emit(&[0xE9, lo(back), hi(back)]);
    let end = code.here();
    code.patch(end_at, end - (end_at + 2));
}

fn exit_ok(code: &mut Code) {
    code.emit(&[0xB0, 0x00]).exit_al();
}

/// GlobalEntryHandle fills GLOBALENTRY for a block a program allocated,
/// given its selector or its handle: its address and size as the
/// descriptor and GlobalSize have them, its handle and lock counts,
/// GF_PDB_OWNER with the task (what GetCurrentTask gives) as its owner,
/// and GT_UNKNOWN, as Windows 3.1 does for private data. SmartHeap, in
/// Outpost 1.1, checks its blocks that way. An entry whose dwSize isn't
/// 36 gets FALSE.
#[test]
fn global_entry_handle_describes_a_block() {
    let mut code = Code::new();
    alloc_block(&mut code, GMEM_MOVEABLE);
    code.call("KERNEL", GET_CURRENT_TASK);
    store_ax(&mut code, TASK);

    entry_size(&mut code, 36);
    entry_handle(&mut code, SEL);
    expect_ax(&mut code, 1, 1);
    expect_same(&mut code, DW_ADDRESS, BASE, 2);
    expect_same(&mut code, DW_ADDRESS + 2, BASE + 2, 3);
    expect_same(&mut code, DW_BLOCK_SIZE, SIZE, 4);
    expect_same(&mut code, DW_BLOCK_SIZE + 2, SIZE + 2, 5);
    expect_same(&mut code, H_BLOCK, HANDLE, 6);
    expect(&mut code, WC_LOCK, 1, 7);
    expect(&mut code, WC_PAGE_LOCK, 0, 8);
    expect_byte(&mut code, W_FLAGS + 1, hi(GF_PDB_OWNER), 9);
    expect_same(&mut code, H_OWNER, TASK, 10);
    expect(&mut code, W_TYPE, GT_UNKNOWN, 11);
    expect(&mut code, W_HEAP_PRESENT, 0, 12);

    // By its handle.
    set(&mut code, H_BLOCK, 0);
    entry_handle(&mut code, HANDLE);
    expect_ax(&mut code, 1, 13);
    expect_same(&mut code, H_BLOCK, HANDLE, 14);

    // A dwSize TOOLHELP doesn't know.
    entry_size(&mut code, 35);
    entry_handle(&mut code, HANDLE);
    expect_ax(&mut code, 0, 15);

    // A handle that is no block.
    entry_size(&mut code, 36);
    set(&mut code, FOUND, 0x0FF6);
    entry_handle(&mut code, FOUND);
    expect_ax(&mut code, 0, 16);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_entry_handle", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// GlobalEntryModule finds a module's segments by number: the code
/// segment as GT_CODE and the automatic data segment, which has the
/// local heap, as GT_DGROUP, each with its number in wData and the
/// module as its owner. The module's database is GT_MODULE and the
/// task's GT_TASK.
#[test]
fn global_entry_module_describes_segments() {
    let mut code = Code::new();
    // LocalInit(DS, 200h, 600h), so the data segment has a local heap.
    code.push(0)
        .push(0x200)
        .push(0x600)
        .call("KERNEL", LOCAL_INIT);
    code.emit(&[0x8C, 0x1E, lo(DS), hi(DS)]); // mov [DS],ds
    push_word(&mut code, DS);
    code.call("KERNEL", GET_EXE_PTR);
    store_ax(&mut code, MODULE);
    // End with 1 if there is no module: cmp ax,0; jne over the exit.
    code.emit(&[0x3D, 0, 0, 0x75, 0x06, 0xB0, 1, 0xB4, 0x4C, 0xCD, 0x21]);

    entry_size(&mut code, 36);
    entry_module(&mut code, 1);
    expect_ax(&mut code, 1, 2);
    expect(&mut code, W_TYPE, GT_CODE, 3);
    expect(&mut code, W_DATA, 1, 4);
    expect_same(&mut code, H_OWNER, MODULE, 5);
    expect_byte(&mut code, W_FLAGS + 1, 0, 6);

    entry_module(&mut code, 2);
    expect_ax(&mut code, 1, 7);
    expect(&mut code, W_TYPE, GT_DGROUP, 8);
    expect(&mut code, W_DATA, 2, 9);
    expect(&mut code, W_HEAP_PRESENT, 1, 10);
    expect_same(&mut code, H_BLOCK, DS, 11);
    expect_same(&mut code, H_OWNER, MODULE, 12);

    // A segment the module hasn't.
    entry_module(&mut code, 3);
    expect_ax(&mut code, 0, 13);

    entry_handle(&mut code, MODULE);
    expect_ax(&mut code, 1, 14);
    expect(&mut code, W_TYPE, GT_MODULE, 15);

    code.call("KERNEL", GET_CURRENT_TASK);
    store_ax(&mut code, FOUND);
    entry_handle(&mut code, FOUND);
    expect_ax(&mut code, 1, 16);
    expect(&mut code, W_TYPE, GT_TASK, 17);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_entry_module", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// GlobalFirst and GlobalNext walk the heap: every item for GLOBAL_ALL,
/// as many as GlobalInfo's wcItems, the program's block among them; the
/// discardable blocks for GLOBAL_LRU, as many as wcItemsLRU; and the
/// free pieces, all GT_FREE, for GLOBAL_FREE, as many as wcItemsFree.
#[test]
fn global_first_and_next_walk_the_heap() {
    let mut code = Code::new();
    alloc_block(&mut code, GMEM_MOVEABLE | GMEM_DISCARDABLE);
    // GlobalInfo(ds:INFO).
    code.emit(&[0x1E]).push(INFO).call("TOOLHELP", GLOBAL_INFO);
    expect_ax(&mut code, 1, 1);

    walk(&mut code, GLOBAL_ALL);
    expect_same(&mut code, COUNT, WC_ITEMS, 2);
    expect_same(&mut code, FREE, WC_ITEMS_FREE, 3);
    expect(&mut code, FOUND, 1, 4);

    walk(&mut code, GLOBAL_LRU);
    expect_same(&mut code, COUNT, WC_ITEMS_LRU, 5);
    expect(&mut code, FOUND, 1, 6);
    expect(&mut code, FREE, 0, 7);

    walk(&mut code, GLOBAL_FREE);
    expect_same(&mut code, COUNT, WC_ITEMS_FREE, 8);
    expect_same(&mut code, FREE, WC_ITEMS_FREE, 9);
    expect(&mut code, FOUND, 0, 10);

    // An entry whose dwSize isn't 36.
    entry_size(&mut code, 0);
    code.emit(&[0x1E])
        .push(ENTRY)
        .push(GLOBAL_ALL)
        .call("TOOLHELP", GLOBAL_FIRST);
    expect_ax(&mut code, 0, 11);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_walk", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}
