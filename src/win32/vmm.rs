//! VMM's code: the part of VMM's locked code object (system.rs) that
//! holds the level 0 routines of the VMM services the built-in Windows
//! runs as guest code, VMM's service table, the scheduler's ready queue,
//! the VM control block and the messages of VMM's debugger interface.
//!
//! The built-in Windows has one VM, which runs the program. Its control
//! block (VMM.INC's `cb_s`) has the VM's status (`CB_VM_Status`, 00h),
//! where its V86 memory is mapped (`CB_High_Linear`, 04h: at 0, the first
//! megabyte being mapped one to one) and its client register structure
//! (`CB_Client_Pointer`, 08h). The structure is at the top of the level 0
//! stack, as on Windows 9x: an interrupt or exception from level 3 pushes
//! its frame there, which fills Client_EIP to Client_SS (and Client_Error
//! for an exception with an error code). So a service that changes the
//! client's state changes what the level 0 code's IRETD returns to. The
//! general registers' slots below the frame are the level 0 code's own
//! stack: the built-in Windows runs no handler of its own before a
//! program's level 0 code to save them there.
//!
//! The VM's status is that of a 32-bit protected-mode application
//! (`VMStat_PM_Exec`, `VMStat_PM_App`, `VMStat_PM_Use32`), the Win32
//! program. Level 0 code may change it, as VxDs do.
//!
//! The routines follow the documented interface of each service (the
//! Windows 95 DDK's VMM services reference). Their code is rust-dos's
//! own, apart from the 44 bytes of Simulate_Iret in `compared`, which
//! run as part of it.

pub(super) mod compared;

/// The size of the part of the object VMM's code has. The object ends
/// with it, and programs scan the object for what `compared` holds up to
/// 128 bytes before its end, so the code and data leave at least that
/// much free at the end.
pub const SIZE: u32 = 0x800;

/// The VMM services run as guest code here. Those the built-in Windows
/// runs in Rust (`system::vxd_service`) have a stub here that calls them
/// through INT 20h.
const GET_CUR_VM_HANDLE: u16 = 0x0001;
const ENABLE_VM_INTS: u16 = 0x001A;
const SIMULATE_IRET: u16 = 0x0047;

/// `CB_VM_Status` bits (VMM.INC).
const VMSTAT_PM_EXEC: u32 = 0x20;
const VMSTAT_PM_APP: u32 = 0x40;
const VMSTAT_PM_USE32: u32 = 0x80;
const VMSTAT_VXD_EXEC: u32 = 0x100;

/// The places in the code that other code refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Label {
    GetCurVmHandle,
    EnableVmInts,
    SimulateIret,
    Iret16,
    Iret32,
    IretV86,
    IretFlags,
    IretExit,
    ServiceTable,
    ReadyQueue,
    ReadyHead,
    VmBlock,
    DebuggerWelcome,
    DebuggerPrompt,
    RustService(u16),
}

#[derive(Clone, Copy)]
enum Fixup {
    /// A byte, relative to the end of the instruction it ends.
    Rel8,
    /// A dword, relative to the end of the instruction it ends.
    Rel32,
    /// A dword with the label's address.
    Abs32,
}

/// Code at `base` being put together, with labels resolved at the end.
struct Code {
    base: u32,
    bytes: Vec<u8>,
    labels: Vec<(Label, usize)>,
    fixups: Vec<(usize, Label, Fixup)>,
}

impl Code {
    fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn label(&mut self, label: Label) {
        self.label_at(label, self.bytes.len());
    }

    fn label_at(&mut self, label: Label, at: usize) {
        debug_assert!(self.labels.iter().all(|&(l, _)| l != label));
        self.labels.push((label, at));
    }

    /// `opcode` followed by a reference of `kind` to `label`.
    fn refer(&mut self, opcode: &[u8], label: Label, kind: Fixup) {
        self.emit(opcode);
        self.fixups.push((self.bytes.len(), label, kind));
        let len = match kind {
            Fixup::Rel8 => 1,
            Fixup::Rel32 | Fixup::Abs32 => 4,
        };
        self.bytes.resize(self.bytes.len() + len, 0);
    }

    fn dword(&mut self, value: u32) {
        self.emit(&value.to_le_bytes());
    }

    fn align(&mut self, to: usize) {
        self.bytes.resize(self.bytes.len().next_multiple_of(to), 0);
    }

    fn offset(&self, label: Label) -> usize {
        self.labels
            .iter()
            .find(|&&(l, _)| l == label)
            .map(|&(_, at)| at)
            .unwrap_or_else(|| panic!("VMM's code has no {label:?}"))
    }

    fn address(&self, label: Label) -> u32 {
        self.base + self.offset(label) as u32
    }

    fn finish(mut self) -> Self {
        for &(at, label, kind) in &self.fixups {
            let target = self.offset(label) as i64;
            match kind {
                Fixup::Rel8 => {
                    let rel = i8::try_from(target - (at as i64 + 1)).expect("a short jump");
                    self.bytes[at] = rel as u8;
                }
                Fixup::Rel32 => {
                    let rel = (target - (at as i64 + 4)) as i32;
                    self.bytes[at..at + 4].copy_from_slice(&rel.to_le_bytes());
                }
                Fixup::Abs32 => {
                    let address = self.base + target as u32;
                    self.bytes[at..at + 4].copy_from_slice(&address.to_le_bytes());
                }
            }
        }
        self
    }
}

/// VMM's code and data, made for the address they go at.
pub struct VmmCode {
    pub bytes: Vec<u8>,
    /// The service table's address and its number of entries, for VMM's
    /// DDB.
    pub service_table: u32,
    pub services: u32,
}

/// VMM's code and data, to go at `base` in the system arena, the client
/// register structure being at `client`. `rust_services` are the VMM
/// services the built-in Windows runs in Rust, by their number.
///
/// The service table has an entry for each service up to the last one
/// with an entry point; those of the services the built-in Windows
/// doesn't have are 0.
pub fn build(base: u32, client: u32, rust_services: &[u16]) -> VmmCode {
    let mut c = Code {
        base,
        bytes: Vec::new(),
        labels: Vec::new(),
        fixups: Vec::new(),
    };
    get_cur_vm_handle(&mut c);
    enable_vm_ints(&mut c);
    simulate_iret(&mut c);
    // The services run in Rust: INT 20h with the service's dword, which
    // the built-in Windows answers (`system::vxd_service`), and back.
    for &service in rust_services {
        c.align(4);
        c.label(Label::RustService(service));
        c.emit(&[0xCD, 0x20]); // int 20h
        c.dword(0x0001_0000 | service as u32);
        c.emit(&[0xC3]); // ret
    }
    let services = data(&mut c, client, rust_services);
    let c = c.finish();
    assert!(
        c.bytes.len() as u32 <= SIZE - 128,
        "VMM's code leaves less than 128 bytes at the object's end"
    );
    VmmCode {
        service_table: c.address(Label::ServiceTable),
        services,
        bytes: c.bytes,
    }
}

/// Get_Cur_VM_Handle (0001h): EBX is the current VM's handle, the VM at
/// the head of the ready queue. The other registers and the flags stay
/// as they were.
fn get_cur_vm_handle(c: &mut Code) {
    c.align(4);
    c.label(Label::GetCurVmHandle);
    c.refer(&[0x8B, 0x1D], Label::ReadyHead, Fixup::Abs32); // mov ebx, [head]
    c.emit(&[0xC3]); // ret
}

/// Enable_VM_Ints (001Ah): sets the interrupt flag in the current VM's
/// Client_EFlags. The built-in Windows keeps no callbacks for
/// Call_When_VM_Ints_Enabled and no events waiting on the flag, so that
/// is all there is to do. The registers stay as they were; the flags
/// change.
fn enable_vm_ints(c: &mut Code) {
    c.align(4);
    c.label(Label::EnableVmInts);
    c.emit(&[0x50]); // push eax
    c.refer(&[0xA1], Label::ReadyHead, Fixup::Abs32); // mov eax, [head]
    c.emit(&[0x8B, 0x40, 0x08]); // mov eax, [eax+8]: CB_Client_Pointer
    c.emit(&[0x80, 0x48, 0x2D, 0x02]); // or byte [eax+2Dh], 2: IF
    c.emit(&[0x58, 0xC3]); // pop eax ; ret
}

/// Simulate_Iret (0047h): the current VM goes on as if it had executed an
/// IRET, its CS, EIP, flags and stack pointer taken from the frame at the
/// top of its stack. Which frame depends on the VM's status:
///
/// - V86 mode (`VMStat_PM_Exec` clear): three words at Client_SS * 16 +
///   Client_SP in the VM's memory, SP going on by 6 within its 64K and
///   the upper halves of ESP and EIP kept. The flags are the popped word
///   with IOPL and NT clear and VM set.
/// - A 16-bit protected-mode client: three words at Client_SS:Client_SP,
///   read through the client's selector. Client_EIP is the popped IP and
///   Client_ESP is SP + 6, both zero-extended.
/// - A 32-bit client or VxD nested execution (`VMStat_PM_Use32` or
///   `VMStat_VxD_Exec`): three dwords at Client_SS:Client_ESP, or at
///   Client_SP if the segment's limit is 64K or less, Client_ESP being
///   that offset + 12. CS and the flags are the low words of theirs.
///
/// In protected mode only the low word of Client_EFlags comes from the
/// frame. If that turns the interrupt flag on, the flag is cleared again
/// and Enable_VM_Ints sets it, as an STI by the VM would.
///
/// The client's stack is read with FS loaded with the client's SS. A
/// selector that can't be loaded or an offset past the segment's limit
/// faults at level 0, where Windows makes it the client's fault; the
/// built-in Windows ends the program, as for any fault at level 0.
///
/// All the registers stay as they were; the flags change.
fn simulate_iret(c: &mut Code) {
    c.align(4);
    c.label(Label::SimulateIret);
    c.emit(&[0x55, 0x56, 0x50, 0x51, 0x53]); // push ebp, esi, eax, ecx, ebx
    c.emit(&[0x0F, 0xA0]); // push fs
    c.refer(&[0x8B, 0x1D], Label::ReadyHead, Fixup::Abs32); // mov ebx, [head]
    c.emit(&[0x8B, 0x6B, 0x08]); // mov ebp, [ebx+8]: CB_Client_Pointer
    c.emit(&[0xF7, 0x03]); // test dword [ebx], Use32 | VxD_Exec
    c.dword(VMSTAT_PM_USE32 | VMSTAT_VXD_EXEC);
    c.refer(&[0x0F, 0x85], Label::Iret32, Fixup::Rel32); // jnz
    c.emit(&[0xF6, 0x03, VMSTAT_PM_EXEC as u8]); // test byte [ebx], PM_Exec
    c.refer(&[0x0F, 0x84], Label::IretV86, Fixup::Rel32); // jz

    // A 16-bit protected-mode client. Client_EIP's upper half is
    // cleared here, its lower half written by the compared code.
    c.emit(&[0x8E, 0x65, 0x34]); // mov fs, [ebp+34h]: Client_SS
    c.emit(&[0x66, 0xC7, 0x45, 0x26, 0x00, 0x00]); // mov word [ebp+26h], 0
    c.label(Label::Iret16);
    let start = c.bytes.len();
    // Leaves AX the popped flags with EAX's upper half 0, and CX the
    // flags that changed; jumps to `IretExit` if IF didn't change. Its
    // last byte is the opcode of `test ah, 2` below.
    c.emit(&compared::SIMULATE_IRET_PM16);
    // From the `mov ecx, eax`: the 32-bit case comes here with AX the
    // popped flags.
    c.label_at(Label::IretFlags, start + 0x1E);
    // IF changed: on, if it is set in the popped flags. Then it goes off
    // again and Enable_VM_Ints turns it on. These 17 bytes, from the
    // compared code's last one, reach its jump target, `IretExit`.
    c.emit(&[0xC4, 0x02]); // test ah, 2
    c.refer(&[0x74], Label::IretExit, Fixup::Rel8); // jz
    c.emit(&[0x81, 0x65, 0x2C]); // and dword [ebp+2Ch], ~IF
    c.dword(!0x200);
    c.refer(&[0xE8], Label::EnableVmInts, Fixup::Rel32); // call
    c.label(Label::IretExit);
    assert_eq!(
        c.bytes.len() - start,
        0x3C,
        "the compared code's jump lands on Simulate_Iret's exit"
    );
    c.emit(&[0x0F, 0xA1]); // pop fs
    c.emit(&[0x5B, 0x59, 0x58, 0x5E, 0x5D]); // pop ebx, ecx, eax, esi, ebp
    c.emit(&[0xC3]); // ret

    // V86 mode: the frame in the VM's memory, at CB_High_Linear +
    // SS * 16, each word at SP within the 64K.
    c.label(Label::IretV86);
    c.emit(&[0x0F, 0xB7, 0x45, 0x34]); // movzx eax, word [ebp+34h]: Client_SS
    c.emit(&[0xC1, 0xE0, 0x04]); // shl eax, 4
    c.emit(&[0x03, 0x43, 0x04]); // add eax, [ebx+4]: CB_High_Linear
    c.emit(&[0x0F, 0xB7, 0x75, 0x30]); // movzx esi, word [ebp+30h]: Client_SP
    c.emit(&[0x66, 0x8B, 0x0C, 0x30]); // mov cx, [eax+esi]
    c.emit(&[0x66, 0x89, 0x4D, 0x24]); // mov [ebp+24h], cx: Client_IP
    c.emit(&[0x66, 0x83, 0xC6, 0x02]); // add si, 2
    c.emit(&[0x66, 0x8B, 0x0C, 0x30]); // mov cx, [eax+esi]
    c.emit(&[0x66, 0x89, 0x4D, 0x28]); // mov [ebp+28h], cx: Client_CS
    c.emit(&[0x66, 0x83, 0xC6, 0x02]); // add si, 2
    c.emit(&[0x0F, 0xB7, 0x0C, 0x30]); // movzx ecx, word [eax+esi]
    c.emit(&[0x66, 0x83, 0xC6, 0x02]); // add si, 2
    c.emit(&[0x66, 0x89, 0x75, 0x30]); // mov [ebp+30h], si: Client_SP
    c.emit(&[0x81, 0xE1]); // and ecx, ~(NT | IOPL)
    c.dword(0x8FFF);
    c.emit(&[0x81, 0xC9]); // or ecx, VM
    c.dword(0x2_0000);
    c.emit(&[0x89, 0x4D, 0x2C]); // mov [ebp+2Ch], ecx: Client_EFlags
    c.refer(&[0xE9], Label::IretExit, Fixup::Rel32); // jmp

    // A 32-bit client: the offset is ESP in a segment above 64K, else SP.
    c.label(Label::Iret32);
    c.emit(&[0x8E, 0x65, 0x34]); // mov fs, [ebp+34h]: Client_SS
    c.emit(&[0x8B, 0x75, 0x30]); // mov esi, [ebp+30h]: Client_ESP
    c.emit(&[0x0F, 0x03, 0x45, 0x34]); // lsl eax, [ebp+34h]
    c.emit(&[0x3D]); // cmp eax, 0FFFFh
    c.dword(0xFFFF);
    c.emit(&[0x77, 0x03]); // ja +3
    c.emit(&[0x0F, 0xB7, 0xF6]); // movzx esi, si
    c.emit(&[0x83, 0xC6, 0x0C]); // add esi, 12
    c.emit(&[0x64, 0x8B, 0x46, 0xF4]); // mov eax, fs:[esi-12]: EIP
    c.emit(&[0x89, 0x45, 0x24]); // mov [ebp+24h], eax: Client_EIP
    c.emit(&[0x64, 0x8B, 0x46, 0xF8]); // mov eax, fs:[esi-8]: CS
    c.emit(&[0x66, 0x89, 0x45, 0x28]); // mov [ebp+28h], ax: Client_CS
    c.emit(&[0x89, 0x75, 0x30]); // mov [ebp+30h], esi: Client_ESP
    c.emit(&[0x64, 0x0F, 0xB7, 0x46, 0xFC]); // movzx eax, word fs:[esi-4]: flags
    c.refer(&[0xE9], Label::IretFlags, Fixup::Rel32); // jmp
}

/// VMM's data: the service table, the ready queue, the VM's control block
/// and the debugger interface's messages. The service table's number of
/// entries.
fn data(c: &mut Code, client: u32, rust_services: &[u16]) -> u32 {
    c.align(4);
    c.label(Label::ServiceTable);
    let entries: Vec<(u16, Label)> = [
        (GET_CUR_VM_HANDLE, Label::GetCurVmHandle),
        (ENABLE_VM_INTS, Label::EnableVmInts),
        (SIMULATE_IRET, Label::SimulateIret),
    ]
    .into_iter()
    .chain(rust_services.iter().map(|&s| (s, Label::RustService(s))))
    .collect();
    let services = entries
        .iter()
        .map(|&(s, _)| s as u32 + 1)
        .max()
        .unwrap_or(0);
    for service in 0..services as u16 {
        match entries.iter().find(|&&(s, _)| s == service) {
            Some(&(_, label)) => c.refer(&[], label, Fixup::Abs32),
            None => c.dword(0),
        }
    }

    // The scheduler's ready queue, the VMs ready to run: the one VM, which
    // runs, is its head and the current VM.
    c.label(Label::ReadyQueue);
    c.emit(&compared::READY_QUEUE_SIGNATURE);
    c.label(Label::ReadyHead);
    c.refer(&[], Label::VmBlock, Fixup::Abs32);

    // The VM's control block: CB_VM_Status, CB_High_Linear and
    // CB_Client_Pointer.
    c.label(Label::VmBlock);
    c.dword(VMSTAT_PM_EXEC | VMSTAT_PM_APP | VMSTAT_PM_USE32);
    c.dword(0);
    c.dword(client);

    // The messages of VMM's debugger interface, which a debugger shows:
    // the built-in Windows has no debugger and shows neither. A table of
    // the two, then the strings.
    c.refer(&[], Label::DebuggerWelcome, Fixup::Abs32);
    c.refer(&[], Label::DebuggerPrompt, Fixup::Abs32);
    c.label(Label::DebuggerWelcome);
    c.emit(compared::DEBUGGER_WELCOME);
    c.emit(&[0]);
    c.label(Label::DebuggerPrompt);
    c.emit(compared::DEBUGGER_PROMPT);
    c.emit(&[0]);
    services
}
