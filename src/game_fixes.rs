//! Fixes for particular games, beyond what the real machine does
//! (`game_fixes`). Each is an entry in [`FIXES`]: the game, what goes
//! wrong, how rust-dos recognises it, and what it does about it. Nothing
//! changes for a program that isn't recognised, and `game_fixes = false`
//! or `game_fixes_off = <names>` turns them off.
//!
//! There are three kinds:
//!
//! - [`Kind::Code`]: code bytes in conventional memory, patched in place.
//!   The check runs when a program starts a reset of the Sound Blaster's
//!   DSP (a write of 1 to base + 6), which a sound driver does before it
//!   sets anything up. It looks for each fix's first piece of code, and
//!   patches every copy where every piece is there as the game shipped
//!   it. A fix that changes what the driver builds as it sets up also
//!   needs the driver's IRQ handler to be out of the interrupt table. A
//!   driver that is already set up, as in a save state from a run without
//!   the fix, has built its tables the old way, and patching the rest
//!   could make things worse; it stays as it is until the game starts
//!   again.
//! - [`Kind::SkipAtExec`]: a program by name, which DOS's EXEC doesn't run
//!   but answers as if it had, with an exit code.
//! - [`Kind::AtLoad`]: programs by name whose files are patched as DOS
//!   loads them, from the shell or by EXEC, or as the built-in Windows
//!   makes a process of them, before any of them runs. It is for what a
//!   program hands on before a sound driver could start, such as the
//!   environment Lands of Lore 2's launchers give DOS/4GW, and for Win32
//!   programs, whose code is in extended memory.
//!
//! The Sound Blaster fixes are for the driver of two Titus games of 1993
//! with the same sound engine: Prehistorik 2 and Blues Brothers: Jukebox
//! Adventure. The engine mixes four channels of MOD music and the sound
//! effects into 8-bit unsigned blocks for DSP command 14h. It turns each
//! signed sample into an unsigned 0-63, with its centre at 32, scales
//! that by the note's volume through a table of `volume * sample / 64`,
//! and adds the channels to a block that starts at 0. So a channel's
//! centre moves with its volume, and a silent channel adds 0 where a
//! playing one adds up to 32: every note that starts, stops or changes
//! volume moves the output's centre line, which is the click at the
//! start of every note, and the output sits far below 128. The patches:
//!
//! - The volume table's loop runs the sample from -32 to 31 instead of
//!   0 to 63 and multiplies signed, so the table holds
//!   `(sample - 32) * volume / 64` in the same places.
//! - Every block starts at 128 instead of 0.
//! - The sound effects, which the driver copies into the block as they
//!   are, are stored around 128 instead of 32 when they are loaded.
//! - What the driver did when a sample ends, to make up for the jump to
//!   0 (Prehistorik 2 ramps the block down, Jukebox Adventure holds a
//!   value derived from the mix so far), goes: an ended channel now just
//!   adds nothing to the centre.
//!
//! To add a fix: an entry in [`FIXES`] with a name of its own, a test that
//! runs it (`tests/program/game_fixes_tests.rs`,
//! `tests/dos/exec_tests.rs` for a program skipped at EXEC, or
//! `tests/win32/win32_tests.rs` for a Win32 program), and a line in
//! CONFIGURATION.md's list of game fixes.

use memchr::memmem::Finder;
use std::sync::OnceLock;

/// Bytes at an offset in a program's code segment.
pub struct Bytes {
    pub at: usize,
    pub bytes: &'static [u8],
}

/// Code to recognise and patch, by offset in its code segment (or, for
/// [`Kind::AtLoad`], in its program file).
pub struct CodePatch {
    /// Code that has to be there, unchanged, besides the patches' own.
    pub checks: &'static [Bytes],
    /// Where the driver's IRQ handler starts: once an interrupt vector
    /// points there, the driver is set up and isn't patched. None for
    /// patches that don't depend on what the driver set up, which can go
    /// in at any time.
    pub irq_handler: Option<usize>,
    /// What to change: the bytes there now, and what replaces them (of
    /// the same length). The first is what the search looks for.
    pub patches: &'static [(Bytes, &'static [u8])],
}

/// How a fix recognises its game, and what it does.
pub enum Kind {
    /// Code in memory, patched when a program resets the DSP.
    Code(&'static CodePatch),
    /// A program that EXEC (AL=00h) doesn't run, when its file name is
    /// `program` and, with `beside`, that file is in the same directory.
    /// The parent gets `exit_code` from INT 21h AH=4Dh.
    SkipAtExec {
        program: &'static str,
        beside: Option<&'static str>,
        exit_code: u8,
    },
    /// Programs whose files are patched as DOS loads them or the built-in
    /// Windows makes a process of them: each program's file name with its
    /// code, for a file of that name that has, with `beside`, that file in
    /// the same directory. The offsets of the code are in the file, and
    /// only a file where every piece is as the game shipped it is patched.
    AtLoad {
        programs: &'static [(&'static str, &'static CodePatch)],
        beside: Option<&'static str>,
    },
}

/// A fix for one game.
pub struct Fix {
    /// The name `game_fixes_off` and the log use.
    pub name: &'static str,
    pub game: &'static str,
    /// What goes wrong, in a few words, for the log.
    pub reason: &'static str,
    pub kind: Kind,
}

/// The IRQ handler's loop that mixes a channel into the block: lodsb es:,
/// xlat, add [di],al, and the step through the sample. Both games have it.
const MIX_LOOP: &[u8] = &[
    0x26, 0xAC, 0xD7, 0x00, 0x05, 0x47, 0x03, 0xD5, 0x8A, 0xC6, 0x32, 0xF6, 0x98, 0x03, 0xF0, 0x3B,
    0xF4, 0x77, 0x04, 0xE2, 0xEB,
];

/// A block without a sound effect: from xor ax,ax / mov cx,54h / rep
/// stosw to mov ax,8080h / mov cl,54h / rep stosw. CX is 0 there: a JCXZ
/// leads to it.
const SILENCE: (&[u8], &[u8]) = (
    &[0x33, 0xC0, 0xB9, 0x54, 0x00, 0xF3, 0xAB],
    &[0xB8, 0x80, 0x80, 0xB1, 0x54, 0xF3, 0xAB],
);

/// Prehistorik 2's driver (PRE2.SQZ as CDRUN.COM unpacks it), by offset
/// in its code segment: the game's `CS`, whose IRQ handler is at 204Dh.
const PREHISTORIK_2_SB_MIX: CodePatch = CodePatch {
    checks: &[
        // The end of the volume table's loops: jne / inc cl / cmp cl,41h /
        // jne.
        Bytes {
            at: 0x1EA8,
            bytes: &[0x75, 0xE8, 0xFE, 0xC1, 0x80, 0xF9, 0x41, 0x75, 0xDF],
        },
        Bytes {
            at: 0x21E6,
            bytes: MIX_LOOP,
        },
        // The IRQ handler's start: push ax, bx, cx, dx.
        Bytes {
            at: 0x204D,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    irq_handler: Some(0x204D),
    patches: &[
        // The volume table: from
        //   xor ch,ch / mov al,cl / mul ch / shr ax,1 (6 times) / stosb /
        //   inc ch / cmp ch,40h
        // to
        //   mov ch,-20h / mov al,cl / imul ch / sar ax,1 (6 times) /
        //   stosb / inc ch / cmp ch,20h
        (
            Bytes {
                at: 0x1E90,
                bytes: &[
                    0x32, 0xED, 0x8A, 0xC1, 0xF6, 0xE5, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1,
                    0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xAA, 0xFE, 0xC5, 0x80, 0xFD, 0x40,
                ],
            },
            &[
                0xB5, 0xE0, 0x8A, 0xC1, 0xF6, 0xED, 0xD1, 0xF8, 0xD1, 0xF8, 0xD1, 0xF8, 0xD1, 0xF8,
                0xD1, 0xF8, 0xD1, 0xF8, 0xAA, 0xFE, 0xC5, 0x80, 0xFD, 0x20,
            ],
        ),
        // The sound effects, converted once as they are loaded: from
        // add al,80h / shr al,1 / shr al,1 (0 to 63) to
        // sar al,1 / sar al,1 / add al,80h (96 to 159).
        (
            Bytes {
                at: 0x1F42,
                bytes: &[0x04, 0x80, 0xD0, 0xE8, 0xD0, 0xE8],
            },
            &[0xD0, 0xF8, 0xD0, 0xF8, 0x04, 0x80],
        ),
        // The rest of a block after an effect ends: from
        //   sub ax,0A8h / neg ax / mov cx,ax / xor ax,ax / shr cx,1 /
        //   rep stosw
        // to
        //   mov cx,0A8h / sub cx,ax / shr cx,1 / mov ax,8080h /
        //   rep stosw / nop
        (
            Bytes {
                at: 0x20FB,
                bytes: &[
                    0x2D, 0xA8, 0x00, 0xF7, 0xD8, 0x8B, 0xC8, 0x33, 0xC0, 0xD1, 0xE9, 0xF3, 0xAB,
                ],
            },
            &[
                0xB9, 0xA8, 0x00, 0x2B, 0xC8, 0xD1, 0xE9, 0xB8, 0x80, 0x80, 0xF3, 0xAB, 0x90,
            ],
        ),
        (
            Bytes {
                at: 0x2112,
                bytes: SILENCE.0,
            },
            SILENCE.1,
        ),
        // The ramp down after a sample that doesn't loop: from
        // mov al,[di-1] / or al,al / je to a jump past it (jmp +0Dh).
        (
            Bytes {
                at: 0x222A,
                bytes: &[0x8A, 0x45, 0xFF, 0x0A, 0xC0, 0x74, 0x08],
            },
            &[0xEB, 0x0D, 0x90, 0x90, 0x90, 0x90, 0x90],
        ),
    ],
};

/// Blues Brothers: Jukebox Adventure's driver (BLUES.EXE as PKLITE
/// unpacks it), by offset in its code segment, whose IRQ handler is at
/// 444Bh. The same engine assembled for the 286.
const JUKEBOX_SB_MIX: CodePatch = CodePatch {
    checks: &[
        Bytes {
            at: 0x4289,
            bytes: &[0x75, 0xF1, 0xFE, 0xC1, 0x80, 0xF9, 0x41, 0x75, 0xE8],
        },
        Bytes {
            at: 0x461E,
            bytes: MIX_LOOP,
        },
        Bytes {
            at: 0x444B,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    irq_handler: Some(0x444B),
    patches: &[
        // The volume table: from
        //   xor ch,ch / mov al,cl / mul ch / shr ax,6 / stosb / inc ch /
        //   cmp ch,40h
        // to
        //   mov ch,-20h / mov al,cl / imul ch / sar ax,6 / stosb / inc ch /
        //   cmp ch,20h
        (
            Bytes {
                at: 0x427A,
                bytes: &[
                    0x32, 0xED, 0x8A, 0xC1, 0xF6, 0xE5, 0xC1, 0xE8, 0x06, 0xAA, 0xFE, 0xC5, 0x80,
                    0xFD, 0x40,
                ],
            },
            &[
                0xB5, 0xE0, 0x8A, 0xC1, 0xF6, 0xED, 0xC1, 0xF8, 0x06, 0xAA, 0xFE, 0xC5, 0x80, 0xFD,
                0x20,
            ],
        ),
        // The sound effects: from add al,80h / shr al,2 to
        // sar al,2 / add al,80h.
        (
            Bytes {
                at: 0x4335,
                bytes: &[0x04, 0x80, 0xC0, 0xE8, 0x02],
            },
            &[0xC0, 0xF8, 0x02, 0x04, 0x80],
        ),
        // The rest of a block after an effect ends, which ramped down from
        // the effect's last byte to 0: from mov al,es:[di-1] / or al,al /
        // je to mov al,80h and a jump to its rep stosb (jmp +0Dh).
        (
            Bytes {
                at: 0x4501,
                bytes: &[0x26, 0x8A, 0x45, 0xFF, 0x0A, 0xC0, 0x74, 0x09],
            },
            &[0xB0, 0x80, 0xEB, 0x0D, 0x90, 0x90, 0x90, 0x90],
        ),
        (
            Bytes {
                at: 0x4552,
                bytes: SILENCE.0,
            },
            SILENCE.1,
        ),
        // A sample that doesn't loop ends: the value the channel adds for
        // the rest of the block, and holds until its next note, from
        // mov al,[di-1] / xlat / shr al,1 to xor al,al.
        (
            Bytes {
                at: 0x4669,
                bytes: &[0x8A, 0x45, 0xFF, 0xD7, 0xD0, 0xE8],
            },
            &[0x32, 0xC0, 0x90, 0x90, 0x90, 0x90],
        ),
    ],
};

/// Jukebox Adventure's MOD player, in the same code segment: before its
/// handler for effect Bxx (position jump) it has `cmp bx,0Bh` and a `je`
/// back to that `cmp`, a loop that never ends, in the IRQ handler with
/// interrupts off. The title song has a B06 at its last position, so the
/// game hangs about 75 s after the title screen comes up. Prehistorik 2's
/// player, the same engine, has no such check. Found in BLUES.EXE of
/// 25096 bytes, MD5 4e4e60dbc63bdd867f7ff65725b7e8b4 (TOSEC's [h1]
/// release, the only one to hand), which is Razor 1911's crack. The loop
/// comes out of the PKLITE unpacker as it is, in the middle of a
/// function, where a crack couldn't have put five bytes in, so it is
/// most likely the game's own. A fix of its own, so it can go if an
/// uncracked copy turns out not to have it.
///
/// Taking out the `je` doesn't depend on anything the driver set up, so
/// it goes in whenever the code is found, a set-up driver too.
const JUKEBOX_MOD_HANG: CodePatch = CodePatch {
    checks: &[
        // mov ax,[di+393h] / mov bl,ah / xor bh,bh: the effect's number.
        Bytes {
            at: 0x47AF,
            bytes: &[0x8B, 0x85, 0x93, 0x03, 0x8A, 0xDC, 0x32, 0xFF],
        },
        // shl bx,1 / jmp [bx+355h]: the handlers' table.
        Bytes {
            at: 0x47BC,
            bytes: &[0xD1, 0xE3, 0xFF, 0xA7, 0x55, 0x03],
        },
        Bytes {
            at: 0x444B,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    irq_handler: None,
    // cmp bx,0Bh / je -5 (to the cmp) loses the je.
    patches: &[(
        Bytes {
            at: 0x47B7,
            bytes: &[0x83, 0xFB, 0x0B, 0x74, 0xFB],
        },
        &[0x83, 0xFB, 0x0B, 0x90, 0x90],
    )],
};

/// The DOS/4GW settings that Lands of Lore 2's launchers add to the
/// environment they start the game (LOLG.DAT, with DOS/4GW bound in)
/// with: LOLG.EXE, which starts the game, and SETUPG.EXE, which runs the
/// setup program (_SETUP.EXE) and then the game. DOS/4GW is its own DPMI
/// host for the game (rust-dos's host passes over a program that loads
/// Glide's overlay, and DOSBox and plain DOS have none), and
/// `VirtualSize:24M` makes its virtual memory manager give the program 24
/// MB of memory in all, whatever the machine has. The game takes up to 16
/// MB of that into its heap at its first allocation, and its C library's
/// malloc takes more from DOS/4GW (DPMI 0501h) whenever the heap runs
/// out. In Huline's cantina, after Baccata's first words, the game asks
/// for 1309694 bytes with about 23.6 MB in use, DOS/4GW refuses the block
/// with error 8016h, and the game stops with "Memory allocation of
/// 1309694 bytes failed!!". GOG's copy in DOSBox stops the same way, and
/// so would a real PC, which runs the same launchers.
///
/// The patch makes it `VirtualSize:32M`, of the same length. 32 MB is
/// what 16 MB of memory (`memsize`'s default) and the 20 MB swap file the
/// same string asks for (`SwapMin:20M`) cover, so DOS/4GW can back it on
/// a machine of the size the game asked for. It never gives the game less
/// memory than 24 MB did. Both launchers append the string with the same
/// loop (mov si,offset / lodsb / stosb / test al,al / jne / movsb), from
/// their own offsets. For version 1.30, GOG's copy: LOLG.EXE of 2847
/// bytes, MD5 1036beaf32bf4d25de4109d4a245d1a0, and SETUPG.EXE of 2262
/// bytes, MD5 6f4b513ea0dcd4af601446bfab178b38.
const LOL2_LAUNCHER: CodePatch = CodePatch {
    checks: &[
        // The string around the patch.
        Bytes {
            at: 0x481,
            bytes: b"DOS4GVM=VirtualMemory:ON,VirtualSize:",
        },
        Bytes {
            at: 0x4A8,
            bytes: b"M,SwapFileName:lolgswap.swp,SwapMin:20M,SwapInc:0\0",
        },
        // The loop that appends it, from DS:0071h.
        Bytes {
            at: 0x3DA,
            bytes: &[0xBE, 0x71, 0x00, 0xAC, 0xAA, 0x84, 0xC0, 0x75, 0xFA, 0xA4],
        },
    ],
    irq_handler: None,
    patches: &[(
        Bytes {
            at: 0x4A6,
            bytes: b"24",
        },
        b"32",
    )],
};

/// SETUPG.EXE's copy of the settings `LOL2_LAUNCHER` patches.
const LOL2_SETUP_LAUNCHER: CodePatch = CodePatch {
    checks: &[
        Bytes {
            at: 0x441,
            bytes: b"DOS4GVM=VirtualMemory:ON,VirtualSize:",
        },
        Bytes {
            at: 0x468,
            bytes: b"M,SwapFileName:lolgswap.swp,SwapMin:20M,SwapInc:0\0",
        },
        // The loop that appends it, from DS:0061h.
        Bytes {
            at: 0x3DD,
            bytes: &[0xBE, 0x61, 0x00, 0xAC, 0xAA, 0x84, 0xC0, 0x75, 0xFA, 0xA4],
        },
    ],
    irq_handler: None,
    patches: &[(
        Bytes {
            at: 0x466,
            bytes: b"24",
        },
        b"32",
    )],
};

/// RenderWare 3.10's BMP reader in Zanzarah's ZANTHP.EXE allocates the
/// buffer for one row of a BMP of more than 8 bits a pixel as
/// `((width + 7) & ~7) * 3` bytes, enough for 24 bits, and reads each row
/// into it as `((bits * width + 7) / 8 + 3) & ~3` bytes. A 32-bit BMP
/// overruns it by a quarter of a row. The game has two, the 256x256 logo
/// texture of its main menu and its mask (TEXTURES\MISC\LOG000T.BMP and
/// LOG000M.BMP in DATA_0.PAK), read when the title screen's Next is
/// clicked: each row runs 0xF4 bytes past the 0x310-byte block the C
/// runtime's small-block heap gives for 0x300, and the last row read has
/// zeros there. That zeroes the header of whatever follows, which the
/// heap then takes for a free block with null links, and the buffer's
/// `free` writes to address 4.
///
/// The game reads the same textures as DDS files instead when the 3D
/// card has a DXTn texture format: it installs RenderWare's DDS reader
/// only if IDirect3DDevice7::EnumTextureFormats lists one. On a card with
/// DXTn (a GeForce or a Radeon) the BMP reader never sees these files. A
/// Voodoo3, which rust-dos emulates and whose Direct3D driver lists only
/// 16-bit and 8-bit palettized formats, sends them through it, and so
/// would a real one: the overrun and the zeros come from the reader's
/// code and the file, whatever the heap's layout.
///
/// The patch makes the buffer `((width + 7) & ~7) * 5` bytes, of the same
/// length (lea ebp,[ebx+ebx*2] to lea ebp,[ebx+ebx*4]), which holds a row
/// of up to 40 bits a pixel. Rows of 16 and 24 bits read as before; the
/// buffer is freed after the image is read. For the Russian release by
/// Russobit-M, version 1.008: ZANTHP.EXE of 2170880 bytes, MD5
/// f3f68fe9bbddde746fd53d47e9483ca5, whose file offsets are its
/// addresses less 0x400000.
const ZANZARAH_BMP_ROWS: CodePatch = CodePatch {
    checks: &[
        // mov eax,[esp+44Ch] (bits) / mov edx,[esp+460h] (width) /
        // mov ecx,[607134h] / mov ebx,7 / add ebx,edx / imul eax,edx /
        // and ebx,-8 / lea edx,[eax+7] / sar edx,2
        Bytes {
            at: 0x1345D1,
            bytes: &[
                0x8B, 0x84, 0x24, 0x4C, 0x04, 0x00, 0x00, 0x8B, 0x94, 0x24, 0x60, 0x04, 0x00, 0x00,
                0x8B, 0x0D, 0x34, 0x71, 0x60, 0x00, 0xBB, 0x07, 0x00, 0x00, 0x00, 0x03, 0xDA, 0x0F,
                0xAF, 0xC2, 0x83, 0xE3, 0xF8, 0x8D, 0x50, 0x07, 0xC1, 0xFA, 0x02,
            ],
        },
        // shr edx,1Dh / mov [esp],ebp (the size to allocate) / the row's
        // size: lea edx,[edx+eax+7] / sar edx,3 / add edx,3 / and edx,-4 /
        // mov [esp+454h],edx / call [ecx+130h] (RenderWare's malloc)
        Bytes {
            at: 0x1345FB,
            bytes: &[
                0xC1, 0xEA, 0x1D, 0x89, 0x2C, 0x24, 0x8D, 0x54, 0x02, 0x07, 0xC1, 0xFA, 0x03, 0x83,
                0xC2, 0x03, 0x83, 0xE2, 0xFC, 0x89, 0x94, 0x24, 0x54, 0x04, 0x00, 0x00, 0xFF, 0x91,
                0x30, 0x01, 0x00, 0x00,
            ],
        },
    ],
    irq_handler: None,
    patches: &[(
        Bytes {
            at: 0x1345F8,
            bytes: &[0x8D, 0x2C, 0x5B],
        },
        &[0x8D, 0x2C, 0x9B],
    )],
};

/// The fixes rust-dos knows.
pub const FIXES: &[Fix] = &[
    Fix {
        name: "prehistorik2-sb-mix",
        game: "Prehistorik 2",
        reason: "its Sound Blaster mix clicks and sits off centre",
        kind: Kind::Code(&PREHISTORIK_2_SB_MIX),
    },
    Fix {
        name: "jukebox-sb-mix",
        game: "Blues Brothers: Jukebox Adventure",
        reason: "its Sound Blaster mix clicks and sits off centre",
        kind: Kind::Code(&JUKEBOX_SB_MIX),
    },
    Fix {
        name: "jukebox-mod-hang",
        game: "Blues Brothers: Jukebox Adventure",
        reason: "its music player hangs on effect Bxx",
        kind: Kind::Code(&JUKEBOX_MOD_HANG),
    },
    // MicroProse's games (F-117, F-19, Gunship, Covert Action) ship two
    // helpers that can't work without the original install disks:
    //
    // - DSWAP.EXE verifies the install disk with INT 13h sector reads.
    //   Without a real disk image the check fails and hangs on "insert
    //   disk". F117.COM tests `AL != 0` after it, so it gets 1.
    // - PLAYER.EXE animates the title and cutscene screens. Its stream
    //   decoder checks a buffer the copy protection's path should have
    //   filled; when the data is zero it loops forever looking for its
    //   terminator word. The parent doesn't look at its exit code.
    //
    // Both are non-essential for play. PLAYER.EXE is a common name
    // (Millennia's intro player has it too): only the one with DSWAP.EXE
    // beside it is MicroProse's.
    Fix {
        name: "microprose-dswap",
        game: "MicroProse's games",
        reason: "DSWAP.EXE wants the original install disk",
        kind: Kind::SkipAtExec {
            program: "DSWAP.EXE",
            beside: None,
            exit_code: 1,
        },
    },
    Fix {
        name: "lol2-virtual-size",
        game: "Lands of Lore: Guardians of Destiny",
        reason: "its launchers give DOS/4GW too little memory for Huline's cantina",
        kind: Kind::AtLoad {
            programs: &[
                ("LOLG.EXE", &LOL2_LAUNCHER),
                ("SETUPG.EXE", &LOL2_SETUP_LAUNCHER),
            ],
            beside: Some("LOLG.DAT"),
        },
    },
    Fix {
        name: "zanzarah-bmp-rows",
        game: "Zanzarah: The Hidden Portal",
        reason: "its BMP reader overruns its row buffer on 32-bit images",
        kind: Kind::AtLoad {
            programs: &[("ZANTHP.EXE", &ZANZARAH_BMP_ROWS)],
            beside: None,
        },
    },
    Fix {
        name: "microprose-player",
        game: "MicroProse's games",
        reason: "PLAYER.EXE loops without the copy protection's data",
        kind: Kind::SkipAtExec {
            program: "PLAYER.EXE",
            beside: Some("DSWAP.EXE"),
            exit_code: 0,
        },
    },
];

/// The fix called `name`.
pub fn by_name(name: &str) -> Option<&'static Fix> {
    FIXES.iter().find(|fix| fix.name == name)
}

/// Whether `fix` applies, with `game_fixes` at `on` and `off` the names
/// `game_fixes_off` leaves out.
pub fn enabled(fix: &Fix, on: bool, off: &[String]) -> bool {
    on && !off.iter().any(|name| name == fix.name)
}

/// Whether `ram` holds `b` at `base`.
fn has(ram: &[u8], base: usize, b: &Bytes) -> bool {
    ram.get(base + b.at..base + b.at + b.bytes.len()) == Some(b.bytes)
}

/// Whether `ram` holds every piece of `code` at `base`, its checks and
/// the bytes its patches replace, as the game shipped them.
fn matches(ram: &[u8], base: usize, code: &CodePatch) -> bool {
    code.checks.iter().all(|b| has(ram, base, b))
        && code.patches.iter().all(|(old, _)| has(ram, base, old))
}

/// The code fixes that `enabled` lets through whose code is in `ram`
/// (conventional memory), unpatched and, where it matters, not yet set
/// up: each with its code and the linear address of its code segment,
/// once for every copy.
pub fn find_code(
    ram: &[u8],
    enabled: impl Fn(&Fix) -> bool,
) -> Vec<(&'static Fix, &'static CodePatch, usize)> {
    // A searcher for each code fix's first piece, built once.
    static FINDERS: OnceLock<Vec<Option<Finder<'static>>>> = OnceLock::new();
    let finders = FINDERS.get_or_init(|| {
        FIXES
            .iter()
            .map(|fix| match fix.kind {
                Kind::Code(code) => Some(Finder::new(code.patches[0].0.bytes)),
                Kind::SkipAtExec { .. } | Kind::AtLoad { .. } => None,
            })
            .collect()
    });
    let mut found = Vec::new();
    for (fix, finder) in FIXES.iter().zip(finders) {
        let (Kind::Code(code), Some(finder)) = (&fix.kind, finder) else {
            continue;
        };
        if !enabled(fix) {
            continue;
        }
        let first = &code.patches[0].0;
        let Some(after) = ram.get(first.at..) else {
            continue;
        };
        // A match's index in what follows `first.at` is the base it would
        // be at. Every copy, so that one left in a buffer below the code
        // that runs doesn't take its place.
        let bases = finder.find_iter(after).filter(|&base| {
            matches(ram, base, code) && !code.irq_handler.is_some_and(|at| set_up(ram, base + at))
        });
        found.extend(bases.map(|base| (fix, *code, base)));
    }
    found
}

/// Whether a vector in the interrupt table of `ram` points at the linear
/// address `handler`.
fn set_up(ram: &[u8], handler: usize) -> bool {
    ram.get(..0x400).is_some_and(|table| {
        table.as_chunks::<4>().0.iter().any(|&[o0, o1, s0, s1]| {
            let offset = u16::from_le_bytes([o0, o1]) as usize;
            let segment = u16::from_le_bytes([s0, s1]) as usize;
            (segment << 4) + offset == handler
        })
    })
}

/// The bytes to write for `code` at `base`: (linear address, byte).
pub fn writes(code: &CodePatch, base: usize) -> impl Iterator<Item = (usize, u8)> + '_ {
    code.patches.iter().flat_map(move |(old, new)| {
        debug_assert_eq!(old.bytes.len(), new.len());
        new.iter()
            .enumerate()
            .map(move |(i, &byte)| (base + old.at + i, byte))
    })
}

/// The fix, and the exit code it gives, for an EXEC (AL=00h) of `path`:
/// `is_file` says whether a path names a file, `enabled` whether a fix
/// applies.
pub fn skip_at_exec(
    path: &str,
    is_file: impl Fn(&str) -> bool,
    enabled: impl Fn(&Fix) -> bool,
) -> Option<(&'static Fix, u8)> {
    FIXES.iter().find_map(|fix| match fix.kind {
        Kind::SkipAtExec {
            program,
            beside,
            exit_code,
        } if is_program(path, program, beside, &is_file) && enabled(fix) => Some((fix, exit_code)),
        _ => None,
    })
}

/// The fix for the program file `bytes` that DOS loads from `path`, and
/// the file as the fix patches it: `is_file` says whether a path names a
/// file, `enabled` whether a fix applies. None when no fix is for the
/// program, or its file isn't as the game shipped it.
pub fn patch_at_load(
    path: &str,
    bytes: &[u8],
    is_file: impl Fn(&str) -> bool,
    enabled: impl Fn(&Fix) -> bool,
) -> Option<(&'static Fix, Vec<u8>)> {
    FIXES.iter().filter(|fix| enabled(fix)).find_map(|fix| {
        let Kind::AtLoad { programs, beside } = fix.kind else {
            return None;
        };
        let (_, code) = programs.iter().find(|(program, code)| {
            is_program(path, program, beside, &is_file) && matches(bytes, 0, code)
        })?;
        let mut patched = bytes.to_vec();
        for (at, byte) in writes(code, 0) {
            patched[at] = byte;
        }
        Some((fix, patched))
    })
}

/// Whether `path` names the file `program` and, with `beside`, a file of
/// that name is in the same directory: `is_file` says whether a path
/// names a file.
fn is_program(
    path: &str,
    program: &str,
    beside: Option<&str>,
    is_file: &impl Fn(&str) -> bool,
) -> bool {
    let upper = path.to_ascii_uppercase();
    let name = upper.rsplit(&['\\', '/', ':'][..]).next().unwrap_or("");
    let dir = &path[..path.len() - name.len()];
    name == program && beside.is_none_or(|other| is_file(&format!("{dir}{other}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code fixes, each with its code.
    fn codes() -> impl Iterator<Item = (&'static Fix, &'static CodePatch)> {
        FIXES.iter().filter_map(|fix| match fix.kind {
            Kind::Code(code) => Some((fix, code)),
            Kind::SkipAtExec { .. } | Kind::AtLoad { .. } => None,
        })
    }

    /// Every fix that patches bytes, in memory or in a program's file,
    /// each with its code.
    fn all_codes() -> impl Iterator<Item = (&'static Fix, &'static CodePatch)> {
        FIXES.iter().flat_map(|fix| match fix.kind {
            Kind::Code(code) => vec![(fix, code)],
            Kind::AtLoad { programs, .. } => {
                programs.iter().map(|&(_, code)| (fix, code)).collect()
            }
            Kind::SkipAtExec { .. } => vec![],
        })
    }

    /// The code's pieces, checks and patches' old bytes.
    fn pieces(code: &CodePatch) -> impl Iterator<Item = &Bytes> {
        code.checks
            .iter()
            .chain(code.patches.iter().map(|(old, _)| old))
    }

    /// `len` zeros with a copy of the code's pieces at `base`.
    fn placed(code: &CodePatch, base: usize, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        for b in pieces(code) {
            bytes[base + b.at..base + b.at + b.bytes.len()].copy_from_slice(b.bytes);
        }
        bytes
    }

    /// A copy of the code's pieces at `base` in 640 KB of zeros.
    fn memory(code: &CodePatch, base: usize) -> Vec<u8> {
        placed(code, base, 0xA0000)
    }

    fn names(found: &[(&Fix, &CodePatch, usize)]) -> Vec<(&'static str, usize)> {
        found
            .iter()
            .map(|(fix, _, base)| (fix.name, *base))
            .collect()
    }

    #[test]
    fn names_are_unique() {
        for (i, fix) in FIXES.iter().enumerate() {
            assert!(
                FIXES[..i].iter().all(|other| other.name != fix.name),
                "{}",
                fix.name
            );
            assert_eq!(by_name(fix.name).map(|f| f.game), Some(fix.game));
        }
    }

    #[test]
    fn finds_each_code_wherever_it_is_loaded() {
        for (fix, code) in codes() {
            for base in [0x1_2340, 0x8_0000] {
                let ram = memory(code, base);
                assert_eq!(names(&find_code(&ram, |_| true)), [(fix.name, base)]);
            }
        }
    }

    #[test]
    fn leaves_code_that_differs_in_any_piece() {
        let base = 0x2_0000;
        for (fix, code) in codes() {
            for b in pieces(code) {
                let mut ram = memory(code, base);
                ram[base + b.at + b.bytes.len() - 1] ^= 1;
                assert!(
                    find_code(&ram, |_| true).is_empty(),
                    "{} found with {:04X} changed",
                    fix.name,
                    b.at
                );
            }
        }
        assert!(find_code(&vec![0u8; 0xA0000], |_| true).is_empty());
    }

    #[test]
    fn leaves_code_that_is_already_set_up() {
        let base = 0x5_DA00;
        for (fix, code) in codes() {
            let mut ram = memory(code, base);
            // IRQ 7's vector (int 0Fh) at the handler, as 5DA0:offset.
            let vector = 0x0F * 4;
            let offset = code.irq_handler.unwrap_or(0x444B) as u16;
            ram[vector..vector + 2].copy_from_slice(&offset.to_le_bytes());
            ram[vector + 2..vector + 4].copy_from_slice(&0x5DA0u16.to_le_bytes());
            // A patch that doesn't depend on the set-up still goes in.
            let want = if code.irq_handler.is_some() {
                vec![]
            } else {
                vec![(fix.name, base)]
            };
            assert_eq!(names(&find_code(&ram, |_| true)), want, "{}", fix.name);
        }
    }

    #[test]
    fn finds_every_copy() {
        for (fix, code) in codes() {
            let mut ram = memory(code, 0x1_0000);
            let other = memory(code, 0x4_0000);
            ram[0x4_0000..].copy_from_slice(&other[0x4_0000..]);
            assert_eq!(
                names(&find_code(&ram, |_| true)),
                [(fix.name, 0x1_0000), (fix.name, 0x4_0000)]
            );
        }
    }

    #[test]
    fn leaves_fixes_that_are_turned_off() {
        let base = 0x3_0000;
        for (fix, code) in codes() {
            let ram = memory(code, base);
            assert!(find_code(&ram, |_| false).is_empty());
            let off = vec![fix.name.to_string()];
            assert!(find_code(&ram, |f| enabled(f, true, &off)).is_empty());
            let others = vec!["no-such-fix".to_string()];
            assert_eq!(find_code(&ram, |f| enabled(f, true, &others)).len(), 1);
        }
    }

    #[test]
    fn patches_once() {
        let base = 0x3_0000;
        for (fix, code) in codes() {
            let mut ram = memory(code, base);
            let found = find_code(&ram, |_| true);
            let &[(_, found_code, at)] = found.as_slice() else {
                panic!("{}: {:?}", fix.name, names(&found));
            };
            for (addr, byte) in writes(found_code, at).collect::<Vec<_>>() {
                ram[addr] = byte;
            }
            for (old, new) in code.patches {
                assert_eq!(&ram[base + old.at..base + old.at + new.len()], *new);
            }
            assert!(
                find_code(&ram, |_| true).is_empty(),
                "{} is found again once patched",
                fix.name
            );
        }
    }

    #[test]
    fn patches_keep_their_lengths_and_stay_apart() {
        for (fix, code) in all_codes() {
            let mut all: Vec<_> = pieces(code).collect();
            all.sort_by_key(|b| b.at);
            for pair in all.windows(2) {
                assert!(
                    pair[0].at + pair[0].bytes.len() <= pair[1].at,
                    "{} overlaps at {:04X}",
                    fix.name,
                    pair[1].at
                );
            }
            for (old, new) in code.patches {
                assert_eq!(old.bytes.len(), new.len(), "{} at {:04X}", fix.name, old.at);
            }
        }
    }

    #[test]
    fn skips_microproses_helpers_at_exec() {
        let only = |files: &'static [&'static str]| move |path: &str| files.contains(&path);
        let skip =
            |path, files| skip_at_exec(path, only(files), |_| true).map(|(f, code)| (f.name, code));
        assert_eq!(
            skip("C:\\F117\\DSWAP.EXE", &[]),
            Some(("microprose-dswap", 1))
        );
        assert_eq!(skip("dswap.exe", &[]), Some(("microprose-dswap", 1)));
        assert_eq!(
            skip("C:\\F117\\player.exe", &["C:\\F117\\DSWAP.EXE"]),
            Some(("microprose-player", 0))
        );
        // Millennia's PLAYER.EXE has no DSWAP.EXE beside it.
        assert_eq!(skip("C:\\MILL\\PLAYER.EXE", &["C:\\F117\\DSWAP.EXE"]), None);
        assert_eq!(skip("C:\\F117\\F117.COM", &["C:\\F117\\DSWAP.EXE"]), None);
        let off = vec!["microprose-dswap".to_string()];
        assert!(skip_at_exec("DSWAP.EXE", |_| false, |f| enabled(f, true, &off)).is_none());
        assert!(skip_at_exec("DSWAP.EXE", |_| false, |f| enabled(f, false, &[])).is_none());
    }

    /// A program file of zeros, a little longer than the code's last
    /// piece, with the code's pieces in it.
    fn file(code: &CodePatch) -> Vec<u8> {
        let end = pieces(code).map(|b| b.at + b.bytes.len()).max().unwrap();
        placed(code, 0, end + 0x40)
    }

    #[test]
    fn patches_a_program_as_it_loads() {
        // The fixes that patch a program as it loads, each with its
        // program, the file that has to be beside it and its code.
        let at_load: Vec<_> = FIXES
            .iter()
            .flat_map(|fix| match fix.kind {
                Kind::AtLoad { programs, beside } => programs
                    .iter()
                    .map(|&(program, code)| (fix, program, beside, code))
                    .collect(),
                _ => vec![],
            })
            .collect();
        assert!(!at_load.is_empty());
        for (fix, program, beside, code) in at_load {
            let bytes = file(code);
            let dir = "C:\\GAME\\";
            let path = format!("{dir}{}", program.to_ascii_lowercase());
            let other = beside.map(|beside| format!("{dir}{beside}"));
            let is_file = |p: &str| other.as_ref().is_some_and(|o| p.eq_ignore_ascii_case(o));
            let (found, patched) = patch_at_load(&path, &bytes, is_file, |_| true).unwrap();
            assert_eq!(found.name, fix.name);
            assert_eq!(patched.len(), bytes.len());
            for (old, new) in code.patches {
                assert_eq!(&patched[old.at..old.at + new.len()], *new, "{}", fix.name);
            }
            // Patched once: the result isn't the file as the game shipped it.
            assert!(patch_at_load(&path, &patched, is_file, |_| true).is_none());
            // Not without the file beside it, with the fix off, or under
            // another name.
            if beside.is_some() {
                assert!(patch_at_load(&path, &bytes, |_| false, |_| true).is_none());
            }
            let off = vec![fix.name.to_string()];
            assert!(patch_at_load(&path, &bytes, is_file, |f| enabled(f, true, &off)).is_none());
            let renamed = format!("{dir}OTHER.EXE");
            assert!(patch_at_load(&renamed, &bytes, is_file, |_| true).is_none());
            // Not when any piece differs, or the file is too short for it.
            for b in pieces(code) {
                let mut changed = bytes.clone();
                changed[b.at] ^= 1;
                assert!(patch_at_load(&path, &changed, is_file, |_| true).is_none());
                let short = &bytes[..b.at + b.bytes.len() - 1];
                assert!(patch_at_load(&path, short, is_file, |_| true).is_none());
            }
        }
    }

    #[test]
    fn lands_of_lore_2_gets_32_mb_of_virtual_memory() {
        let Kind::AtLoad { programs, .. } = by_name("lol2-virtual-size").unwrap().kind else {
            panic!("lol2-virtual-size patches the launchers as they load");
        };
        let is_file = |p: &str| p == "LOLG.DAT";
        // Each launcher, with where its string starts in the file.
        for (program, at) in [("LOLG.EXE", 0x481), ("SETUPG.EXE", 0x441)] {
            let (_, code) = programs.iter().find(|(name, _)| *name == program).unwrap();
            let bytes = file(code);
            let (_, patched) = patch_at_load(program, &bytes, is_file, |_| true).unwrap();
            assert_eq!(
                &patched[at..at + 89],
                b"DOS4GVM=VirtualMemory:ON,VirtualSize:32M,SwapFileName:lolgswap.swp,SwapMin:20M,SwapInc:0\0",
                "{program}"
            );
            // Each launcher's code is its own.
            let other = if program == "LOLG.EXE" {
                "SETUPG.EXE"
            } else {
                "LOLG.EXE"
            };
            assert!(patch_at_load(other, &bytes, is_file, |_| true).is_none());
        }
    }
}
