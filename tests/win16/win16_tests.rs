//! The built-in Windows's Win16 loader and memory: Windows 3.1 programs
//! with segments flagged 32-bit, and the selectors such programs make
//! 32-bit; libraries that load their own segments; what Windows does for
//! their CLI, STI and INT n; and the real-mode code they run through
//! DPMI (INT 31h AX=0300h to 0304h). The programs are made byte by byte
//! (tests/common/win16), or assembled.

use crate::win16;

use iced_x86::code_asm::*;
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
fn cli_at_offset_28h_goes_on_in_a_task() {
    // The #GP of a task's CLI at offset 28h pushes an error code and an
    // EIP of 28h, where a frame from level 0 has its CS, the system's
    // level 0 code segment. It is still the task's fault, and CLI goes
    // on. Exit code 7 if it did.
    let mut code = Code::new();
    code.emit(&[0x90; 0x28]);
    assert_eq!(code.here(), 0x28);
    code.emit(&[
        0xFA, // cli
        0xFB, // sti
        0xB0, 0x07, // mov al,7
    ])
    .exit_al();
    let (exit, log) = run("cli_at_28h", program(&code, CODE));
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

/// The DPMI tests' places in the automatic data segment: the DOS block's
/// segment and selector, the callback's address and how many times its
/// procedure ran, and three real mode call structures.
const RM_SEG: u32 = 0x10;
const RM_SEL: u32 = 0x12;
const CB_ADDR: u32 = 0x14;
const CB_COUNT: u32 = 0x18;
const STRUCT: u32 = 0x20;
const STRUCT2: u32 = 0x60;
const STRUCT3: u32 = 0xA0;

/// The fields of a real mode call structure.
const RC_EDX: u32 = 0x14;
const RC_EBX: u32 = 0x10;
const RC_ECX: u32 = 0x18;
const RC_EAX: u32 = 0x1C;
const RC_FLAGS: u32 = 0x20;
const RC_IP: u32 = 0x2A;
const RC_CS: u32 = 0x2C;
const RC_SP: u32 = 0x2E;
const RC_SS: u32 = 0x30;

/// Where a callback's procedure is in the code segment.
const PROC: usize = 0x280;

/// Where the real-mode code is in the code segment, and its bytes. It is
/// copied to offset 0 of a block of DOS memory of `RM_PARAS`, whose
/// last bytes hold the callback's address (`RM_CB_PTR`) and a stack.
const REAL: usize = 0x300;
const REAL_LEN: usize = 0x100;
const RM_PARAS: u32 = 0x20;
const RM_CB_PTR: u16 = 0x1F0;
const RM_STACK: u32 = 0x1E0;

fn asm16(f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    f(&mut a).unwrap();
    a.assemble(0).unwrap()
}

/// Real-mode code with `parts` at their offsets.
fn real_code(parts: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut code = vec![0xCC; REAL_LEN];
    for (at, bytes) in parts {
        code[*at..*at + bytes.len()].copy_from_slice(bytes);
    }
    code
}

/// Zero the call structure at `at` of the data segment.
fn clear_struct(a: &mut CodeAssembler, at: u32) -> Result<(), IcedError> {
    a.push(ds)?;
    a.pop(es)?;
    a.mov(di, at)?;
    a.mov(cx, 0x19)?;
    a.xor(ax, ax)?;
    a.cld()?;
    a.rep().stosw()
}

/// Point the real-mode vector `vector` at `offset` of the DOS block, with
/// DPMI 0201h.
fn set_vector(a: &mut CodeAssembler, vector: u32, offset: u32) -> Result<(), IcedError> {
    a.mov(ax, 0x0201)?;
    a.mov(bl, vector)?;
    a.mov(cx, word_ptr(RM_SEG))?;
    a.mov(dx, offset)?;
    a.int(0x31)
}

/// INT 31h function `function` with the structure at `at`, BX `bx` and
/// CX `words`, going to `fail` if it sets CF.
fn call_real(
    a: &mut CodeAssembler,
    function: u32,
    bx_value: u32,
    words: u32,
    at: u32,
    fail: CodeLabel,
) -> Result<(), IcedError> {
    a.mov(ax, function)?;
    a.mov(bx, bx_value)?;
    a.mov(cx, words)?;
    a.push(ds)?;
    a.pop(es)?;
    a.mov(di, at)?;
    a.int(0x31)?;
    a.jc(fail)
}

/// The structure's CS:IP: the DOS block's `offset`.
fn set_target(a: &mut CodeAssembler, at: u32, offset: u32) -> Result<(), IcedError> {
    a.mov(word_ptr(at + RC_IP), offset)?;
    a.mov(ax, word_ptr(RM_SEG))?;
    a.mov(word_ptr(at + RC_CS), ax)
}

/// A Windows 3.1 program that copies `real` to a block of DOS memory
/// (DPMI 0100h, through a data alias of its code segment) and runs
/// `body`, which ends it with AL as the exit code. A DPMI call that
/// fails ends it with EEh: `body` gets the label to go to for that.
/// `procedure` goes at `PROC`.
fn dpmi_program(
    real: Vec<u8>,
    procedure: Vec<u8>,
    body: impl FnOnce(&mut CodeAssembler, CodeLabel) -> Result<(), IcedError>,
) -> Vec<u8> {
    let main = asm16(|a| {
        let mut fail = a.create_label();
        a.mov(ax, 0x0100)?;
        a.mov(bx, RM_PARAS)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(word_ptr(RM_SEG), ax)?;
        a.mov(word_ptr(RM_SEL), dx)?;
        a.mov(ax, 0x000A)?;
        a.mov(bx, cs)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.push(ds)?;
        a.mov(es, word_ptr(RM_SEL))?;
        a.mov(ds, ax)?;
        a.xor(di, di)?;
        a.mov(si, REAL as u32)?;
        a.mov(cx, REAL_LEN as u32)?;
        a.cld()?;
        a.rep().movsb()?;
        a.pop(ds)?;
        body(a, fail)?;
        a.mov(ah, 0x4C)?;
        a.int(0x21)?;
        a.set_label(&mut fail)?;
        a.mov(ax, 0x4CEE)?;
        a.int(0x21)
    });
    assert!(
        main.len() <= PROC,
        "the program's code runs into what follows"
    );
    assert!(PROC + procedure.len() <= REAL);
    let mut bytes = main;
    bytes.resize(PROC, 0xCC);
    bytes.extend_from_slice(&procedure);
    bytes.resize(REAL, 0xCC);
    bytes.extend_from_slice(&real);
    let mut code = Code::new();
    code.emit(&bytes);
    program(&code, CODE)
}

#[test]
fn dpmi_0300h_runs_a_handler_a_task_put_in_the_vector_table() {
    // The handler gives AL = BL + 1 and CF set, returning with RETF 2 as
    // DOS's handlers do: the structure gets both.
    let real = real_code(&[(0, vec![0x88, 0xD8, 0xFE, 0xC0, 0xF9, 0xCA, 0x02, 0x00])]);
    let (exit, log) = run(
        "dpmi_0300h_hooked",
        dpmi_program(real, Vec::new(), |a, fail| {
            set_vector(a, 0x60, 0)?;
            clear_struct(a, STRUCT)?;
            a.mov(byte_ptr(STRUCT + RC_EBX), 0x41)?;
            call_real(a, 0x0300, 0x60, 0, STRUCT, fail)?;
            a.test(byte_ptr(STRUCT + RC_FLAGS), 1)?;
            a.jz(fail)?;
            a.mov(al, byte_ptr(STRUCT + RC_EAX))
        }),
    );
    assert_eq!(exit, 0x42, "{log}");
}

#[test]
fn dpmi_0300h_runs_the_bios_for_a_vector_it_has() {
    // INT 12h through the vector table's own handler, the BIOS's: the
    // memory size at 40h:13h.
    let (exit, log) = run(
        "dpmi_0300h_bios",
        dpmi_program(real_code(&[]), Vec::new(), |a, fail| {
            clear_struct(a, STRUCT)?;
            call_real(a, 0x0300, 0x12, 0, STRUCT, fail)?;
            a.mov(ax, 0x0002)?;
            a.mov(bx, 0x40)?;
            a.int(0x31)?;
            a.jc(fail)?;
            a.mov(es, ax)?;
            a.mov(ax, word_ptr(0x13).es())?;
            a.cmp(ax, word_ptr(STRUCT + RC_EAX))?;
            a.jne(fail)?;
            a.cmp(ax, 0)?;
            a.je(fail)?;
            a.mov(al, 1)
        }),
    );
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn dpmi_0301h_calls_a_procedure_with_words_of_the_tasks_stack() {
    // The procedure gives the second word less the first: both copied,
    // in the order the task's stack has them.
    let procedure = asm16(|a| {
        a.push(bp)?;
        a.mov(bp, sp)?;
        a.mov(ax, word_ptr(bp + 8))?;
        a.sub(ax, word_ptr(bp + 6))?;
        a.pop(bp)?;
        a.retf()
    });
    let (exit, log) = run(
        "dpmi_0301h",
        dpmi_program(real_code(&[(0, procedure)]), Vec::new(), |a, fail| {
            clear_struct(a, STRUCT)?;
            set_target(a, STRUCT, 0)?;
            a.push(0x1234)?;
            a.push(0x0101)?;
            call_real(a, 0x0301, 0, 2, STRUCT, fail)?;
            a.add(sp, 4)?;
            a.cmp(word_ptr(STRUCT + RC_EAX), 0x1133)?;
            a.jne(fail)?;
            a.mov(al, 1)
        }),
    );
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn dpmi_0302h_calls_a_procedure_with_an_iret_frame() {
    // The procedure starts with IF and TF clear (DX), finds the word
    // copied above the IRET frame, and the flags it returns (CF, from
    // the frame, which has the structure's) go back in the structure.
    let procedure = asm16(|a| {
        a.pushf()?;
        a.pop(dx)?;
        a.push(bp)?;
        a.mov(bp, sp)?;
        a.mov(ax, word_ptr(bp + 8))?;
        a.pop(bp)?;
        a.iret()
    });
    let (exit, log) = run(
        "dpmi_0302h",
        dpmi_program(real_code(&[(0, procedure)]), Vec::new(), |a, fail| {
            clear_struct(a, STRUCT)?;
            set_target(a, STRUCT, 0)?;
            a.mov(word_ptr(STRUCT + RC_FLAGS), 0x0303)?;
            a.push(0x0042)?;
            call_real(a, 0x0302, 0, 1, STRUCT, fail)?;
            a.add(sp, 2)?;
            a.test(byte_ptr(STRUCT + RC_FLAGS), 1)?;
            a.jz(fail)?;
            a.test(word_ptr(STRUCT + RC_EDX), 0x0300)?;
            a.jnz(fail)?;
            a.mov(al, byte_ptr(STRUCT + RC_EAX))
        }),
    );
    assert_eq!(exit, 0x42, "{log}");
}

#[test]
fn dpmi_real_mode_code_runs_on_the_stack_its_structure_gives() {
    // The procedure gives its SS:SP: the structure's, under the return
    // address, and with SS:SP 0:0 the host's. The structure keeps its
    // SS:SP either way.
    let procedure = asm16(|a| {
        a.mov(ax, ss)?;
        a.mov(dx, sp)?;
        a.retf()
    });
    let (exit, log) = run(
        "dpmi_real_stack",
        dpmi_program(real_code(&[(0, procedure)]), Vec::new(), |a, fail| {
            clear_struct(a, STRUCT)?;
            set_target(a, STRUCT, 0)?;
            a.mov(ax, word_ptr(RM_SEG))?;
            a.mov(word_ptr(STRUCT + RC_SS), ax)?;
            a.mov(word_ptr(STRUCT + RC_SP), RM_STACK)?;
            call_real(a, 0x0301, 0, 0, STRUCT, fail)?;
            a.mov(ax, word_ptr(RM_SEG))?;
            a.cmp(word_ptr(STRUCT + RC_EAX), ax)?;
            a.jne(fail)?;
            a.cmp(word_ptr(STRUCT + RC_EDX), RM_STACK - 4)?;
            a.jne(fail)?;
            a.cmp(word_ptr(STRUCT + RC_SP), RM_STACK)?;
            a.jne(fail)?;
            a.mov(word_ptr(STRUCT + RC_SS), 0)?;
            a.mov(word_ptr(STRUCT + RC_SP), 0)?;
            call_real(a, 0x0301, 0, 0, STRUCT, fail)?;
            a.mov(ax, word_ptr(RM_SEG))?;
            a.cmp(word_ptr(STRUCT + RC_EAX), ax)?;
            a.je(fail)?;
            a.cmp(word_ptr(STRUCT + RC_EAX), 0)?;
            a.je(fail)?;
            a.cmp(word_ptr(STRUCT + RC_SS), 0)?;
            a.jne(fail)?;
            a.mov(al, 1)
        }),
    );
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn dpmi_real_mode_code_calls_back_into_a_task_that_calls_real_mode_again() {
    // INT 60h's handler calls the task's callback (0303h), whose
    // procedure runs INT 61h's handler through 0300h, nested, and gives
    // what that returned (BL doubled) to the first handler in CX, which
    // returns it in AL.
    let int60 = asm16(|a| {
        a.db(&[0x2E, 0xFF, 0x1E])?; // call far cs:[RM_CB_PTR]
        a.dw(&[RM_CB_PTR])?;
        a.mov(al, cl)?;
        a.iret()
    });
    let int61 = asm16(|a| {
        a.mov(al, bl)?;
        a.add(al, al)?;
        a.iret()
    });
    let real = real_code(&[(0, int60), (0x40, int61)]);
    // The callback's procedure: DS:SI the real-mode stack, ES:DI the
    // structure. It returns as the RETF would, and runs on the task's
    // stack, in the automatic data segment.
    let procedure = asm16(|a| {
        let mut fail = a.create_label();
        a.mov(ax, word_ptr(si))?;
        a.mov(word_ptr(di + RC_IP).es(), ax)?;
        a.mov(ax, word_ptr(si + 2))?;
        a.mov(word_ptr(di + RC_CS).es(), ax)?;
        a.add(word_ptr(di + RC_SP).es(), 4)?;
        a.push(es)?;
        a.push(di)?;
        a.push(ss)?;
        a.pop(ds)?;
        a.inc(byte_ptr(CB_COUNT))?;
        clear_struct(a, STRUCT3)?;
        a.mov(byte_ptr(STRUCT3 + RC_EBX), 0x21)?;
        call_real(a, 0x0300, 0x61, 0, STRUCT3, fail)?;
        a.pop(di)?;
        a.pop(es)?;
        a.mov(ax, word_ptr(STRUCT3 + RC_EAX))?;
        a.mov(word_ptr(di + RC_ECX).es(), ax)?;
        a.iret()?;
        a.set_label(&mut fail)?;
        a.mov(ax, 0x4CEE)?;
        a.int(0x21)
    });
    let (exit, log) = run(
        "dpmi_callback",
        dpmi_program(real, procedure, |a, fail| {
            set_vector(a, 0x60, 0)?;
            set_vector(a, 0x61, 0x40)?;
            a.push(ds)?;
            a.pop(es)?;
            a.push(ds)?;
            a.push(cs)?;
            a.pop(ds)?;
            a.mov(si, PROC as u32)?;
            a.mov(di, STRUCT2)?;
            a.mov(ax, 0x0303)?;
            a.int(0x31)?;
            a.pop(ds)?;
            a.jc(fail)?;
            a.mov(word_ptr(CB_ADDR), dx)?;
            a.mov(word_ptr(CB_ADDR + 2), cx)?;
            a.mov(es, word_ptr(RM_SEL))?;
            a.mov(word_ptr(RM_CB_PTR as u32).es(), dx)?;
            a.mov(word_ptr(RM_CB_PTR as u32 + 2).es(), cx)?;
            clear_struct(a, STRUCT)?;
            call_real(a, 0x0300, 0x60, 0, STRUCT, fail)?;
            a.cmp(byte_ptr(CB_COUNT), 1)?;
            a.jne(fail)?;
            // Free the callback; a second time fails.
            for freed in [false, true] {
                a.mov(ax, 0x0304)?;
                a.mov(cx, word_ptr(CB_ADDR + 2))?;
                a.mov(dx, word_ptr(CB_ADDR))?;
                a.int(0x31)?;
                if freed { a.jnc(fail)? } else { a.jc(fail)? }
            }
            a.mov(al, byte_ptr(STRUCT + RC_EAX))
        }),
    );
    assert_eq!(exit, 0x42, "{log}");
}

/// SELF.DLL, a library that loads its own segments, as a compressed one
/// does: its first segment is the loader, with the loader's table at its
/// start. BootApp allocates segment 2 with the MyAlloc KERNEL put in the
/// table and writes its handle into the module database; LoadAppSeg reads
/// the segment from the file into it through a data alias. Segment 2 has
/// VALUE, which returns 42. With `dgroup`, segment 2 is instead the
/// library's automatic data segment.
fn self_loading_dll(dgroup: bool) -> Vec<u8> {
    let mut b = ne::Builder::new("SELF", ne::LIBMODULE | ne::SELFLOAD | ne::PROTMODE);
    let kernel = b.import_module("KERNEL");
    let mut relocations = Vec::new();
    let mut code = vec![0u8; 0x28];
    code[..2].copy_from_slice(b"A0");
    let mut call = |code: &mut Vec<u8>, ordinal: u16| {
        relocations.push(ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset: code.len() as u16 + 1,
            target: ne::Target::Ordinal {
                module: kernel,
                ordinal,
            },
        });
        code.extend_from_slice(&[0x9A, 0xFF, 0xFF, 0x00, 0x00]);
    };
    // BootApp(module, file).
    let boot = code.len() as u16;
    code.extend_from_slice(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp, sp
        0x1E, 0x56, // push ds; push si
        0x8E, 0x5E, 0x08, // mov ds, [bp+8] (the module database)
        0x8B, 0x36, 0x22, 0x00, // mov si, [22h] (the segment table)
        0x83, 0xC6, 0x0A, // add si, 10 (segment 2's entry)
        0xFF, 0x74, 0x04, // push word [si+4] (its flags)
        0xFF, 0x74, 0x06, // push word [si+6] (its size)
        0x6A, 0x00, // push 0
        0x2E, 0xFF, 0x1E, 0x10, 0x00, // call far [cs:10h] (MyAlloc)
        0x89, 0x54, 0x08, // mov [si+8], dx
        0x83, 0x4C, 0x04, 0x02, // or word [si+4], 2 (allocated)
        0x5E, 0x1F, 0x5D, // pop si; pop ds; pop bp
        0xCA, 0x04, 0x00, // retf 4
    ]);
    // LoadAppSeg(module, file, segment).
    let load = code.len() as u16;
    code.extend_from_slice(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp, sp
        0x1E, 0x56, 0x57, // push ds; push si; push di
        0x8E, 0x5E, 0x0A, // mov ds, [bp+0Ah]
        0x8B, 0x36, 0x22, 0x00, // mov si, [22h]
        0x8B, 0x46, 0x06, // mov ax, [bp+6]
        0x48, // dec ax
        0xB9, 0x0A, 0x00, // mov cx, 10
        0xF7, 0xE1, // mul cx
        0x03, 0xF0, // add si, ax
        0x6A, 0x00, // push 0
    ]);
    call(&mut code, 175); // AllocSelector
    code.extend_from_slice(&[
        0xFF, 0x74, 0x08, // push word [si+8]
        0x50, // push ax
    ]);
    call(&mut code, 177); // PrestoChangoSelector: a data alias
    code.extend_from_slice(&[
        0x8B, 0xF8, // mov di, ax
        0x8B, 0x04, // mov ax, [si] (the sector)
        0x33, 0xD2, // xor dx, dx
        0x8B, 0x0E, 0x32, 0x00, // mov cx, [32h] (the alignment shift)
        0xE3, 0x06, // jcxz +6
        0xD1, 0xE0, 0xD1, 0xD2, 0xE2, 0xFA, // shl ax, 1; rcl dx, 1; loop
        0xFF, 0x76, 0x08, // push word [bp+8]
        0x52, 0x50, // push dx; push ax
        0x6A, 0x00, // push 0
    ]);
    call(&mut code, 84); // _llseek
    code.extend_from_slice(&[
        0xFF, 0x76, 0x08, // push word [bp+8]
        0x57, 0x6A, 0x00, // push di; push 0
        0xFF, 0x74, 0x02, // push word [si+2] (the length)
    ]);
    call(&mut code, 82); // _lread
    code.push(0x57); // push di
    call(&mut code, 176); // FreeSelector
    code.extend_from_slice(&[
        0x83, 0x4C, 0x04, 0x04, // or word [si+4], 4 (loaded)
        0x8B, 0x44, 0x08, // mov ax, [si+8]
        0x5F, 0x5E, 0x1F, 0x5D, // pop di; pop si; pop ds; pop bp
        0xCA, 0x06, 0x00, // retf 6
    ]);
    let internal = |offset: u16| ne::Target::Internal { segment: 1, offset };
    for (at, offset) in [(4, boot), (8, load)] {
        code[at..at + 2].copy_from_slice(&[0xFF, 0xFF]);
        relocations.push(ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset: at as u16,
            target: internal(offset),
        });
    }
    b.segment(ne::SegmentSpec {
        data: code,
        flags: CODE,
        alloc: 0,
        relocations,
    });
    b.segment(ne::SegmentSpec {
        // VALUE: mov ax, 42; retf.
        data: vec![0xB8, 0x2A, 0x00, 0xCB],
        flags: if dgroup {
            ne::SEG_MOVEABLE | ne::SEG_DATA
        } else {
            CODE
        },
        alloc: 0x10,
        relocations: Vec::new(),
    });
    if dgroup {
        b.auto_data(2, 0, 0);
    }
    b.entry(
        1,
        ne::Entry::Code {
            segment: 2,
            offset: 0,
            flags: ne::ENTRY_EXPORTED,
            moveable: true,
        },
    );
    b.export("VALUE", 1, true);
    b.build()
}

#[test]
fn a_self_loading_dll_loads_its_segments_with_its_own_loader() {
    // LoadLibrary("SELF.DLL"), GetProcAddress(it, "VALUE"), and the call
    // of VALUE, whose 42 is the exit code. 1 if the library didn't load.
    let mut code = Code::new();
    let dll_name = code.here() + 2;
    code.push_far(0);
    code.call("KERNEL", 95);
    code.emit(&[0x3D, 0x20, 0x00, 0x73, 0x06, 0xB0, 0x01]); // cmp ax, 32; jae; mov al, 1
    code.exit_al();
    code.emit(&[0x50]); // push ax
    let value_name = code.here() + 2;
    code.push_far(0);
    code.call("KERNEL", 50);
    code.emit(&[0x52, 0x50, 0x8B, 0xEC, 0xFF, 0x5E, 0x00]); // push dx; push ax; call far [bp]
    code.exit_al();
    let at = code.here();
    code.patch(dll_name, at);
    code.emit(b"SELF.DLL\0");
    let at = code.here();
    code.patch(value_name, at);
    code.emit(b"VALUE\0");
    let (exit, log) = win16::run_with("self_loading_dll", program(&code, CODE), |_, dir| {
        std::fs::write(dir.join("SELF.DLL"), self_loading_dll(false)).unwrap();
    });
    assert_eq!(exit, 42, "{log}");
    assert!(
        log.contains("The loader of SELF loaded its segments"),
        "{log}"
    );
}

#[test]
fn load_library_gives_the_data_segment_a_self_loading_dlls_loader_made() {
    // LoadLibrary("SELF.DLL") of a library whose automatic data segment
    // its loader allocates returns that segment's handle, the instance,
    // as Windows does: not the module's handle, which is all there is
    // before the loader runs. 0 if they differ, 2 if they are the same.
    let mut code = Code::new();
    let dll_name = code.here() + 2;
    code.push_far(0);
    code.call("KERNEL", 95);
    code.emit(&[0x3D, 0x20, 0x00, 0x73, 0x06, 0xB0, 0x01]); // cmp ax, 32; jae; mov al, 1
    code.exit_al();
    code.emit(&[0x50]); // push ax
    let module_name = code.here() + 2;
    code.push_far(0);
    code.call("KERNEL", 47); // GetModuleHandle
    code.emit(&[0x59, 0x3B, 0xC1, 0xB0, 0x02, 0x74, 0x02, 0xB0, 0x00]); // pop cx; cmp; je
    code.exit_al();
    let at = code.here();
    code.patch(dll_name, at);
    code.emit(b"SELF.DLL\0");
    let at = code.here();
    code.patch(module_name, at);
    code.emit(b"SELF\0");
    let (exit, log) = win16::run_with("self_loading_dgroup", program(&code, CODE), |_, dir| {
        std::fs::write(dir.join("SELF.DLL"), self_loading_dll(true)).unwrap();
    });
    assert_eq!(exit, 0, "{log}");
    assert!(
        log.contains("The loader of SELF loaded its segments"),
        "{log}"
    );
}
