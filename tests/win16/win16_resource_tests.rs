//! Win16 resources on the built-in Windows: what the 32-bit image of a
//! module's resources (src/win16/resource.rs), which USER's thunked
//! functions read, takes of the machine's memory, and what a commit of
//! pages that fails leaves of it. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use rust_dos_ne as ne;
use win16::{CODE, Code, program, program_with_resources, run_in_memory};

/// KERNEL's and USER's ordinals.
const GLOBAL_ALLOC: u16 = 15;
const LOAD_STRING: u16 = 176;

/// A program's exit code when it got what it asked for.
const OK: u8 = 0x2A;

/// The resource types.
const RT_STRING: u16 = 6;
const RT_RCDATA: u16 = 10;

/// The string table with the strings 0 to 15: string 1 is "Hi".
fn strings() -> Vec<u8> {
    let mut block = vec![0, 2, b'H', b'i'];
    block.resize(18, 0);
    block
}

/// Ask GlobalAlloc for a fixed block of `size` bytes: AX is its handle,
/// 0 if there is none.
fn global_alloc(code: &mut Code, size: u32) -> &mut Code {
    code.push(0) // GMEM_FIXED
        .push((size >> 16) as u16)
        .push(size as u16)
        .call("KERNEL", GLOBAL_ALLOC)
}

/// Exit with `OK` if AX is a block's handle, else with 0.
fn exit_ok_if_block(code: &mut Code) {
    code.emit(&[
        0xF7, 0xD8, // neg ax
        0x19, 0xC0, // sbb ax,ax
        0x24, OK, // and al,OK: OK if there is a block
    ])
    .exit_al();
}

/// A program with `data` bytes of resource data besides a string table,
/// which loads string 1 with LoadString if `load_string`, and then
/// allocates a fixed block of `alloc` bytes. Its exit code: `OK` if
/// LoadString gave "Hi" (or wasn't called) and the block was given, else
/// 0 (2 if LoadString failed).
fn loader(data: usize, load_string: bool, alloc: u32) -> Vec<u8> {
    let mut code = Code::new();
    if load_string {
        code.emit(&[0x57]) // push di: the instance
            .push(1)
            .emit(&[0x1E]) // push ds
            .push(0x10)
            .push(0x10)
            .call("USER", LOAD_STRING)
            .emit(&[
                0x83, 0xF8, 0x02, // cmp ax,2
                0x75, 0x08, // jne fail
                0x81, 0x3E, 0x10, 0x00, b'H', b'i', // cmp word [10h],"Hi"
            ])
            .emit(&[0x74, 0x06]) // je ok
            .emit(&[0xB0, 0x02]) // fail: mov al,2
            .exit_al();
    }
    global_alloc(&mut code, alloc); // ok:
    exit_ok_if_block(&mut code);
    let raw = vec![0x5A; data];
    let strings = strings();
    program_with_resources(
        &code,
        CODE,
        &[
            (ne::ResId::Int(RT_STRING), ne::ResId::Int(1), &strings),
            (ne::ResId::Int(RT_RCDATA), ne::ResId::Int(1), &raw),
        ],
    )
}

/// RAM of the machines the programs run on, in MB: little enough that
/// the resource data below is a large share of what is free.
const MEMORY_MB: usize = 6;
/// The resource data: nearly 1 MB, the most one resource of the test's
/// NE files can have.
const DATA: usize = 0xF_0000;
/// The steps the largest block is found in.
const STEP: u32 = 0x1_0000;
/// The largest block GlobalAlloc gives: 16 MB less 64 KB.
const MAX_ALLOC: u32 = 0xFF_0000;

/// What is free for a fixed block on the machine, in 64 KB steps: the
/// largest block a program without the large resource gets. Committed
/// memory includes the page file's, which is twice the machine's memory,
/// so the search goes up to the largest block GlobalAlloc gives, and the
/// machine has to be small enough that its memory runs out first. A
/// binary search, as a block that fits has every smaller one fit. The
/// runs go in the scratch directory `test`, one of the calling test's own.
fn largest_block(test: &str) -> u32 {
    let fits = |steps: u32| {
        let program = loader(0, false, steps * STEP);
        run_in_memory(test, program, MEMORY_MB).0 == OK
    };
    // `fits(low)` holds, unless low is 0; `fits(high)` doesn't.
    let (mut low, mut high) = (0, MAX_ALLOC / STEP + 1);
    while high - low > 1 {
        let middle = (low + high) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    assert!(low > 0, "no block can be allocated");
    assert!(
        low * STEP < MAX_ALLOC,
        "the machine's memory doesn't run out before GlobalAlloc's limit"
    );
    low * STEP
}

/// A string USER's LoadString reads from the module's 32-bit image of
/// resources leaves the module's other resources out of memory: as on
/// Windows, where a resource is read from the file when it is loaded,
/// they take no memory until something asks for them. The image held all
/// of them before, which is how Metal Marines' 28 MB sound DLL took the
/// memory of a 32 MB machine (issue 631).
#[test]
fn a_resource_image_takes_no_memory_for_data_nothing_asked_for() {
    let free = largest_block("resource_image_largest");
    assert!(
        free > DATA as u32,
        "the test needs more free memory than the resource's size"
    );
    let alloc = free - STEP;
    let (code, log) = run_in_memory(
        "resource_image_memory",
        loader(DATA, true, alloc),
        MEMORY_MB,
    );
    assert_eq!(
        code, OK,
        "LoadString and a block of {alloc:#X} bytes after it:\n{log}"
    );
}

/// A GlobalAlloc larger than what is free fails and leaves what is free
/// as it was: the heap's commit of the block's pages gives back those it
/// took before it ran out. It kept them before, so a block that fitted
/// before the failed one no longer did.
#[test]
fn a_global_alloc_that_fails_takes_no_memory() {
    let free = largest_block("resource_failed_alloc_largest");
    // One step past the largest block that fits: it fails for want of
    // memory, as it is within GlobalAlloc's limit.
    let (huge, alloc) = (free + STEP, free - STEP);
    let mut code = Code::new();
    global_alloc(&mut code, huge).emit(&[
        0x85, 0xC0, // test ax,ax
        0x74, 0x06, // jz next
        0xB0, 0x03, // mov al,3: the huge block was given
    ]);
    code.exit_al();
    global_alloc(&mut code, alloc); // next:
    exit_ok_if_block(&mut code);
    let exe = program(&code, CODE);
    let (code, log) = run_in_memory("resource_failed_alloc", exe, MEMORY_MB);
    assert_eq!(
        code, OK,
        "a block of {alloc:#X} bytes after one of {huge:#X} failed:\n{log}"
    );
}
