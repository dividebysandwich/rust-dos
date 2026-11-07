//! Win16 resources on the built-in Windows: what the 32-bit image of a
//! module's resources (src/win16/resource.rs), which USER's thunked
//! functions read, takes of the machine's memory. The programs are made
//! byte by byte (tests/common/win16).

use crate::win16;

use rust_dos_ne as ne;
use win16::{CODE, Code, program_with_resources, run_in_memory};

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
    block.resize(17, 0);
    block
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
    code.push(0) // ok: GMEM_FIXED
        .push((alloc >> 16) as u16)
        .push(alloc as u16)
        .call("KERNEL", GLOBAL_ALLOC)
        .emit(&[
            0xF7, 0xD8, // neg ax
            0x19, 0xC0, // sbb ax,ax
            0x24, OK, // and al,OK: OK if there is a block
        ])
        .exit_al();
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

/// What is free for a fixed block on the machine, in 64 KB steps: the
/// largest block a program without the large resource gets.
fn largest_block() -> u32 {
    let mut size = 0x40_0000;
    while size > 0 {
        let (code, _) = run_in_memory("resource_largest", loader(0, false, size), MEMORY_MB);
        if code == OK {
            return size;
        }
        size -= 0x1_0000;
    }
    panic!("no block can be allocated");
}

/// A string USER's LoadString reads from the module's 32-bit image of
/// resources leaves the module's other resources out of memory: as on
/// Windows, where a resource is read from the file when it is loaded,
/// they take no memory until something asks for them. The image held all
/// of them before, which is how Metal Marines' 28 MB sound DLL took the
/// memory of a 32 MB machine (issue 631).
#[test]
fn a_resource_image_takes_no_memory_for_data_nothing_asked_for() {
    let free = largest_block();
    assert!(
        free > DATA as u32,
        "the test needs more free memory than the resource's size"
    );
    let alloc = free - 0x1_0000;
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
