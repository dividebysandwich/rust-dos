//! Streams and byte arrays on global memory: CreateStreamOnHGlobal's
//! IStream and CreateILockBytesOnHGlobal's ILockBytes, whose data is a
//! block GlobalAlloc gave. Their state (the handle, the size, the
//! position) is here; the data stays in the block, so the program can
//! read it with GlobalLock as on Windows.
//!
//! What the methods do follows Wine's ole32/hglobalstream.c and
//! memlockbytes.c (LGPL 2.1 or later):
//!
//! Copyright 1999 Thuy Nguyen
//! Copyright 1999 Francis Beaudet

use super::{dll_index, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid, S_OK};
use crate::win32::kernel32;
use crate::win32::task::{Next, Task};

/// Structured storage's errors.
pub const STG_E_INVALIDFUNCTION: u32 = 0x8003_0001;
pub const STG_E_INVALIDPOINTER: u32 = 0x8003_0009;
pub const STG_E_SEEKERROR: u32 = 0x8003_0019;
pub const STG_E_READFAULT: u32 = 0x8003_001E;

pub const IID_ISEQUENTIALSTREAM: Guid = Guid::new(
    0x0C73_3A30,
    0x2A1C,
    0x11CE,
    [0xAD, 0xE5, 0x00, 0xAA, 0x00, 0x44, 0x77, 0x3D],
);
pub const IID_ISTREAM: Guid = Guid::new(0xC, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_ILOCKBYTES: Guid = Guid::new(0xA, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// GlobalAlloc's flags for a block OLE makes.
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_NODISCARD: u32 = 0x0020;
const GMEM_SHARE: u32 = 0x2000;

/// STATSTG's size, and its types.
pub const STATSTG_SIZE: usize = 72;
pub const STGTY_STREAM: u32 = 2;
pub const STGTY_LOCKBYTES: u32 = 3;

/// An object on global memory: a stream or a byte array.
#[derive(Clone, Debug, Default)]
pub struct Global {
    pub pid: u32,
    pub refs: u32,
    /// The block's handle, and whether Release frees it.
    pub handle: u32,
    pub delete: bool,
    /// The size the object says the data has, which it keeps when the
    /// block is freed under it, and a stream's position.
    pub size: u32,
    pub position: u32,
    /// Whether it is an ILockBytes.
    pub lock_bytes: bool,
}

crate::state_fields!(Global {
    pid,
    refs,
    handle,
    delete,
    size,
    position,
    lock_bytes
});

fn globals(cpu: &mut Cpu) -> &mut Vec<Option<Global>> {
    &mut cpu.bus.win32.ole.globals
}

/// The object of the interface `this`, if it is one of these.
fn object(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("IStream") && v != vtable("ILockBytes") {
        return None;
    }
    let id = id as usize;
    globals(cpu).get(id)?.as_ref()?;
    Some(id)
}

fn get(cpu: &mut Cpu, id: usize) -> &mut Global {
    globals(cpu)[id].as_mut().expect("an object of the table")
}

/// A new object on the block `handle` (a new empty block when it is 0):
/// its interface, or None when memory is out.
fn new_object(cpu: &mut Cpu, handle: u32, delete: bool, lock_bytes: bool) -> Option<u32> {
    let handle = match handle {
        0 => kernel32::global_alloc(cpu, GMEM_MOVEABLE | GMEM_NODISCARD | GMEM_SHARE, 0),
        h => h,
    };
    if handle == 0 {
        return None;
    }
    let size = kernel32::global_size(cpu, handle);
    let object = Global {
        pid: crate::win32::user32::current_pid(cpu),
        refs: 1,
        handle,
        delete,
        size,
        position: 0,
        lock_bytes,
    };
    let table = globals(cpu);
    let id = match table.iter().position(Option::is_none) {
        Some(i) => {
            table[i] = Some(object);
            i
        }
        None => {
            table.push(Some(object));
            table.len() - 1
        }
    };
    let kind = vtable(if lock_bytes { "ILockBytes" } else { "IStream" });
    let at = com::new_interface(cpu, dll_index(), kind, id as u32, 0);
    if at.is_none() {
        globals(cpu)[id] = None;
    }
    at
}

/// A process ended: its objects went with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in globals(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

/// CreateStreamOnHGlobal(handle, delete on release, out).
pub fn create_stream_on_hglobal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, false)
}

/// CreateILockBytesOnHGlobal(handle, delete on release, out).
pub fn create_ilockbytes_on_hglobal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, true)
}

fn create(cpu: &mut Cpu, args: &[u32], lock_bytes: bool) -> Option<u32> {
    let &[handle, delete, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_INVALIDARG);
    }
    match new_object(cpu, handle, delete != 0, lock_bytes) {
        Some(at) => {
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// GetHGlobalFromStream(stream, out) and GetHGlobalFromILockBytes: the
/// block of an object these functions made.
pub fn get_hglobal_from_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_hglobal(cpu, args, false)
}

pub fn get_hglobal_from_ilockbytes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_hglobal(cpu, args, true)
}

fn get_hglobal(cpu: &mut Cpu, args: &[u32], lock_bytes: bool) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    if this == 0 {
        return Some(E_INVALIDARG);
    }
    let handle = match object(cpu, this) {
        Some(id) if get(cpu, id).lock_bytes == lock_bytes => get(cpu, id).handle,
        _ => 0,
    };
    if !com::write_out(cpu, out, handle) {
        return Some(E_INVALIDARG);
    }
    Some(if handle == 0 { E_INVALIDARG } else { S_OK })
}

/// QueryInterface for IUnknown, ISequentialStream and IStream, or
/// ILockBytes.
pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let lock_bytes = get(cpu, id).lock_bytes;
    let ok = match Guid::read(cpu, iid) {
        Some(g) if g == com::IID_IUNKNOWN => true,
        Some(g) if lock_bytes => g == IID_ILOCKBYTES,
        Some(g) => g == IID_ISTREAM || g == IID_ISEQUENTIALSTREAM,
        None => false,
    };
    if !ok {
        com::write_out(cpu, out, 0);
        return Some(E_NOINTERFACE);
    }
    get(cpu, id).refs += 1;
    com::write_out(cpu, out, this);
    Some(S_OK)
}

pub fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(0);
    };
    let o = get(cpu, id);
    o.refs += 1;
    Some(o.refs)
}

/// Release: the last one frees the block if the object was made to.
pub fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    let Some(id) = object(cpu, this) else {
        return Some(0);
    };
    let o = get(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    let refs = o.refs;
    if refs == 0 {
        let o = globals(cpu)[id].take().expect("an object of the table");
        if o.delete {
            kernel32::global_free(cpu, o.handle);
        }
        com::free_interface(cpu, this);
    }
    Some(refs)
}

/// Copy `n` bytes between the object's block at `offset` and the
/// program's memory at `at`: false when the block is gone.
fn transfer(cpu: &mut Cpu, id: usize, offset: u32, at: u32, n: u32, write: bool) -> Option<bool> {
    let handle = get(cpu, id).handle;
    let block = kernel32::global_address(cpu, handle);
    if block == 0 {
        return Some(false);
    }
    if n == 0 {
        return Some(true);
    }
    let (from, to) = if write {
        (at, block + offset)
    } else {
        (block + offset, at)
    };
    if !cpu.bus.guest_probe(from, n as usize, false) || !cpu.bus.guest_probe(to, n as usize, true) {
        return None;
    }
    let mut bytes = vec![0; n as usize];
    cpu.bus.guest_read_bytes(from, &mut bytes);
    cpu.bus.guest_write_bytes(to, &bytes);
    Some(true)
}

/// Make the object's block `size` bytes: E_OUTOFMEMORY when GlobalReAlloc
/// can't.
fn set_size(cpu: &mut Cpu, id: usize, size: u32) -> u32 {
    let o = get(cpu, id);
    if o.size == size {
        return S_OK;
    }
    let handle = o.handle;
    let handle = kernel32::global_realloc(cpu, handle, size, 0);
    if handle == 0 {
        return E_OUTOFMEMORY;
    }
    let o = get(cpu, id);
    o.handle = handle;
    o.size = size;
    S_OK
}

/// The bytes of the stream `this` from its position on, for another
/// built-in DLL (DirectMusic's IPersistStream::Load), without moving
/// the position: None when `this` isn't a stream on global memory, or
/// its block is gone.
pub fn rest_of(cpu: &mut Cpu, this: u32) -> Option<Vec<u8>> {
    let id = object(cpu, this)?;
    let o = get(cpu, id);
    if o.lock_bytes {
        return None;
    }
    let (position, size, handle) = (o.position, o.size, o.handle);
    let block = kernel32::global_address(cpu, handle);
    if block == 0 {
        return None;
    }
    let n = size.saturating_sub(position) as usize;
    if !cpu.bus.guest_probe(block + position, n, false) {
        return None;
    }
    let mut bytes = vec![0; n];
    cpu.bus.guest_read_bytes(block + position, &mut bytes);
    Some(bytes)
}

/// Move the position of the stream `this` on by `n` bytes, as reading
/// them does.
pub fn advance(cpu: &mut Cpu, this: u32, n: u32) {
    if let Some(id) = object(cpu, this) {
        let o = get(cpu, id);
        o.position = o.position.saturating_add(n).min(o.size);
    }
}

/// IStream::Read(this, buffer, count, read): what there is from the
/// position on, up to `count` bytes. S_OK at the end too.
pub fn read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, count, read] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let o = get(cpu, id);
    let (position, size) = (o.position, o.size);
    let n = size.saturating_sub(position).min(count);
    let done = match transfer(cpu, id, position, buffer, n, false)? {
        true => {
            get(cpu, id).position = position + n;
            n
        }
        false => 0,
    };
    if read != 0 && !com::write_out(cpu, read, done) {
        return None;
    }
    Some(S_OK)
}

/// IStream::Write(this, buffer, count, written): at the position, the
/// block growing to take it.
pub fn write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, count, written] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if written != 0 && !com::write_out(cpu, written, 0) {
        return None;
    }
    if count == 0 {
        return Some(S_OK);
    }
    let o = get(cpu, id);
    let position = o.position;
    // Past 4 GB the block can't grow to take it.
    let Some(end) = position.checked_add(count) else {
        return Some(E_OUTOFMEMORY);
    };
    if end > o.size {
        let hr = set_size(cpu, id, end);
        if hr != S_OK {
            return Some(hr);
        }
    }
    if transfer(cpu, id, position, buffer, count, true)? {
        get(cpu, id).position = end;
    }
    if written != 0 {
        com::write_out(cpu, written, count);
    }
    Some(S_OK)
}

/// STREAM_SEEK_ origins.
const STREAM_SEEK_SET: u32 = 0;
const STREAM_SEEK_CUR: u32 = 1;
const STREAM_SEEK_END: u32 = 2;

/// IStream::Seek(this, move (64 bits), origin, new position): positions
/// are 32-bit, the high half of the move ignored; a move back past the
/// start fails.
pub fn seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, low, _, origin, out] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let o = get(cpu, id);
    let base = match origin {
        STREAM_SEEK_SET => Some(0),
        STREAM_SEEK_CUR => Some(o.position),
        STREAM_SEEK_END => Some(o.size),
        _ => None,
    };
    let hr = match base {
        Some(base) => {
            let to = base.wrapping_add(low);
            if low >= 0x8000_0000 && to >= low {
                STG_E_SEEKERROR
            } else {
                o.position = to;
                S_OK
            }
        }
        None => STG_E_SEEKERROR,
    };
    let position = get(cpu, id).position;
    if out != 0 {
        if !cpu.bus.guest_probe(out, 8, true) {
            return None;
        }
        cpu.bus.guest_write_32(out, position);
        cpu.bus.guest_write_32(out + 4, 0);
    }
    Some(hr)
}

/// IStream::SetSize(this, size (64 bits)) and ILockBytes::SetSize: the
/// high half is ignored.
pub fn stream_set_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(set_size(cpu, id, args[1]))
}

/// Commit, Revert, UnlockRegion and Flush do nothing.
pub fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// LockRegion: these objects can't lock.
pub fn lock_region(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(STG_E_INVALIDFUNCTION)
}

/// IStream::Stat(this, statstg, flag) and ILockBytes::Stat: the type and
/// size, no name.
pub fn stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out, _] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, STATSTG_SIZE, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let o = get(cpu, id);
    let (kind, size) = (
        if o.lock_bytes {
            STGTY_LOCKBYTES
        } else {
            STGTY_STREAM
        },
        o.size,
    );
    cpu.bus.guest_fill(out, STATSTG_SIZE, 0);
    cpu.bus.guest_write_32(out + 4, kind);
    cpu.bus.guest_write_32(out + 8, size);
    Some(S_OK)
}

/// IStream::Clone(this, out): a stream on the same block, at the same
/// position, which doesn't free it.
pub fn clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let (handle, position) = (get(cpu, id).handle, get(cpu, id).position);
    let Some(at) = new_object(cpu, handle, false, false) else {
        return Some(E_OUTOFMEMORY);
    };
    if let Some(new) = object(cpu, at) {
        get(cpu, new).position = position;
    }
    cpu.bus.guest_write_32(out, at);
    Some(S_OK)
}

/// ILockBytes::ReadAt(this, offset (64 bits), buffer, count, read).
pub fn read_at(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, offset, _, buffer, count, read] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let size = get(cpu, id).size;
    let n = size.saturating_sub(offset).min(count);
    let done = if n > 0 && transfer(cpu, id, offset, buffer, n, false)? {
        n
    } else {
        0
    };
    if read != 0 && !com::write_out(cpu, read, done) {
        return None;
    }
    Some(S_OK)
}

/// ILockBytes::WriteAt(this, offset (64 bits), buffer, count, written):
/// the block grows to take it.
pub fn write_at(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, offset, _, buffer, count, written] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if written != 0 && !com::write_out(cpu, written, 0) {
        return None;
    }
    if count == 0 {
        return Some(S_OK);
    }
    let Some(end) = offset.checked_add(count) else {
        return Some(STG_E_READFAULT);
    };
    if end > get(cpu, id).size {
        let hr = set_size(cpu, id, end);
        if hr != S_OK {
            return Some(STG_E_READFAULT);
        }
    }
    if !transfer(cpu, id, offset, buffer, count, true)? {
        return Some(STG_E_READFAULT);
    }
    if written != 0 {
        com::write_out(cpu, written, count);
    }
    Some(S_OK)
}

/// The variables of CopyTo's task.
const C_THIS: usize = 1;
const C_TARGET: usize = 2;
const C_LEFT_LOW: usize = 3;
const C_LEFT_HIGH: usize = 4;
const C_READ_OUT: usize = 5;
const C_WRITTEN_OUT: usize = 6;
const C_READ: usize = 7;
const C_WRITTEN: usize = 8;

/// How much CopyTo copies at a time, through its scratch space, which
/// has the target's "written" count after it.
const CHUNK: u32 = 4096;
pub const COPY_SCRATCH: u32 = CHUNK + 4;

/// IStream::CopyTo(this, target, count (64 bits), read, written): from
/// the position on into the target stream, whose Write the program may
/// have written.
pub fn copy_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, target, low, high, read, written] = args else {
        unreachable!()
    };
    if target == 0 {
        return Some(STG_E_INVALIDPOINTER);
    }
    if object(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    let vars = vec![
        super::calls::COPY_TO,
        this,
        target,
        low,
        high,
        read,
        written,
        0,
        0,
    ];
    super::calls::start(cpu, vars, COPY_SCRATCH)
}

/// CopyTo's steps: read a chunk into the scratch space, have the target
/// write it, until the count is copied or the stream ends.
pub fn copy_to_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 {
        // The target's Write returned.
        if value & 0x8000_0000 != 0 {
            return finish_copy(cpu, t, value);
        }
        let written = cpu.bus.guest_read_32(t.scratch + CHUNK);
        t.set(C_WRITTEN, t.var(C_WRITTEN).wrapping_add(written));
    }
    let left = (t.var(C_LEFT_HIGH) as u64) << 32 | t.var(C_LEFT_LOW) as u64;
    if left == 0 {
        return finish_copy(cpu, t, S_OK);
    }
    let Some(id) = object(cpu, t.var(C_THIS)) else {
        return finish_copy(cpu, t, E_INVALIDARG);
    };
    let want = left.min(CHUNK as u64) as u32;
    let o = get(cpu, id);
    let position = o.position;
    let n = o.size.saturating_sub(position).min(want);
    let n = match transfer(cpu, id, position, t.scratch, n, false) {
        Some(true) => {
            get(cpu, id).position = position + n;
            n
        }
        _ => 0,
    };
    t.set(C_READ, t.var(C_READ).wrapping_add(n));
    // A short read is the stream's end: the copy ends after its Write.
    let left = if n != want { 0 } else { left - n as u64 };
    t.set(C_LEFT_LOW, left as u32);
    t.set(C_LEFT_HIGH, (left >> 32) as u32);
    if n == 0 {
        return finish_copy(cpu, t, S_OK);
    }
    let target = t.var(C_TARGET);
    let Some(write) = method(cpu, target, 4) else {
        return finish_copy(cpu, t, STG_E_INVALIDPOINTER);
    };
    cpu.bus.guest_write_32(t.scratch + CHUNK, 0);
    Next::call(write, &[target, t.scratch, n, t.scratch + CHUNK], 1)
}

fn finish_copy(cpu: &mut Cpu, t: &Task, hr: u32) -> Next {
    for (out, value) in [
        (t.var(C_READ_OUT), C_READ),
        (t.var(C_WRITTEN_OUT), C_WRITTEN),
    ] {
        if out != 0 && cpu.bus.guest_probe(out, 8, true) {
            cpu.bus.guest_write_32(out, t.var(value));
            cpu.bus.guest_write_32(out + 4, 0);
        }
    }
    Next::Done(hr)
}

/// The address of the method `index` of the interface `this`, from its
/// vtable; None when it can't be read.
pub fn method(cpu: &mut Cpu, this: u32, index: u32) -> Option<u32> {
    if this == 0 || !cpu.bus.guest_probe(this, 4, false) {
        return None;
    }
    let vtable = cpu.bus.guest_read_32(this);
    let at = vtable.checked_add(4 * index)?;
    if !cpu.bus.guest_probe(at, 4, false) {
        return None;
    }
    Some(cpu.bus.guest_read_32(at))
}
