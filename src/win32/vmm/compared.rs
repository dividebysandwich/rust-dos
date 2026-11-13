//! Programs of the Windows 98 era
//! read VMM's locked code object, which VMM_GetVxDLocationList reports,
//! and look in it for these, byte for byte.
/// Code of Simulate_Iret's case of a 16-bit protected-mode client: it
/// pops IP, CS and FLAGS from the client's stack (at FS:Client_SP) into
/// the client register structure at EBP, sets Client_ESP past them and
/// jumps 11h bytes on from its last instruction when the interrupt flag
/// didn't change. The last byte, F6h, opens the next instruction, which
/// isn't part of what is compared. The built-in VMM runs these 44 bytes
/// as part of its own Simulate_Iret (`super::simulate_iret`).
pub(in crate::win32) const SIMULATE_IRET_PM16: [u8; 44] = [
    0x0F, 0xB7, 0x75, 0x30, // movzx esi, word [ebp+30h]: Client_SP
    0x83, 0xC6, 0x06, // add esi, 6
    0x64, 0x8B, 0x46, 0xFA, // mov eax, fs:[esi-6]: IP and CS
    0x66, 0x89, 0x45, 0x24, // mov [ebp+24h], ax: Client_IP
    0xC1, 0xE8, 0x10, // shr eax, 16
    0x66, 0x89, 0x45, 0x28, // mov [ebp+28h], ax: Client_CS
    0x66, 0x64, 0x8B, 0x46, 0xFE, // mov ax, fs:[esi-2]: FLAGS
    0x89, 0x75, 0x30, // mov [ebp+30h], esi: Client_ESP
    0x8B, 0xC8, // mov ecx, eax
    0x66, 0x87, 0x4D, 0x2C, // xchg [ebp+2Ch], cx: Client_Flags
    0x33, 0xC8, // xor ecx, eax: the flags that changed
    0xF6, 0xC5, 0x02, // test ch, 2: IF
    0x74, 0x11, // jz +11h
    0xF6, // the next instruction's opcode
];

/// The greeting of VMM's debugger interface, which a debugger shows with
/// the system's name when it connects. Compared without its terminating
/// zero.
pub(in crate::win32) const DEBUGGER_WELCOME: &[u8; 24] = b"Welcome to %s's debugger";

/// The prompt of VMM's debugger interface, with the name of what the
/// debugger stopped in. Compared without its terminating zero.
pub(in crate::win32) const DEBUGGER_PROMPT: &[u8; 20] = b"%s (space for help)?";

/// The signature dword of the scheduler's ready queue, "RDYQ" in memory
/// (51594452h).
pub(in crate::win32) const READY_QUEUE_SIGNATURE: [u8; 4] = *b"RDYQ";
