//! Fixes for particular games, beyond what the real machine does
//! (`game_fixes`). Each is an entry in [`FIXES`]: the game, what goes
//! wrong, how rust-dos recognises it, and what it does about it. Nothing
//! changes for a program that isn't recognised, and `game_fixes = false`
//! or `game_fixes_off = <names>` turns them off.
//!
//! There are two kinds:
//!
//! - [`Kind::Code`]: code bytes in conventional memory, patched in place.
//!   The check runs when a program starts a reset of the Sound Blaster's
//!   DSP (a write of 1 to base + 6), which a sound driver does before it
//!   sets anything up. It looks for each fix's first piece of code, and
//!   patches only a copy where every piece is there as the game shipped
//!   it and whose IRQ handler isn't in the interrupt table yet. A driver
//!   that is already set up, as in a save state from a run without the
//!   fix, has built its tables the old way, and patching the rest could
//!   make things worse; it stays as it is until the game starts again.
//! - [`Kind::SkipAtExec`]: a program by name, which DOS's EXEC doesn't run
//!   but answers as if it had, with an exit code.
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
//! To add a fix: an entry in [`FIXES`] with a name of its own, a test
//! that runs it (`tests/game_fixes_tests.rs`, or `tests/exec_tests.rs`
//! for a program skipped at EXEC), and a line in CONFIGURATION.md's list
//! of game fixes.

use memchr::memmem::Finder;
use std::sync::OnceLock;

/// Bytes at an offset in a program's code segment.
pub struct Bytes {
    pub at: usize,
    pub bytes: &'static [u8],
}

/// Code to recognise and patch, by offset in its code segment.
pub struct CodePatch {
    /// Code that has to be there, unchanged, besides the patches' own.
    pub checks: &'static [Bytes],
    /// Where the driver's IRQ handler starts: once an interrupt vector
    /// points there, the driver is set up and isn't patched.
    pub irq_handler: usize,
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
    irq_handler: 0x204D,
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
    irq_handler: 0x444B,
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
/// release, the only one to hand); a fix of its own, so it can go if a
/// release without the loop turns up.
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
    irq_handler: 0x444B,
    // cmp bx,0Bh / je -5 (to the cmp) loses the je.
    patches: &[(
        Bytes {
            at: 0x47B7,
            bytes: &[0x83, 0xFB, 0x0B, 0x74, 0xFB],
        },
        &[0x83, 0xFB, 0x0B, 0x90, 0x90],
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

/// The code fixes that `enabled` lets through whose code is in `ram`
/// (conventional memory), unpatched and not yet set up: each with its
/// code and the linear address of its code segment.
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
                Kind::SkipAtExec { .. } => None,
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
        // be at.
        let base = finder.find_iter(after).find(|&base| {
            code.checks.iter().all(|b| has(ram, base, b))
                && code.patches.iter().all(|(old, _)| has(ram, base, old))
                && !set_up(ram, base + code.irq_handler)
        });
        if let Some(base) = base {
            found.push((fix, *code, base));
        }
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
    let upper = path.to_ascii_uppercase();
    let name = upper.rsplit(&['\\', '/', ':'][..]).next().unwrap_or("");
    let dir = &path[..path.len() - name.len()];
    FIXES.iter().find_map(|fix| match fix.kind {
        Kind::SkipAtExec {
            program,
            beside,
            exit_code,
        } if name == program
            && beside.is_none_or(|other| is_file(&format!("{dir}{other}")))
            && enabled(fix) =>
        {
            Some((fix, exit_code))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code fixes, each with its code.
    fn codes() -> impl Iterator<Item = (&'static Fix, &'static CodePatch)> {
        FIXES.iter().filter_map(|fix| match fix.kind {
            Kind::Code(code) => Some((fix, code)),
            Kind::SkipAtExec { .. } => None,
        })
    }

    /// The code's pieces, checks and patches' old bytes.
    fn pieces(code: &CodePatch) -> impl Iterator<Item = &Bytes> {
        code.checks
            .iter()
            .chain(code.patches.iter().map(|(old, _)| old))
    }

    /// A copy of the code's pieces at `base` in 640 KB of zeros.
    fn memory(code: &CodePatch, base: usize) -> Vec<u8> {
        let mut ram = vec![0u8; 0xA0000];
        for b in pieces(code) {
            ram[base + b.at..base + b.at + b.bytes.len()].copy_from_slice(b.bytes);
        }
        ram
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
            let offset = code.irq_handler as u16;
            ram[vector..vector + 2].copy_from_slice(&offset.to_le_bytes());
            ram[vector + 2..vector + 4].copy_from_slice(&0x5DA0u16.to_le_bytes());
            assert!(
                find_code(&ram, |_| true).is_empty(),
                "{} found once set up",
                fix.name
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
        for (fix, code) in codes() {
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
}
