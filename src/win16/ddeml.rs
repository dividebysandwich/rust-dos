//! DDEML, the DDE Management Library of Windows 3.1, for 16-bit
//! programs. As on Windows 95 and 98, where the 16-bit DDEML.DLL and
//! USER32's DDEML share the system's instances, string handles and
//! conversations, its functions go on to USER32's (win32/user32/
//! ddeml.rs): a 16-bit program reaches the same servers, the shell's
//! Program Manager among them, which setup programs ask to make their
//! groups.
//!
//! What differs for 16-bit programs is done here: the callback is a
//! 16:16 procedure, which USER32 gets a 32-bit thunk of; the data of a
//! transaction may be a data handle where a pointer would be; and
//! DdeAccessData gives a 16:16 pointer, into a global block of the
//! program's that holds a copy of the data until DdeUnaccessData puts
//! it back. CONVINFO has Windows 3.1's narrower fields.

use super::builtin::{Def, rust, win32};
use super::{global, thunk};
use crate::cpu::Cpu;
use crate::win32::user32::ddeml as dde;

const U: &str = "USER32.DLL";

/// DdeCallback(type, format, conversation, hsz1, hsz2, data, data1,
/// data2): two words, then six longs, returning a long.
const CALLBACK_ARGS: u32 = 8 | 0xFC << 8;

pub static MODULE: Def = Def {
    name: "DDEML",
    file: "DDEML.DLL",
    apis: &[
        rust("DdeInitialize", 2, "pSll", dde_initialize),
        win32("DdeUninitialize", 3, "l", U, "DdeUninitialize", b'b'),
        win32("DdeConnect", 7, "lllp", U, "DdeConnect", b'w'),
        win32("DdeDisconnect", 8, "l", U, "DdeDisconnect", b'b'),
        rust("DdeQueryConvInfo", 9, "llp", dde_query_conv_info),
        win32("DdeSetUserHandle", 10, "lll", U, "DdeSetUserHandle", b'b'),
        rust(
            "DdeClientTransaction",
            11,
            "Slllwwlp",
            dde_client_transaction,
        ),
        win32(
            "DdeAbandonTransaction",
            12,
            "lll",
            U,
            "DdeAbandonTransaction",
            b'b',
        ),
        win32("DdePostAdvise", 13, "lll", U, "DdePostAdvise", b'b'),
        win32(
            "DdeCreateDataHandle",
            14,
            "lplllww",
            U,
            "DdeCreateDataHandle",
            b'w',
        ),
        win32("DdeAddData", 15, "lpll", U, "DdeAddData", b'w'),
        win32("DdeGetData", 16, "lpll", U, "DdeGetData", b'w'),
        rust("DdeAccessData", 17, "lp", dde_access_data),
        rust("DdeUnaccessData", 18, "l", dde_unaccess_data),
        rust("DdeFreeDataHandle", 19, "l", dde_free_data_handle),
        win32("DdeGetLastError", 20, "l", U, "DdeGetLastError", b'w'),
        win32(
            "DdeCreateStringHandle",
            21,
            "lps",
            U,
            "DdeCreateStringHandleA",
            b'w',
        ),
        win32(
            "DdeFreeStringHandle",
            22,
            "ll",
            U,
            "DdeFreeStringHandle",
            b'b',
        ),
        win32("DdeQueryString", 23, "llpls", U, "DdeQueryStringA", b'w'),
        win32(
            "DdeKeepStringHandle",
            24,
            "ll",
            U,
            "DdeKeepStringHandle",
            b'b',
        ),
        win32("DdeEnableCallback", 26, "llw", U, "DdeEnableCallback", b'b'),
        win32("DdeNameService", 27, "lllw", U, "DdeNameService", b'w'),
        win32(
            "DdeCmpStringHandles",
            36,
            "ll",
            U,
            "DdeCmpStringHandles",
            b'w',
        ),
    ],
    library: true,
};

/// DdeInitialize(id, callback, flags, reserved): USER32's, given a
/// thunk of the 16:16 callback.
fn dde_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [out, callback, flags, _] = args[..] else {
        return Some(0);
    };
    let callback = thunk::proc32(cpu, callback, thunk::KIND_PLAIN, CALLBACK_ARGS);
    super::misc::go(
        cpu,
        U,
        "DdeInitializeA",
        &[out, callback, flags, 0],
        Vec::new(),
    )
}

/// DdeClientTransaction(data, size, conversation, item, format, type,
/// timeout, result): the data a 16:16 pointer, or a data handle for a
/// size of -1.
fn dde_client_transaction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [source, size, hconv, item, format, kind, timeout, result] = args[..] else {
        return Some(0);
    };
    let source = if size == u32::MAX {
        source
    } else {
        super::linear(cpu, source)
    };
    let all = [source, size, hconv, item, format, kind, timeout, result];
    super::misc::go(cpu, U, "DdeClientTransaction", &all, Vec::new())
}

/// DdeQueryConvInfo(conversation, transaction, info): Windows 3.1's
/// CONVINFO, as much of it as its cb asks for; the bytes written.
fn dde_query_conv_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    /// CP_WINANSI, and the size of Windows 3.1's CONVCONTEXT.
    const CP_WINANSI: u16 = 1004;
    const CONVCONTEXT_SIZE: u16 = 16;
    let [hconv, id, out] = args[..] else {
        return Some(0);
    };
    if out == 0 {
        return Some(0);
    }
    let Some(info) = dde::conv_info(cpu, hconv, id) else {
        return Some(0);
    };
    // cb, hUser, hConvPartner, hszSvcPartner, hszServiceReq, hszTopic,
    // hszItem; wFmt, wType, wStatus, wConvst, wLastError; hConvList;
    // ConvCtxt (cb, wFlags, wCountryID, iCodePage, dwLangID,
    // dwSecurity).
    let mut bytes = Vec::new();
    for v in [
        0,
        info.user,
        0,
        info.service,
        info.service,
        info.topic,
        info.item,
    ] {
        bytes.extend(v.to_le_bytes());
    }
    for v in [
        info.format as u16,
        info.kind as u16,
        info.status,
        info.state,
        0,
    ] {
        bytes.extend(v.to_le_bytes());
    }
    bytes.extend(0u32.to_le_bytes());
    for v in [CONVCONTEXT_SIZE, 0, 0, CP_WINANSI] {
        bytes.extend(v.to_le_bytes());
    }
    bytes.extend([0u8; 8]);
    let cb = super::read_32(cpu, out) as usize;
    let n = cb.min(bytes.len());
    if n < 4 || !super::poke(cpu, out + 4, &bytes[4..n]) {
        return Some(0);
    }
    Some(n as u32)
}

/// DdeAccessData(data, size): a 16:16 pointer to a copy of the data, in
/// a global block of the running task's, and its size.
fn dde_access_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [h, size_out] = args[..] else {
        return Some(0);
    };
    let Some(d) = dde::data_mut(cpu, h) else {
        return Some(0);
    };
    let (bytes, block) = (d.bytes.clone(), d.access16);
    let block = if block != 0 {
        block as u16
    } else {
        // A block of one byte at least, as GlobalAlloc gives none of 0.
        let mut copy = bytes.clone();
        if copy.is_empty() {
            copy.push(0);
        }
        let block = super::global_block_of(cpu, &copy);
        if block == 0 {
            return Some(0);
        }
        global::lock(cpu, block);
        dde::data_mut(cpu, h)?.access16 = block as u32;
        block
    };
    if size_out != 0 {
        super::write_32(cpu, size_out, bytes.len() as u32);
    }
    Some((global::handle_to_sel(block) as u32) << 16)
}

/// The 16-bit copy of the data `h` DdeAccessData made, if there is one:
/// its bytes put back into the data if `keep`, and the block freed.
fn end_access(cpu: &mut Cpu, h: u32, keep: bool) {
    let Some(d) = dde::data_mut(cpu, h) else {
        return;
    };
    let (block, len) = (std::mem::take(&mut d.access16) as u16, d.bytes.len());
    if block == 0 {
        return;
    }
    if keep && let Some(mut bytes) = super::global_bytes(cpu, block) {
        bytes.truncate(len);
        if bytes.len() == len
            && let Some(d) = dde::data_mut(cpu, h)
        {
            d.bytes = bytes;
        }
    }
    global::unlock(cpu, block);
    global::free(cpu, block);
}

/// DdeUnaccessData(data).
fn dde_unaccess_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if dde::data_mut(cpu, args[0]).is_none() {
        return Some(0);
    }
    end_access(cpu, args[0], true);
    Some(1)
}

/// DdeFreeDataHandle(data).
fn dde_free_data_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    end_access(cpu, args[0], false);
    dde::dde_free_data_handle(cpu, args)
}
