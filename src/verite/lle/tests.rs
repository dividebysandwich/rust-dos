use super::risc::asm::*;
use super::*;

/// A board that remembers what the chip wrote to its registers.
#[derive(Default)]
struct TestBoard {
    writes: Vec<(u8, u32, u8)>,
    ram: Vec<u32>,
}

impl Board for TestBoard {
    fn ext_read(&mut self, _io: u8, _len: u8) -> u32 {
        0
    }
    fn ext_write(&mut self, io: u8, value: u32, len: u8) {
        self.writes.push((io, value, len));
    }
    fn dma_read(&mut self, phys: u32) -> u32 {
        self.ram.get(phys as usize / 4).copied().unwrap_or(0)
    }
}

const HALT: u32 = 0x6D00_0000;

/// A chip running `program` from `at`, in 1 MB of memory.
fn machine(program: &[u32], at: u32) -> (Core, Vec<u8>, TestBoard) {
    let mut memory = vec![0u8; 1 << 20];
    for (k, &word) in program.iter().enumerate() {
        mem::write32(&mut memory, at + 4 * k as u32, word);
    }
    let mut core = Core::new(rom_image());
    core.regs[io::DEBUG as usize] = 0;
    core.pc = at;
    core.npc = risc::next(at);
    (core, memory, TestBoard::default())
}

fn run(core: &mut Core, memory: &mut [u8], board: &mut TestBoard) -> u64 {
    core.run(memory, board, 10_000)
}

#[test]
fn alu_operations_and_delay_slots() {
    let program = [
        imm16(0x76, 128, 0x1234), // li     %128, #1234h
        imm16(0x77, 129, 0x8000), // lui    %129, #8000h
        int(0x10, 130, 128, 129), // add    %130, %128, %129
        int(0x46, 131, 129, 4),   // asr    %131, %129, #4
        int(0x47, 132, 129, 4),   // sr     %132, %129, #4
        branch(0x61, 128, 3),     // bnez   %128, +3
        int(0x00, 133, 0, 7),     // add    %133, zero, #7 (delay slot)
        int(0x00, 134, 0, 1),     // skipped
        int(0x00, 135, 0, 2),     // skipped
        int(0x13, 136, 128, 133), // subf   %136, %128, %133
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0x1000);
    run(&mut core, &mut memory, &mut board);
    assert!(core.halted);
    assert_eq!(core.reg[130], 0x8000_1234);
    assert_eq!(core.reg[131], 0xF800_0000);
    assert_eq!(core.reg[132], 0x0800_0000);
    assert_eq!(core.reg[133], 7);
    assert_eq!((core.reg[134], core.reg[135]), (0, 0));
    assert_eq!(core.reg[136], 7u32.wrapping_sub(0x1234));
    assert_ne!(core.intr & intr::HALT, 0);
}

#[test]
fn vector_operations_work_on_register_pairs() {
    let program = [
        imm16(0x76, 128, 10),
        imm16(0x76, 129, 20),
        imm16(0x76, 130, 1),
        imm16(0x76, 131, 2),
        int(0x20, 132, 128, 130), // addv   %132, %128, %130
        int(0x30, 134, 128, 131), // addvs  %134, %128, %131
        int(0xC0, 136, 0, 3),     // getxy  %136, msb
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    run(&mut core, &mut memory, &mut board);
    assert_eq!(&core.reg[132..136], &[11, 22, 12, 22]);
    assert_eq!(&core.reg[136..138], &[0x8000_0000, 0]);
}

#[test]
fn links_point_past_the_delay_slot() {
    let program = [
        int(0x6B, 128, 0, 0),   // 0: getpc %128
        jump(0x6E, 0x10 >> 2),  // 4: jmpl 10h
        0,                      // 8: nop
        HALT,                   // C
        int(0x6F, 129, 0, 254), // 10: jmprl %129, (ra)
        0,                      // 14: nop
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    run(&mut core, &mut memory, &mut board);
    assert_eq!(core.reg[128], 8);
    assert_eq!(core.reg[254], 0xC);
    assert_eq!(core.reg[129], 0x18);
    assert!(core.halted);
    assert_eq!(core.pc, 0xC);
}

#[test]
fn memory_is_big_endian_to_the_risc() {
    let program = [
        imm16(0x77, 128, 0x1122),
        int(0x4B, 128, 128, 0x33), // addsl8 %128, %128, #33h
        int(0x00, 128, 128, 0x44), // add    %128, %128, #44h
        imm16(0x76, 129, 0x800),
        0x7A00_8081, // sw     0(%129), %128
        0x7082_0181, // lb     %130, 1(%129)
        0x7183_0181, // lh     %131, 1(%129)  (+2)
        0x7801_0081, // sb     1(%129), zero
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    run(&mut core, &mut memory, &mut board);
    assert_eq!(core.reg[128], 0x1122_3344);
    assert_eq!(core.reg[130], 0x22);
    assert_eq!(core.reg[131], 0x3344);
    // The host sees the word as it was stored, its second byte cleared.
    assert_eq!(&memory[0x800..0x804], &[0x44, 0x33, 0x00, 0x11]);
}

#[test]
fn the_fifo_stalls_the_risc_until_the_host_writes() {
    let program = [
        int(0x10, 128, 0, 33), // add %128, zero, fifo
        int(0x00, 33, 128, 1), // add fifo, %128, #1
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    assert_eq!(run(&mut core, &mut memory, &mut board), 0);
    assert!(core.stalled);
    assert_eq!(core.io_read(io::STATUS, 1, &memory) & 0x10, 0x10);
    core.io_write(io::FIFO, 41, 4, &mut memory, &mut board);
    run(&mut core, &mut memory, &mut board);
    assert!(core.halted);
    assert_eq!(core.io_read(io::FIFO_OUT_VALID, 1, &memory), 1);
    assert_eq!(core.io_read(io::FIFO, 4, &memory), 42);
    assert_eq!(core.io_read(io::FIFO_OUT_VALID, 1, &memory), 0);
}

// v1krisc.c's writeRF, risc_writemem, readRF and v1k_start through the
// debug registers.
#[test]
fn the_host_forces_instructions_through_the_debug_registers() {
    let (mut core, mut memory, mut board) = machine(&[], 0);
    let mut force = |core: &mut Core, memory: &mut Vec<u8>, ir: u32| {
        core.io_write(io::STATE_INDEX, 0x80, 1, memory, &mut board);
        core.io_write(io::STATE_DATA, ir, 4, memory, &mut board);
        core.io_write(
            io::DEBUG,
            (debug::HOLD | debug::STEP) as u32,
            1,
            memory,
            &mut board,
        );
        assert_eq!(core.io_read(io::DEBUG, 1, memory), debug::HOLD as u32);
    };
    core.io_write(
        io::DEBUG,
        (debug::SOFT_RESET | debug::HOLD) as u32,
        1,
        &mut memory,
        &mut TestBoard::default(),
    );
    core.io_write(
        io::DEBUG,
        debug::HOLD as u32,
        1,
        &mut memory,
        &mut TestBoard::default(),
    );
    assert_eq!(core.pc, RESET_PC);
    force(&mut core, &mut memory, 0x76FE_0800); // li ra, #800h
    force(&mut core, &mut memory, 0x77FF_1234); // lui fp, #1234h
    force(&mut core, &mut memory, 0x4BFF_FF56);
    force(&mut core, &mut memory, 0x00FF_FF78);
    force(&mut core, &mut memory, 0x7A00_FFFE); // sw 0(ra), fp
    assert_eq!(mem::read32(&memory, 0x800), 0x1234_5678);
    // readRF: "add zero, zero, fp" in decode, then S1.
    core.io_write(
        io::STATE_DATA,
        0x1000_00FF,
        4,
        &mut memory,
        &mut TestBoard::default(),
    );
    core.io_write(
        io::STATE_INDEX,
        0x82,
        1,
        &mut memory,
        &mut TestBoard::default(),
    );
    assert_eq!(core.io_read(io::STATE_DATA, 4, &memory), 0x1234_5678);
    // v1k_start: a jump and the nop in its delay slot.
    force(&mut core, &mut memory, jump(0x6C, 0x800 >> 2));
    force(&mut core, &mut memory, 0);
    core.io_write(
        io::STATE_INDEX,
        0x81,
        1,
        &mut memory,
        &mut TestBoard::default(),
    );
    assert_eq!(core.io_read(io::STATE_DATA, 4, &memory), 0x800);
}

#[test]
fn the_seed_gives_reciprocals() {
    // seedsr D, S2, #n gives 2^(30 + n) / S2.
    let program = [
        imm16(0x76, 129, 4),
        int(0x42, 128, 129, 0),
        int(0x42, 130, 129, 2),
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    run(&mut core, &mut memory, &mut board);
    assert_eq!(core.scale, 3);
    assert_eq!(core.reg[128], 1 << 28);
    assert_eq!(core.reg[130], 1 << 30);
}

#[test]
fn dma_feeds_the_fifo_from_a_command_list() {
    let program = [
        int(0x10, 128, 0, 33),
        int(0x10, 129, 0, 33),
        int(0x10, 130, 0, 33),
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    // The list at 100h: a block of three words at 200h, swapped by bytes.
    board.ram = vec![0; 0x100];
    board.ram[0x40] = 0x200;
    board.ram[0x41] = 12 | 1;
    board.ram[0x80..0x83].copy_from_slice(&[0x1122_3344, 2, 3]);
    core.io_write(io::MODE, mode::DMA as u32, 1, &mut memory, &mut board);
    core.io_write(io::DMA_CMD_PTR, 0x100, 4, &mut memory, &mut board);
    run(&mut core, &mut memory, &mut board);
    assert!(core.halted);
    assert_eq!(
        &core.reg[128..131],
        &[0x4433_2211, 0x0200_0000, 0x0300_0000]
    );
    assert!(!core.dma.busy);
    assert_ne!(core.intr & intr::DMA, 0);
}

/// Pixel engine set up for a 16-bit 565 destination 1024 bytes a line.
fn sixteen_bit(core: &mut Core) {
    core.pe.dst_fmt = 4;
    core.pe.stride = 0x0300; // DstStride0 = 3: 1024 bytes
    core.pe.dst_base = 0x1_0000;
}

#[test]
fn a_parallel_draw_fills_a_span() {
    let program = [
        // stepv xy with XShadow 3, Y 2 (16.16) and Cnt 5.
        imm16(0x77, 128, 3),
        imm16(0x77, 129, 2),
        int(0xD8, 140, 128, 0),
        imm16(0x77, 130, 5),
        int(0xD7, 142, 130, 0),
        int(0xE1, 144, 146, 148), // rdrawp
        HALT,
    ];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    sixteen_bit(&mut core);
    core.pe.fg = 0xF800_F800;
    core.pe.dst_mode = 0xC;
    run(&mut core, &mut memory, &mut board);
    let pixel = |x: u32| mem::read16(&memory, 0x1_0000 + 2 * 1024 + 2 * x);
    assert_eq!(
        (pixel(2), pixel(3), pixel(7), pixel(8)),
        (0, 0xF800, 0xF800, 0)
    );
    assert_eq!(core.pe.cnt, 0);
    assert!(core.drew);
}

#[test]
fn a_parallel_blit_copies_and_xors() {
    let (mut core, mut memory, _) = machine(&[HALT], 0);
    sixteen_bit(&mut core);
    core.pe.stride = 0x0303;
    core.pe.src_base = 0x2_0000;
    core.pe.src_mode = 1 << 4;
    for x in 0..4 {
        mem::write16(&mut memory, 0x2_0000 + 2 * x, 0x100 + x);
        mem::write16(&mut memory, 0x1_0000 + 2 * x, 0x00FF);
    }
    core.pe.dst_mode = 6; // XOR
    core.pe.cnt = 4;
    core.pe.set(pixel::Param::U, 0);
    core.draw_parallel(0xE1, 200, 202, 204, &mut memory);
    for x in 0..4 {
        assert_eq!(mem::read16(&memory, 0x1_0000 + 2 * x), (0x100 + x) ^ 0xFF);
    }
}

#[test]
fn a_parameterized_draw_iterates_its_parameters() {
    let (mut core, mut memory, _) = machine(&[HALT], 0);
    sixteen_bit(&mut core);
    core.pe.dst_mode = 0xC;
    core.pe.dst_fmt = 0xC; // 8888, 4 bytes
    core.pe.stride = 0x0400; // 2 << 2? Shift 2: 4 bytes a line, unused at Y 0
    core.pe.drawctl = 6; // pair1: X, R; pair2 (0): G, B
    // X 0, red 10 rising by 20; green and blue 0.
    core.reg[160] = 0;
    core.reg[161] = 10 << 16;
    core.reg[176] = 1 << 16;
    core.reg[177] = 20 << 16;
    core.pe.cnt = 3;
    core.pe.a = 255 << 16;
    core.draw_param(0xF3, 192, 160, 176, &mut memory);
    let pixel = |x: u32| mem::read32(&memory, 0x1_0000 + 4 * x);
    assert_eq!(pixel(0), 0xFF0A_0000);
    assert_eq!(pixel(1), 0xFF1E_0000);
    assert_eq!(pixel(2), 0xFF32_0000);
    assert_eq!(core.pe.cnt, 0);
    // A single draw adds the deltas into D.
    core.pe.cnt = 1;
    core.draw_param(0xF0, 192, 160, 176, &mut memory);
    assert_eq!((core.reg[192], core.reg[193]), (1 << 16, 30 << 16));
}

#[test]
fn textures_are_sampled_through_the_masks() {
    let (mut core, mut memory, _) = machine(&[HALT], 0);
    sixteen_bit(&mut core);
    core.pe.src_base = 0x3_0000;
    core.pe.src_mode = 4 | 1 << 4; // 565, replace
    core.pe.stride = 0x0301 | 1 << 4; // source lines of 256 + 16 bytes: unused at V 0
    core.pe.src_mask = 0x0003_0003;
    for x in 0..4 {
        mem::write16(&mut memory, 0x3_0000 + 2 * x, 0x1000 * (x + 1));
    }
    core.pe.dst_mode = 0xC;
    // U = 5 wraps to 1.
    core.pe.set(pixel::Param::U, 5 << 16);
    core.pe.set(pixel::Param::V, 0);
    core.pe.set(pixel::Param::X, 0);
    core.pe.set(pixel::Param::Y, 0);
    core.pe.cnt = 1;
    core.pe.drawctl = 0;
    core.reg[160] = 0;
    core.draw_param(0xF0, 192, 160, 176, &mut memory);
    assert_eq!(mem::read16(&memory, 0x1_0000), 0x2000);
}

#[test]
fn the_rom_loader_stores_sections_and_starts_the_microcode() {
    let rom = rom_image();
    let stub: Vec<u32> = (0..6)
        .map(|k| u32::from_be_bytes(rom[0x10000 + 4 * k..][..4].try_into().unwrap()))
        .collect();
    let (mut core, mut memory, mut board) = machine(&stub, 0x3F_F800);
    run(&mut core, &mut memory, &mut board);
    assert!(core.loader.active);
    for word in [0x2000, 2, 0x1111_1111, HALT, 0x3000, 1, 7, 0, 0] {
        core.io_write(io::FIFO, word, 4, &mut memory, &mut board);
    }
    run(&mut core, &mut memory, &mut board);
    assert!(!core.loader.active);
    assert_eq!(mem::read32(&memory, 0x2000), 0x1111_1111);
    assert_eq!(mem::read32(&memory, 0x3000), 7);
    // It started at the first section, and halted at its second word.
    assert!(core.halted);
    assert_eq!(core.pc, 0x2004);
}

#[test]
fn the_palette_loader_writes_the_ramdac() {
    let mut program = PALETTE_LOADER.to_vec();
    program.extend([HALT; 2]);
    let (mut core, mut memory, mut board) = machine(&program, 0);
    core.reg[254] = 4 * PALETTE_LOADER.len() as u32;
    for word in [5 << 16 | 2, 0x0011_2233, 0x0044_5566] {
        core.io_write(io::FIFO, word, 4, &mut memory, &mut board);
    }
    run(&mut core, &mut memory, &mut board);
    assert!(core.halted);
    let rgb: Vec<u32> = board
        .writes
        .iter()
        .filter(|w| w.0 == 0xB1)
        .map(|w| w.1)
        .collect();
    assert_eq!(board.writes[0], (0xB0, 5, 1));
    assert_eq!(rgb, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
}

#[test]
fn the_state_saves_and_loads() {
    let program = [int(0x10, 128, 0, 33), int(0x00, 33, 128, 1), HALT];
    let (mut core, mut memory, mut board) = machine(&program, 0);
    run(&mut core, &mut memory, &mut board);
    let mut w = Writer::new();
    core.save(&mut w);
    let mut copy = Core::new(rom_image());
    copy.load(&mut Reader::new(&w.buf)).unwrap();
    copy.io_write(io::FIFO, 1, 4, &mut memory, &mut board);
    run(&mut copy, &mut memory, &mut board);
    assert_eq!(copy.io_read(io::FIFO, 4, &memory), 2);
}

/// xf86-video-rendition's context switch monitor (`cscode.h`), which waits
/// at 800h for a command and jumps to the microcode's entry.
const CSRISC: [u32; 30] = [
    0x1080_2100,
    0x5D80_8000,
    0x4C80_8002,
    0x6B82_0000,
    0x0081_8002,
    0x4581_8103,
    0x1082_8281,
    0x6F00_0082,
    0x0000_0000,
    0x6200_0500,
    0x0000_0000,
    0x6200_0300,
    0x0000_0000,
    0x6200_0800,
    0x0000_0000,
    0x1081_2100,
    0x1082_2100,
    0x10C0_2100,
    0x6FFE_00C0,
    0x0000_0000,
    0x62FF_EB00,
    0x0000_0000,
    0x0481_2502,
    0x61FF_FE81,
    0x0000_0000,
    0x1021_8000,
    0x0000_0000,
    0x0000_0000,
    0x62FF_E300,
    0x0000_0000,
];

// The V1000 2D microcode of xf86-video-rendition, driven as its
// `accelX.c` does: the monitor started with forced steps, the microcode
// started through it, SETUP for 640x480 at 16 bits, fills, a blit and the
// GET_PIXEL sync. The microcode isn't in the repository: the test runs
// when `target/verite-lle/xorg/v10002d.uc` is there.
#[test]
fn xorg_2d_microcode_draws() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/verite-lle/xorg/v10002d.uc");
    let Ok(elf) = std::fs::read(&path) else {
        eprintln!("{} not there: skipped", path.display());
        return;
    };
    let mut memory = vec![0u8; 4 << 20];
    let mut board = TestBoard::default();
    let mut core = Core::new(rom_image());
    let (debug_reg, hold) = (io::DEBUG, debug::HOLD as u32);
    core.io_write(
        debug_reg,
        (debug::SOFT_RESET | debug::HOLD) as u32,
        1,
        &mut memory,
        &mut board,
    );
    core.io_write(debug_reg, hold, 1, &mut memory, &mut board);
    for (k, &word) in CSRISC.iter().enumerate() {
        mem::write32(&mut memory, 0x800 + 4 * k as u32, word);
    }
    // The one segment, at 1000h, in the RISC's byte order.
    let at = |o: usize| u32::from_be_bytes(elf[o..o + 4].try_into().unwrap()) as usize;
    let (offset, size) = (at(52 + 4), at(52 + 16));
    for (i, &byte) in elf[offset..offset + size].iter().enumerate() {
        mem::write8(&mut memory, 0x1000 + i as u32, byte as u32);
    }
    let entry = at(24) as u32;
    for ir in [0, 0, 0, jump(0x6C, 0x800 >> 2), 0] {
        core.io_write(io::STATE_INDEX, 0x80, 1, &mut memory, &mut board);
        core.io_write(io::STATE_DATA, ir, 4, &mut memory, &mut board);
        core.io_write(
            debug_reg,
            hold | debug::STEP as u32,
            1,
            &mut memory,
            &mut board,
        );
    }
    core.io_write(debug_reg, 0, 1, &mut memory, &mut board);
    core.run(&mut memory, &mut board, 100_000);
    assert_eq!(core.pc, 0x800, "the monitor waits for a command");
    let p2 = |a: u32, b: u32| a << 16 | b;
    let mut send = |core: &mut Core, memory: &mut Vec<u8>, words: &[u32]| {
        for &w in words {
            while core.fifo_full() {
                core.run(memory, &mut board, 1000);
            }
            core.io_write(io::FIFO, w, 4, memory, &mut board);
        }
        core.run(memory, &mut board, 1_000_000);
    };
    send(&mut core, &mut memory, &[0, 0, 0, entry]);
    send(
        &mut core,
        &mut memory,
        &[
            32,
            p2(640, 480),
            p2(16, 4),
            0x1_0000,
            1280,
            5 << 12 | 1 << 8,
        ],
    );
    let pixel = |memory: &[u8], x: u32, y: u32| mem::read16(memory, 0x1_0000 + y * 1280 + 2 * x);
    // RECT_SOLID_ROP with S: red.
    send(
        &mut core,
        &mut memory,
        &[p2(0xC, 41), 0xF800_F800, p2(10, 20), p2(30, 4)],
    );
    assert_eq!(pixel(&memory, 10, 20), 0xF800);
    assert_eq!(pixel(&memory, 39, 23), 0xF800);
    assert_eq!(
        (
            pixel(&memory, 9, 20),
            pixel(&memory, 40, 20),
            pixel(&memory, 10, 24)
        ),
        (0, 0, 0)
    );
    // With XOR.
    send(
        &mut core,
        &mut memory,
        &[p2(6, 41), 0xFFFF_FFFF, p2(12, 21), p2(2, 1)],
    );
    assert_eq!(
        (
            pixel(&memory, 12, 21),
            pixel(&memory, 13, 21),
            pixel(&memory, 14, 21)
        ),
        (0x07FF, 0x07FF, 0xF800)
    );
    // SCREEN_BLT down and to the right.
    send(
        &mut core,
        &mut memory,
        &[12, 0xC, p2(10, 20), p2(30, 4), p2(100, 100)],
    );
    assert_eq!(
        (pixel(&memory, 100, 100), pixel(&memory, 102, 101)),
        (0xF800, 0x07FF)
    );
    assert_eq!(
        (pixel(&memory, 129, 103), pixel(&memory, 130, 103)),
        (0xF800, 0)
    );
    // GET_PIXEL answers through the output FIFO.
    send(&mut core, &mut memory, &[9, 0]);
    assert!(core.io_read(io::FIFO_OUT_VALID, 1, &memory) > 0);
}
