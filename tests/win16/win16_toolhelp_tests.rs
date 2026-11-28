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
const GET_CURRENT_PDB: u16 = 37;
const FAR_SET_OWNER: u16 = 403;
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
const GMEM_DDESHARE: u16 = 0x2000;

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
/// Outpost 1.1, checks its blocks that way. A block FarSetOwner gave to
/// the task's PDB is the task's too. An entry whose dwSize isn't 36 gets
/// FALSE.
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

    // FarSetOwner(handle, GetCurrentPDB()): a block the task's PDB owns
    // is the task's too.
    push_word(&mut code, HANDLE);
    code.call("KERNEL", GET_CURRENT_PDB);
    code.emit(&[0x50]).call("KERNEL", FAR_SET_OWNER); // push ax
    set(&mut code, H_OWNER, 0);
    entry_handle(&mut code, HANDLE);
    expect_ax(&mut code, 1, 17);
    expect_byte(&mut code, W_FLAGS + 1, hi(GF_PDB_OWNER), 18);
    expect_same(&mut code, H_OWNER, TASK, 19);
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

/// A GMEM_DDESHARE block belongs to the module whose code allocated it,
/// as Windows 3.1's GlobalAlloc gives it, so it stays when the task
/// running then ends: GlobalEntryHandle has the program's module as its
/// owner, without GF_PDB_OWNER. A block without the flag is the task's.
#[test]
fn a_shared_block_belongs_to_the_calling_module() {
    let mut code = Code::new();
    code.emit(&[0x8C, 0x1E, lo(DS), hi(DS)]); // mov [DS],ds
    push_word(&mut code, DS);
    code.call("KERNEL", GET_EXE_PTR);
    store_ax(&mut code, MODULE);
    code.call("KERNEL", GET_CURRENT_TASK);
    store_ax(&mut code, TASK);

    alloc_block(&mut code, GMEM_MOVEABLE | GMEM_DDESHARE);
    entry_size(&mut code, 36);
    entry_handle(&mut code, HANDLE);
    expect_ax(&mut code, 1, 1);
    expect_same(&mut code, H_OWNER, MODULE, 2);
    expect_byte(&mut code, W_FLAGS + 1, 0, 3);

    alloc_block(&mut code, GMEM_MOVEABLE);
    entry_handle(&mut code, HANDLE);
    expect_ax(&mut code, 1, 4);
    expect_same(&mut code, H_OWNER, TASK, 5);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_shared_block", program(&code, CODE));
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

/// TOOLHELP's task, module and local heap functions.
const LOCAL_INFO: u16 = 56;
const LOCAL_FIRST: u16 = 57;
const LOCAL_NEXT: u16 = 58;
const MODULE_FIRST: u16 = 59;
const MODULE_NEXT: u16 = 60;
const MODULE_FIND_NAME: u16 = 61;
const MODULE_FIND_HANDLE: u16 = 62;
const TASK_FIRST: u16 = 63;
const TASK_NEXT: u16 = 64;
const TASK_FIND_HANDLE: u16 = 65;
/// KERNEL's LocalAlloc and GetModuleHandle.
const LOCAL_ALLOC: u16 = 5;
const GET_MODULE_HANDLE: u16 = 47;

/// TASKENTRY, MODULEENTRY, LOCALINFO and LOCALENTRY sizes.
const TASKENTRY: u16 = 0x28;
const MODULEENTRY: u16 = 0x114;
const LOCALINFO: u16 = 6;
const LOCALENTRY: u16 = 0x16;

/// More places in the data segment: the instance DI had at the start,
/// SS, a module name, what a walk copies from the entry it looks for,
/// the value it looks for, and a second handle.
const INST: u16 = 0x28;
const SS: u16 = 0x2A;
const NAME: u16 = 0x2C;
const GOT_FLAGS: u16 = 0x36;
const GOT_SIZE: u16 = 0x38;
const WANT: u16 = 0x3A;
const HANDLE2: u16 = 0x3C;

/// TASKENTRY's fields at ENTRY.
const TE_TASK: u16 = ENTRY + 4;
const TE_INST: u16 = ENTRY + 8;
const TE_MODULE: u16 = ENTRY + 10;
const TE_SS: u16 = ENTRY + 12;
const TE_MODULE_NAME: u16 = ENTRY + 0x1A;
const TE_PSP_OFFSET: u16 = ENTRY + 0x24;

/// MODULEENTRY's fields at ENTRY.
const ME_NAME: u16 = ENTRY + 4;
const ME_MODULE: u16 = ENTRY + 0x0E;
const ME_USAGE: u16 = ENTRY + 0x10;
const ME_PATH: u16 = ENTRY + 0x12;

/// LOCALENTRY's fields at ENTRY, and its wFlags and wType values.
const LE_HANDLE: u16 = ENTRY + 4;
const LE_SIZE: u16 = ENTRY + 8;
const LE_FLAGS: u16 = ENTRY + 10;
const LE_TYPE: u16 = ENTRY + 14;
const LE_HEAP: u16 = ENTRY + 16;
const LF_FIXED: u16 = 1;
const LF_FREE: u16 = 2;
const LF_MOVEABLE: u16 = 4;
const LT_FREE: u16 = 0xFF;

/// End with `fail` if the word at `at` is 0.
fn expect_nonzero(code: &mut Code, at: u16, fail: u8) {
    code.emit(&[0x83, 0x3E, lo(at), hi(at), 0])
        .emit(&[0x75, 0x06]) // jne over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// Walk a list: `first` calls the function that gives its first item,
/// then `next` (TOOLHELP's, given ds:ENTRY) the rest, to the first
/// FALSE. COUNT counts the items; FOUND is 1 once one whose word `key`
/// equals the word at WANT comes, and GOT_FLAGS and GOT_SIZE then have
/// its words `flags` and `size`.
fn walk_list(
    code: &mut Code,
    first: impl Fn(&mut Code),
    next: u16,
    key: u16,
    flags: u16,
    size: u16,
) {
    set(code, COUNT, 0);
    set(code, FOUND, 0);
    first(code);
    let top = code.here();
    // test ax,ax; jz near (the end, patched below).
    code.emit(&[0x85, 0xC0, 0x0F, 0x84, 0, 0]);
    let end_at = code.here() - 2;
    code.emit(&[0xFF, 0x06, lo(COUNT), hi(COUNT)]); // inc word [COUNT]
    code.emit(&[0xA1, lo(key), hi(key)]) // mov ax,[key]
        .emit(&[0x3B, 0x06, lo(WANT), hi(WANT)]) // cmp ax,[WANT]
        .emit(&[0x75, 0x12]); // jne over the next 18 bytes
    set(code, FOUND, 1);
    code.emit(&[0xA1, lo(flags), hi(flags)]);
    store_ax(code, GOT_FLAGS);
    code.emit(&[0xA1, lo(size), hi(size)]);
    store_ax(code, GOT_SIZE);
    code.emit(&[0x1E]).push(ENTRY).call("TOOLHELP", next);
    // jmp near top.
    let back = top.wrapping_sub(code.here() + 3);
    code.emit(&[0xE9, lo(back), hi(back)]);
    let end = code.here();
    code.patch(end_at, end - (end_at + 2));
}

/// TaskFindHandle fills TASKENTRY for the running task: its handle (what
/// GetCurrentTask gives), its instance (what DI had at the start), its
/// module, its stack's selector, its module's name and the PDB's offset
/// 100h. Visual Basic 4's runtime reads the instance from it at start
/// and quits when the call fails. TaskFirst and TaskNext walk the tasks
/// to this one; an entry whose dwSize isn't 28h, and a handle that is no
/// task, get FALSE.
#[test]
fn task_find_handle_describes_the_running_task() {
    let mut code = Code::new();
    code.emit(&[0x89, 0x3E, lo(INST), hi(INST)]); // mov [INST],di
    code.emit(&[0x8C, 0x16, lo(SS), hi(SS)]); // mov [SS],ss
    code.call("KERNEL", GET_CURRENT_TASK);
    store_ax(&mut code, TASK);
    code.emit(&[0x1E]).call("KERNEL", GET_EXE_PTR); // push ds
    store_ax(&mut code, MODULE);

    entry_size(&mut code, TASKENTRY);
    code.emit(&[0x1E]).push(ENTRY);
    push_word(&mut code, TASK);
    code.call("TOOLHELP", TASK_FIND_HANDLE);
    expect_ax(&mut code, 1, 1);
    expect_same(&mut code, TE_TASK, TASK, 2);
    expect_same(&mut code, TE_INST, INST, 3);
    expect_same(&mut code, TE_MODULE, MODULE, 4);
    expect_same(&mut code, TE_SS, SS, 5);
    expect_byte(&mut code, TE_MODULE_NAME, b'T', 6);
    expect_byte(&mut code, TE_MODULE_NAME + 1, 0, 7);
    expect(&mut code, TE_PSP_OFFSET, 0x100, 8);

    // TaskFirst and TaskNext come to this task.
    code.emit(&[0xA1, lo(TASK), hi(TASK)]); // mov ax,[TASK]
    store_ax(&mut code, WANT);
    walk_list(
        &mut code,
        |c| {
            c.emit(&[0x1E]).push(ENTRY).call("TOOLHELP", TASK_FIRST);
        },
        TASK_NEXT,
        TE_TASK,
        TE_INST,
        TE_MODULE,
    );
    expect(&mut code, FOUND, 1, 9);
    expect_same(&mut code, GOT_FLAGS, INST, 10);
    expect_same(&mut code, GOT_SIZE, MODULE, 11);

    // A handle that is no task.
    code.emit(&[0x1E]).push(ENTRY).push(0x0FF6);
    code.call("TOOLHELP", TASK_FIND_HANDLE);
    expect_ax(&mut code, 0, 12);

    // A dwSize TOOLHELP doesn't know.
    entry_size(&mut code, TASKENTRY - 1);
    code.emit(&[0x1E]).push(ENTRY);
    push_word(&mut code, TASK);
    code.call("TOOLHELP", TASK_FIND_HANDLE);
    expect_ax(&mut code, 0, 13);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_tasks", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// ModuleFindHandle fills MODULEENTRY for the program's module, given
/// its data segment as GetExePtr takes it: the module's handle, its
/// name, its use count and its file's path. ModuleFindName finds KERNEL
/// by its name in any case, the handle GetModuleHandle gives, and
/// ModuleFirst and ModuleNext walk the modules from KERNEL to the
/// program's. A dwSize that isn't 114h gets FALSE.
#[test]
fn module_walk_finds_the_programs_module() {
    let mut code = Code::new();
    code.emit(&[0x1E]).call("KERNEL", GET_EXE_PTR); // push ds
    store_ax(&mut code, MODULE);
    entry_size(&mut code, MODULEENTRY);

    code.emit(&[0x1E]).push(ENTRY).emit(&[0x1E]); // push ds (the module)
    code.call("TOOLHELP", MODULE_FIND_HANDLE);
    expect_ax(&mut code, 1, 1);
    expect_same(&mut code, ME_MODULE, MODULE, 2);
    expect_byte(&mut code, ME_NAME, b'T', 3);
    expect_byte(&mut code, ME_NAME + 1, 0, 4);
    expect_nonzero(&mut code, ME_USAGE, 5);
    expect_byte(&mut code, ME_PATH + 1, b':', 6);

    // "kernel", in lower case.
    set(&mut code, NAME, u16::from_le_bytes(*b"ke"));
    set(&mut code, NAME + 2, u16::from_le_bytes(*b"rn"));
    set(&mut code, NAME + 4, u16::from_le_bytes(*b"el"));
    set(&mut code, NAME + 6, 0);
    code.emit(&[0x1E]).push(ENTRY).emit(&[0x1E]).push(NAME);
    code.call("TOOLHELP", MODULE_FIND_NAME);
    expect_ax(&mut code, 1, 7);
    expect_byte(&mut code, ME_NAME, b'K', 8);
    code.emit(&[0x1E])
        .push(NAME)
        .call("KERNEL", GET_MODULE_HANDLE);
    code.emit(&[0x3B, 0x06, lo(ME_MODULE), hi(ME_MODULE)]); // cmp ax,[ME_MODULE]
    or_fail(&mut code, 9);

    // ModuleFirst gives KERNEL, and the walk comes to the program.
    code.emit(&[0x1E])
        .push(ENTRY)
        .call("TOOLHELP", MODULE_FIRST);
    expect_ax(&mut code, 1, 10);
    expect_byte(&mut code, ME_NAME, b'K', 11);
    code.emit(&[0xA1, lo(MODULE), hi(MODULE)]); // mov ax,[MODULE]
    store_ax(&mut code, WANT);
    walk_list(
        &mut code,
        |c| {
            c.emit(&[0x1E]).push(ENTRY).call("TOOLHELP", MODULE_FIRST);
        },
        MODULE_NEXT,
        ME_MODULE,
        ME_NAME,
        ME_USAGE,
    );
    expect(&mut code, FOUND, 1, 12);
    expect(&mut code, GOT_FLAGS, u16::from(b'T'), 13);

    // A dwSize TOOLHELP doesn't know.
    entry_size(&mut code, MODULEENTRY + 1);
    code.emit(&[0x1E])
        .push(ENTRY)
        .call("TOOLHELP", MODULE_FIRST);
    expect_ax(&mut code, 0, 14);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_modules", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// LocalInfo counts the items of a local heap, and LocalFirst and
/// LocalNext walk as many: a fixed block as LF_FIXED and a moveable one
/// as LF_MOVEABLE, each by its handle and with its size, and the free
/// rest of the heap as LF_FREE of type LT_FREE, every entry naming the
/// heap. Wrong dwSizes get FALSE.
#[test]
fn local_walk_lists_the_heaps_blocks() {
    let mut code = Code::new();
    // LocalInit(DS, 200h, 600h).
    code.push(0)
        .push(0x200)
        .push(0x600)
        .call("KERNEL", LOCAL_INIT);
    code.emit(&[0x8C, 0x1E, lo(DS), hi(DS)]); // mov [DS],ds
    code.push(0).push(0x20).call("KERNEL", LOCAL_ALLOC);
    store_ax(&mut code, HANDLE);
    code.push(0x0002).push(0x30).call("KERNEL", LOCAL_ALLOC);
    store_ax(&mut code, HANDLE2);
    expect_nonzero(&mut code, HANDLE, 1);
    expect_nonzero(&mut code, HANDLE2, 2);

    set(&mut code, INFO, LOCALINFO);
    set(&mut code, INFO + 2, 0);
    code.emit(&[0x1E]).push(INFO);
    push_word(&mut code, DS);
    code.call("TOOLHELP", LOCAL_INFO);
    expect_ax(&mut code, 1, 3);

    let first = |c: &mut Code| {
        set(c, ENTRY, LOCALENTRY);
        set(c, ENTRY + 2, 0);
        c.emit(&[0x1E]).push(ENTRY);
        push_word(c, DS);
        c.call("TOOLHELP", LOCAL_FIRST);
    };
    code.emit(&[0xA1, lo(HANDLE), hi(HANDLE)]); // mov ax,[HANDLE]
    store_ax(&mut code, WANT);
    walk_list(&mut code, first, LOCAL_NEXT, LE_HANDLE, LE_FLAGS, LE_SIZE);
    expect_same(&mut code, COUNT, WC_ITEMS, 4);
    expect(&mut code, FOUND, 1, 5);
    expect(&mut code, GOT_FLAGS, LF_FIXED, 6);
    expect(&mut code, GOT_SIZE, 0x20, 7);
    expect_same(&mut code, LE_HEAP, DS, 8);

    code.emit(&[0xA1, lo(HANDLE2), hi(HANDLE2)]); // mov ax,[HANDLE2]
    store_ax(&mut code, WANT);
    walk_list(&mut code, first, LOCAL_NEXT, LE_HANDLE, LE_FLAGS, LE_SIZE);
    expect(&mut code, FOUND, 1, 9);
    expect(&mut code, GOT_FLAGS, LF_MOVEABLE, 10);
    expect(&mut code, GOT_SIZE, 0x30, 11);

    set(&mut code, WANT, LT_FREE);
    walk_list(&mut code, first, LOCAL_NEXT, LE_TYPE, LE_FLAGS, LE_SIZE);
    expect(&mut code, FOUND, 1, 12);
    expect(&mut code, GOT_FLAGS, LF_FREE, 13);

    // Sizes TOOLHELP doesn't know.
    set(&mut code, INFO, LOCALINFO + 1);
    code.emit(&[0x1E]).push(INFO);
    push_word(&mut code, DS);
    code.call("TOOLHELP", LOCAL_INFO);
    expect_ax(&mut code, 0, 14);
    set(&mut code, ENTRY, LOCALENTRY + 1);
    code.emit(&[0x1E]).push(ENTRY);
    push_word(&mut code, DS);
    code.call("TOOLHELP", LOCAL_FIRST);
    expect_ax(&mut code, 0, 15);
    exit_ok(&mut code);

    let (status, log) = run("toolhelp_local_walk", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// KERNEL's and TOOLHELP's ordinals the notification test calls.
const YIELD: u16 = 29;
const MAKE_PROC_INSTANCE: u16 = 51;
const WIN_EXEC: u16 = 166;
const NOTIFY_REGISTER: u16 = 73;
const NOTIFY_UNREGISTER: u16 = 74;

/// Where the notification test keeps what it learns: the events seen so
/// far, the child's instance and module, the result the callback sets
/// (FFh until it does), the callback's MakeProcInstance address, and a
/// TASKENTRY.
const SEEN: u16 = 0x10;
const CHILD_INSTANCE: u16 = 0x12;
const RESULT: u16 = 0x14;
const CHILD_MODULE: u16 = 0x16;
const PROC: u16 = 0x18;
const TASK_ENTRY: u16 = 0x40;

/// Emit the jump `op` (a short one) to a place given later: its
/// operand's place, for `land`.
fn jump(code: &mut Code, op: u8) -> usize {
    code.emit(&[op, 0]);
    code.bytes.len() - 1
}

/// The short jump whose operand is at `at` goes here.
fn land(code: &mut Code, at: usize) {
    let distance = code.bytes.len() - (at + 1);
    assert!(distance < 0x80, "a short jump of {distance} bytes");
    code.bytes[at] = distance as u8;
}

/// `mov byte [RESULT],value` and a short jump to a place given later.
fn result_and_jump(code: &mut Code, value: u8) -> usize {
    code.emit(&[0xC6, 0x06, lo(RESULT), hi(RESULT), value]);
    jump(code, 0xEB)
}

#[test]
fn notify_register_tells_a_task_end_in_the_ending_task() {
    // Sierra's setup (Outpost 1.5) starts a copy of itself and waits for
    // it with a TOOLHELP callback: on NFY_EXITTASK (5) it compares the
    // instance TaskFindHandle gives for GetCurrentTask with the one
    // WinExec gave. Windows 95 calls the callbacks as the task ends, in
    // its context, with NFY_EXITTASK (the exit code in dwData) and then
    // NFY_DELMODULE (6, the module's handle) for its module, as a probe
    // program run on it showed.
    //
    // The program registers a callback (through MakeProcInstance, which
    // gives it its data segment in AX) and starts CHILD, which yields
    // and ends with 37h. The callback checks the events, their order and
    // data, and the current task, and sets RESULT; the program waits
    // for it and ends with it.
    let mut code = Code::new();
    set(&mut code, RESULT, 0xFF);
    set(&mut code, SEEN, 0);
    set(&mut code, TASK_ENTRY, 0x28);
    set(&mut code, TASK_ENTRY + 2, 0);
    let callback_at = code.here() + 2;
    code.push_far(0).push(0).call("KERNEL", MAKE_PROC_INSTANCE);
    code.emit(&[0xA3, lo(PROC), hi(PROC)]) // mov [PROC],ax
        .emit(&[0x89, 0x16, lo(PROC + 2), hi(PROC + 2)]); // mov [PROC+2],dx
    code.push(0)
        .emit(&[0x52, 0x50]) // push dx; push ax
        .push(0)
        .call("TOOLHELP", NOTIFY_REGISTER);
    expect_ax(&mut code, 1, 7);
    // A second registration of the same callback for the task is refused
    // (FALSE on Windows 95), and the first stays.
    code.push(0)
        .emit(&[0xFF, 0x36, lo(PROC + 2), hi(PROC + 2)]) // push word [PROC+2]
        .emit(&[0xFF, 0x36, lo(PROC), hi(PROC)]) // push word [PROC]
        .push(0)
        .call("TOOLHELP", NOTIFY_REGISTER);
    expect_ax(&mut code, 0, 8);
    let path_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    store_ax(&mut code, CHILD_INSTANCE);
    let name_at = code.here() + 2;
    code.push_far(0).call("KERNEL", GET_MODULE_HANDLE);
    store_ax(&mut code, CHILD_MODULE);
    // Wait for RESULT, a hundred turns at most.
    code.emit(&[0xBE, 100, 0]); // mov si,100
    let wait = code.here();
    code.emit(&[0x80, 0x3E, lo(RESULT), hi(RESULT), 0xFF]); // cmp byte [RESULT],FFh
    let done = jump(&mut code, 0x75); // jne done
    code.call("KERNEL", YIELD).emit(&[0x4E]); // dec si
    let back = wait as i32 - (code.here() as i32 + 2);
    code.emit(&[0x75, back as i8 as u8]); // jnz wait
    code.emit(&[0xB0, 9]).exit_al();
    land(&mut code, done);
    // The callback goes with NotifyUnregister, once.
    code.push(0).call("TOOLHELP", NOTIFY_UNREGISTER);
    expect_ax(&mut code, 1, 5);
    code.push(0).call("TOOLHELP", NOTIFY_UNREGISTER);
    expect_ax(&mut code, 0, 6);
    code.emit(&[0xA0, lo(RESULT), hi(RESULT)]).exit_al(); // mov al,[RESULT]

    // BOOL CALLBACK f(WORD wID, DWORD dwData): wID at [bp+0Ah], dwData
    // at [bp+6].
    let callback = code.here();
    code.emit(&[0x55, 0x8B, 0xEC, 0x1E, 0x8E, 0xD8]); // push bp; mov bp,sp; push ds; mov ds,ax
    code.emit(&[0x8B, 0x46, 0x0A, 0x3D, 0x05, 0x00]); // mov ax,[bp+0Ah]; cmp ax,5
    let not_exit = jump(&mut code, 0x75);
    // NFY_EXITTASK: the exit code, and nothing seen before.
    code.emit(&[0x83, 0x7E, 0x06, 0x37]); // cmp word [bp+6],37h
    let fail1 = jump(&mut code, 0x75);
    code.emit(&[0x83, 0x3E, lo(SEEN), hi(SEEN), 0x00]); // cmp word [SEEN],0
    let fail2 = jump(&mut code, 0x75);
    set(&mut code, SEEN, 1);
    let ret1 = jump(&mut code, 0xEB);
    land(&mut code, not_exit);
    code.emit(&[0x3D, 0x06, 0x00]); // cmp ax,6
    let ret2 = jump(&mut code, 0x75);
    // NFY_DELMODULE: after NFY_EXITTASK, CHILD's module, and CHILD the
    // current task.
    code.emit(&[0x83, 0x3E, lo(SEEN), hi(SEEN), 0x01]); // cmp word [SEEN],1
    let fail2b = jump(&mut code, 0x75);
    code.emit(&[0x8B, 0x46, 0x06]) // mov ax,[bp+6]
        .emit(&[0x3B, 0x06, lo(CHILD_MODULE), hi(CHILD_MODULE)]); // cmp ax,[CHILD_MODULE]
    let fail4 = jump(&mut code, 0x75);
    code.call("KERNEL", GET_CURRENT_TASK);
    code.emit(&[0x1E]).push(TASK_ENTRY).emit(&[0x50]); // push ds; push TASK_ENTRY; push ax
    code.call("TOOLHELP", TASK_FIND_HANDLE);
    code.emit(&[0xA1, lo(TASK_ENTRY + 8), hi(TASK_ENTRY + 8)]) // mov ax,[hInst]
        .emit(&[0x3B, 0x06, lo(CHILD_INSTANCE), hi(CHILD_INSTANCE)]); // cmp ax,[CHILD_INSTANCE]
    let fail3 = jump(&mut code, 0x75);
    let ret3 = result_and_jump(&mut code, 0);
    land(&mut code, fail1);
    let ret4 = result_and_jump(&mut code, 1);
    land(&mut code, fail2);
    land(&mut code, fail2b);
    let ret5 = result_and_jump(&mut code, 2);
    land(&mut code, fail3);
    let ret6 = result_and_jump(&mut code, 3);
    land(&mut code, fail4);
    code.emit(&[0xC6, 0x06, lo(RESULT), hi(RESULT), 4]);
    for at in [ret1, ret2, ret3, ret4, ret5, ret6] {
        land(&mut code, at);
    }
    code.emit(&[0x31, 0xC0, 0x1F, 0x5D, 0xCA, 0x06, 0x00]); // xor ax,ax; pop ds; pop bp; retf 6

    let path = code.here();
    code.emit(b"C:\\CHILD.EXE\0");
    let name = code.here();
    code.emit(b"CHILD\0");
    code.patch(callback_at, callback);
    code.patch(path_at, path);
    code.patch(name_at, name);

    let mut child = Code::new();
    child.emit(&[0xBE, 0x03, 0x00]); // mov si,3
    let again = child.here();
    child.call("KERNEL", YIELD).emit(&[0x4E]); // dec si
    let back = again as i32 - (child.here() as i32 + 2);
    child.emit(&[0x75, back as i8 as u8, 0xB0, 0x37]); // jnz again; mov al,37h
    child.exit_al();
    let child = win16::program_named("CHILD", &child, CODE);

    let (status, log) = win16::run_with("toolhelp_notify", program(&code, CODE), |_, base| {
        std::fs::write(base.join("CHILD.EXE"), child).unwrap();
    });
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}
