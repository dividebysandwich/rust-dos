//! SETUPX: Windows 95's and 98's setup engine, as far as installers'
//! 16-bit DLLs use it to install an INF file: W95INF16.DLL, which the
//! ADVPACK.DLL of Internet Explorer's time thunks to through W95INF32.DLL
//! on Windows 9x, opens the INF with IpOpen, checks its signature with
//! IpGetProfileString, sets the source of the files with CtlSetLddPath,
//! opens the copy queue with VcpOpen, runs GenInstall for the files,
//! closes the queue with VcpClose, which copies them, and runs GenInstall
//! again for the registry and the INI files. IExpress packages, such as
//! the Windows Media Audio runtime and DirectPlay's redistributable, go
//! that way.
//!
//! The INF and what an install section does are win32/inf.rs's. Here are
//! the handles of the open INF files, the directories of the logical disk
//! ids (LDIDs) a program set, and the queue of file operations between
//! VcpOpen and VcpClose. The queue's callback, vcpUICallbackProc, shows
//! no progress: the copies are done by VcpClose without calling it.
//!
//! The ordinals and the signatures are Wine's (dlls/setupx.dll16), and
//! the error numbers its setupx16.h's.

use super::builtin::{Def, rust};
use crate::cpu::Cpu;
use crate::win32::inf::{self, FileOp, Inf, Install};
use crate::win32::registry;

/// The W32 state version that first saves SETUPX's state
/// (`State::setupx`).
pub const SETUPX_SAVED: u16 = 109;

/// The errors (RETERR): the INF file isn't there, the handle is no open
/// INF's, the section or the key isn't in it; a file couldn't be copied,
/// the queue is open already or not open, the LDID has no directory.
const ERR_IP_INF_NOT_FOUND: u32 = 105;
const ERR_IP_INVALID_HINF: u32 = 107;
const ERR_IP_SECT_NOT_FOUND: u32 = 109;
const ERR_IP_PROFILE_NOT_FOUND: u32 = 111;
const ERR_VCP_IOFAIL: u32 = 301;
const ERR_VCP_BUSY: u32 = 310;
const ERR_VCP_LDDFIND: u32 = 319;
const ERR_VCP_NOTOPEN: u32 = 323;

/// VcpClose's flags that carry out the queue: its copies, deletions and
/// renames (VCPFL_COPY, VCPFL_DELETE, VCPFL_RENAME). Without them the
/// queue is thrown away.
const VCPFL_COPY: u32 = 0x02;
const VCPFL_DELETE: u32 = 0x08;
const VCPFL_RENAME: u32 = 0x10;

/// The longest path CtlGetLddPath writes, with its zero.
const MAX_PATH: u32 = 260;

/// What SETUPX keeps between calls.
#[derive(Clone, Debug, Default)]
pub struct Setupx {
    /// The INF files IpOpen opened: each one's handle (HINF), path and
    /// text, read as it was opened.
    pub infs: Vec<(u16, String, Vec<u8>)>,
    /// The directories programs gave LDIDs with CtlSetLddPath.
    pub ldids: Vec<(u16, String)>,
    /// The file operations GenInstall queued since VcpOpen, None while
    /// the queue isn't open.
    pub queue: Option<Vec<FileOp>>,
}

crate::state_fields!(Setupx { infs, ldids, queue });

pub static MODULE: Def = Def {
    name: "SETUPX",
    file: "SETUPX.DLL",
    apis: &[
        rust("IpOpen", 2, "pp", ip_open),
        rust("IpClose", 4, "w", ip_close),
        rust("CtlGetLddPath", 38, "wS", ctl_get_ldd_path),
        rust("suErrorToIds", 61, "ww", su_error_to_ids),
        rust("GenInstall", 101, "wpw", gen_install),
        rust(
            "GenFormStrWithoutPlaceHolders",
            103,
            "Spw",
            gen_form_str_without_place_holders,
        ),
        rust("VcpOpen", 200, "SS", vcp_open),
        rust("VcpClose", 201, "wS", vcp_close),
        rust("IpGetProfileString", 210, "wpppw", ip_get_profile_string),
        rust("vcpUICallbackProc", 213, "Swwll", vcp_ui_callback_proc),
        rust("CtlSetLddPath", 508, "wp", ctl_set_ldd_path),
    ],
    library: true,
};

fn state(cpu: &mut Cpu) -> &mut Setupx {
    &mut cpu.bus.win32.win16.setupx
}

/// The INF file of the handle `hinf`, parsed, and its path.
fn inf_of(cpu: &mut Cpu, hinf: u32) -> Option<(Inf, String)> {
    let infs = &state(cpu).infs;
    let (_, path, text) = infs.iter().find(|(h, _, _)| *h as u32 == hinf)?;
    Some((Inf::parse(text), path.clone()))
}

/// The directories of the LDIDs, for an install: those a program set,
/// the source path from the registry, as Windows' setup left it, and
/// Windows' own.
fn dirs(cpu: &mut Cpu) -> impl Fn(u16) -> Option<String> + use<> {
    let set = state(cpu).ldids.clone();
    let source = registry::string_value(
        cpu,
        registry::HKEY_LOCAL_MACHINE,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Setup",
        "SourcePath",
    );
    move |ldid| {
        set.iter()
            .find(|(l, _)| *l == ldid)
            .map(|(_, path)| path.clone())
            .or_else(|| {
                (ldid == inf::LDID_SRCPATH)
                    .then(|| source.clone())
                    .flatten()
            })
            .or_else(|| inf::standard_dir(ldid))
    }
}

/// Write `text` and its zero at the 16:16 pointer `to`, cut to `size`
/// bytes with the zero.
fn put_string(cpu: &mut Cpu, to: u32, size: u32, text: &str) {
    let at = super::linear(cpu, to);
    if at == 0 || size == 0 {
        return;
    }
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(size as usize - 1);
    bytes.push(0);
    super::poke(cpu, at, &bytes);
}

/// IpOpen(path, &hinf): read the INF file and give it a handle.
fn ip_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[path, out] = args else { unreachable!() };
    let name = super::string(cpu, path);
    let Ok((text, _, _)) = crate::win32::kernel32::read_path(cpu, &name) else {
        return Some(ERR_IP_INF_NOT_FOUND);
    };
    let infs = &mut state(cpu).infs;
    let hinf = (1..=u16::MAX).find(|h| infs.iter().all(|(o, _, _)| o != h))?;
    infs.push((hinf, name, text));
    super::write_16(cpu, out, hinf);
    Some(0)
}

/// IpClose(hinf).
fn ip_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let infs = &mut state(cpu).infs;
    let before = infs.len();
    infs.retain(|(h, _, _)| *h as u32 != args[0]);
    Some(if infs.len() < before {
        0
    } else {
        ERR_IP_INVALID_HINF
    })
}

/// IpGetProfileString(hinf, section, key, buffer, size): the value of
/// the key in the section, its fields with their strings put in, joined
/// by commas.
fn ip_get_profile_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hinf, section, key, buffer, size] = args else {
        unreachable!()
    };
    let Some((file, _)) = inf_of(cpu, hinf) else {
        return Some(ERR_IP_INVALID_HINF);
    };
    let (section, key) = (super::string(cpu, section), super::string(cpu, key));
    if !file.has_section(&section) {
        return Some(ERR_IP_SECT_NOT_FOUND);
    }
    let Some(line) = file.line(&section, &key) else {
        return Some(ERR_IP_PROFILE_NOT_FOUND);
    };
    let dirs = dirs(cpu);
    let fields: Vec<String> = line.fields.iter().map(|f| file.expand(f, &dirs)).collect();
    let value = fields.join(",");
    let mut bytes = crate::dosstr::to_bytes(&value);
    bytes.truncate((size as usize).saturating_sub(1));
    bytes.push(0);
    if size > 0 {
        super::poke(cpu, buffer, &bytes);
    }
    Some(0)
}

/// GenInstall(hinf, section, flags): carry out the parts of the install
/// section the flags (GENINSTALL_DO_*) say. Its file operations go into
/// the queue VcpOpen opened, or without one are done at once.
fn gen_install(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hinf, section, flags] = args else {
        unreachable!()
    };
    let Some((file, path)) = inf_of(cpu, hinf) else {
        return Some(ERR_IP_INVALID_HINF);
    };
    let section = super::string(cpu, section);
    cpu.bus.log_string(&format!(
        "[W16] SETUPX.GenInstall: [{section}] of {path}, flags {flags:X}"
    ));
    let dirs = dirs(cpu);
    let install = Install {
        inf: &file,
        dirs: &dirs,
    };
    let mut files = Vec::new();
    if install
        .run(cpu, &section, flags as u16, &mut files)
        .is_err()
    {
        return Some(ERR_IP_SECT_NOT_FOUND);
    }
    match &mut state(cpu).queue {
        Some(queue) => {
            queue.extend(files);
            Some(0)
        }
        None => Some(match inf::commit(cpu, &files) {
            Ok(()) => 0,
            Err(_) => ERR_VCP_IOFAIL,
        }),
    }
}

/// GenFormStrWithoutPlaceHolders(out, text, hinf): `text` with the INF's
/// strings and the LDIDs' directories put in for its %name%s.
fn gen_form_str_without_place_holders(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, text, hinf] = args else {
        unreachable!()
    };
    let text = super::string(cpu, text);
    let file = inf_of(cpu, hinf).map(|(f, _)| f).unwrap_or_default();
    let dirs = dirs(cpu);
    let expanded = file.expand(&text, &dirs);
    let room = super::segment_room(cpu, out);
    put_string(cpu, out, room, &expanded);
    Some(0)
}

/// CtlGetLddPath(ldid, buffer): the LDID's directory.
fn ctl_get_ldd_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[ldid, buffer] = args else {
        unreachable!()
    };
    let dirs = dirs(cpu);
    let Some(path) = dirs(ldid as u16) else {
        return Some(ERR_VCP_LDDFIND);
    };
    put_string(cpu, buffer, MAX_PATH, &path);
    Some(0)
}

/// CtlSetLddPath(ldid, path): give the LDID the directory `path`.
fn ctl_set_ldd_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[ldid, path] = args else { unreachable!() };
    let path = super::string(cpu, path);
    let ldids = &mut state(cpu).ldids;
    ldids.retain(|(l, _)| *l as u32 != ldid);
    ldids.push((ldid as u16, path));
    Some(0)
}

/// VcpOpen(callback, data): open the queue of file operations.
fn vcp_open(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let setupx = state(cpu);
    if setupx.queue.is_some() {
        return Some(ERR_VCP_BUSY);
    }
    setupx.queue = Some(Vec::new());
    Some(0)
}

/// VcpClose(flags, backup directory): carry out the queue's operations
/// the flags name, and close it.
fn vcp_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(queue) = state(cpu).queue.take() else {
        return Some(ERR_VCP_NOTOPEN);
    };
    let flags = args[0];
    let ops: Vec<FileOp> = queue
        .into_iter()
        .filter(|op| {
            let wanted = match op.kind {
                inf::COPY => VCPFL_COPY,
                inf::DELETE => VCPFL_DELETE,
                _ => VCPFL_RENAME,
            };
            flags & wanted != 0
        })
        .collect();
    Some(match inf::commit(cpu, &ops) {
        Ok(()) => 0,
        Err(_) => ERR_VCP_IOFAIL,
    })
}

/// vcpUICallbackProc(object, message, wParam, lParam, data): the queue's
/// own callback, which shows its progress and asks for disks. Every
/// message gets VCPN_OK.
fn vcp_ui_callback_proc(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// suErrorToIds(error, type): the string resource of SETUPX that says
/// what the error is. There are none: 0, and W95INF16 then formats the
/// error's number in its own message.
fn su_error_to_ids(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
