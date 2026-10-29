//! The built-in Windows's Win16 loader and memory: Windows 3.1 programs
//! with segments flagged 32-bit, and the selectors such programs make
//! 32-bit; and what Windows does for their CLI, STI and INT n. The
//! programs are made byte by byte (tests/win16).

mod win16;

use rust_dos_ne as ne;
use win16::{CODE, Code, program, run};

/// KERNEL's ordinals.
const GLOBAL_ALLOC: u16 = 15;
const GLOBAL_REALLOC: u16 = 16;
const ALLOC_DS_TO_CS_ALIAS: u16 = 171;
const ZERO_SELECTOR: u16 = 183;
const GLOBAL_DOS_ALLOC: u16 = 184;

/// `mov bx,cs` and `mov bx,si`.
const BX_CS: [u8; 2] = [0x8C, 0xCB];
const BX_SI: [u8; 2] = [0x8B, 0xDE];

/// The descriptor of the selector `mov_bx` loads into BX, at ES:DI on the
/// stack, with DPMI 000Bh.
fn get_descriptor(code: &mut Code, mov_bx: [u8; 2]) {
    code.emit(&[
        0x8C, 0xD0, // mov ax,ss
        0x8E, 0xC0, // mov es,ax
        0x83, 0xEC, 0x08, // sub sp,8
        0x8B, 0xFC, // mov di,sp
    ])
    .emit(&mov_bx)
    .emit(&[
        0xB8, 0x0B, 0x00, // mov ax,000Bh: get the descriptor
        0xCD, 0x31, // int 31h
    ]);
}

/// After `get_descriptor`: set the D bit and write the descriptor back
/// with DPMI 000Ch, the stack as it was.
fn set_d_bit(code: &mut Code) {
    code.emit(&[
        0x26, 0x80, 0x4D, 0x06, 0x40, // or byte es:[di+6],40h: the D bit
        0xB8, 0x0C, 0x00, // mov ax,000Ch: set the descriptor
        0xCD, 0x31, // int 31h
        0x83, 0xC4, 0x08, // add sp,8
    ]);
}

/// After `get_descriptor`: end with the D bit (40h) as the exit code.
fn exit_with_d_bit(code: &mut Code) {
    code.emit(&[
        0x26, 0x8A, 0x45, 0x06, // mov al,es:[di+6]
        0x24, 0x40, // and al,40h
    ])
    .exit_al();
}

/// A fixed data block of 100h bytes, its selector in SI.
fn alloc_block(code: &mut Code) {
    code.push(0) // GMEM_FIXED
        .push(0)
        .push(0x100)
        .call("KERNEL", GLOBAL_ALLOC)
        .emit(&[0x8B, 0xF0]); // mov si,ax
}

#[test]
fn a_segment_flagged_32_bit_loads_as_16_bit_and_can_make_itself_32_bit() {
    // Makes its own selector 32-bit, as Metal Marines' 32-bit segment
    // does, then ends with 20h if it runs as 32-bit code, or 10h if it
    // still runs as 16-bit. The IRET that ends the INT 31h loads CS
    // again, so the code after it is 32-bit at once; the game checks that
    // the same way, with the carry of ADD AX,AX.
    let mut code = Code::new();
    get_descriptor(&mut code, BX_CS);
    set_d_bit(&mut code);
    code.emit(&[
        0x33, 0xC0, // xor (e)ax,(e)ax
        0xB4, 0x80, // mov ah,80h
        0x03, 0xC0, // add (e)ax,(e)ax: a carry only in 16-bit code
        0xB0, 0x10, // mov al,10h
        0x72, 0x02, // jc over the next
        0xB0, 0x20, // mov al,20h
    ])
    .exit_al();
    let (exit, log) = run("seg32", program(&code, CODE | ne::SEG_32BIT));
    assert_eq!(exit, 0x20, "{log}");
}

#[test]
fn a_block_keeps_the_d_bit_its_program_set_when_it_moves() {
    // A block made 32-bit, then grown to 2000h bytes and let move: the
    // new selector's D bit is the exit code.
    let mut code = Code::new();
    alloc_block(&mut code);
    get_descriptor(&mut code, BX_SI);
    set_d_bit(&mut code);
    code.emit(&[0x56]) // push si
        .push(0)
        .push(0x2000)
        .push(2) // GMEM_MOVEABLE
        .call("KERNEL", GLOBAL_REALLOC)
        .emit(&[0x8B, 0xF0]); // mov si,ax
    get_descriptor(&mut code, BX_SI);
    exit_with_d_bit(&mut code);
    let (exit, log) = run("realloc_big", program(&code, CODE));
    assert_eq!(exit, 0x40, "{log}");
}

#[test]
fn an_alias_of_a_32_bit_selector_is_32_bit() {
    // A data block made 32-bit, then a code alias of it: the alias's D
    // bit is the exit code.
    let mut code = Code::new();
    alloc_block(&mut code);
    get_descriptor(&mut code, BX_SI);
    set_d_bit(&mut code);
    code.emit(&[0x56]) // push si
        .call("KERNEL", ALLOC_DS_TO_CS_ALIAS)
        .emit(&[0x8B, 0xF0]); // mov si,ax
    get_descriptor(&mut code, BX_SI);
    exit_with_d_bit(&mut code);
    let (exit, log) = run("alias_big", program(&code, CODE));
    assert_eq!(exit, 0x40, "{log}");
}

#[test]
fn a_32_bit_offset_is_relocated_whole() {
    // A dword relocated to offset 1234h of the code segment, read back
    // as two words: exit code 1 if it is 00001234h.
    let mut code = Code::new();
    let read = code.here();
    code.emit(&[
        0x2E, 0xA1, 0x00, 0x00, // mov ax,cs:[dword]
        0x2E, 0x8B, 0x16, 0x00, 0x00, // mov dx,cs:[dword+2]
        0x3D, 0x34, 0x12, // cmp ax,1234h
        0x75, 0x08, // jne bad
        0x0B, 0xD2, // or dx,dx
        0x75, 0x04, // jne bad
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp over the next
        0xB0, 0x00, // bad: mov al,0
    ])
    .exit_al();
    let dword = code.here();
    code.emit(&[0xFF, 0xFF, 0x00, 0x00]);
    code.patch(read + 2, dword);
    code.patch(read + 7, dword + 2);
    let target = ne::Target::Internal {
        segment: 1,
        offset: 0x1234,
    };
    code.fixup(dword, ne::ADDR_OFFSET32, target);
    let (exit, log) = run("offset32", program(&code, CODE | ne::SEG_32BIT));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn cli_and_sti_go_on_in_a_task() {
    // Windows virtualizes the interrupt flag of its protected-mode
    // programs: CLI and STI at level 3 go on. Exit code 7 if they did.
    let mut code = Code::new();
    code.emit(&[
        0xFA, // cli
        0xFB, // sti
        0xB0, 0x07, // mov al,7
    ])
    .exit_al();
    let (exit, log) = run("cli_sti", program(&code, CODE));
    assert_eq!(exit, 7, "{log}");
}

#[test]
fn a_real_mode_handler_a_task_set_runs_for_its_int() {
    // Windows 98 Setup's way of running code in real mode: the code goes
    // into a block of GlobalDOSAlloc, its address into the real-mode
    // vector table through __0000H, and an INT of the task's runs it,
    // with the task's registers in and out. The handler gives AL = BL +
    // 1, so the exit code is 42h if it ran.
    let mut code = Code::new();
    code.push(0)
        .push(0x10)
        .call("KERNEL", GLOBAL_DOS_ALLOC)
        .emit(&[
            0x8E, 0xC0, // mov es,ax: the block's selector
            0x26, 0xC7, 0x06, 0x00, 0x00, 0x88, 0xD8, // mov es:[0],"mov al,bl"
            0x26, 0xC7, 0x06, 0x02, 0x00, 0xFE, 0xC0, // mov es:[2],"inc al"
            0x26, 0xC6, 0x06, 0x04, 0x00, 0xCF, // mov es:[4],"iret"
        ]);
    let zero = code.here() + 1;
    code.emit(&[
        0xBB, 0xFF, 0xFF, // mov bx,__0000H
        0x8E, 0xC3, // mov es,bx
        0xFA, // cli
        0x26, 0xC7, 0x06, 0x80, 0x01, 0x00, 0x00, // mov es:[180h],0: INT 60h
        0x26, 0x89, 0x16, 0x82, 0x01, // mov es:[182h],dx: the segment
        0xFB, // sti
        0xB3, 0x41, // mov bl,41h
        0xCD, 0x60, // int 60h
        0x26, 0x66, 0xC7, 0x06, 0x80, 0x01, 0, 0, 0, 0, // mov es:[180h],0
    ])
    .exit_al();
    let kernel = code.module("KERNEL");
    let target = ne::Target::Ordinal {
        module: kernel,
        ordinal: ZERO_SELECTOR,
    };
    code.fixup(zero, ne::ADDR_OFFSET, target);
    let (exit, log) = run("real_mode_handler", program(&code, CODE));
    assert_eq!(exit, 0x42, "{log}");
}
