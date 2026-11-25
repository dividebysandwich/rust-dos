//! Windows 3.1's GDI heap, for a process with
//! [`Shim::Win31GdiHeap`]: a view of GDI32's bitmaps laid out as
//! Windows 3.1's GDI kept them ([`super`] has the layout and where it is
//! in GDI.EXE).
//!
//! GDI32 keeps its objects in the emulator, so a 16-bit program that
//! reads GDI's heap finds nothing there. The view gives the process a
//! segment of 64K, built at its first SystemHeapInfo, which returns it as
//! hGDISegment. Each of the process's bitmaps has a handle table entry at
//! the offset its handle is (the handles are GDI32's, the same in 16-bit
//! code), an object somewhere free in the segment, and a global block
//! with the device's BITMAP and a copy of the bits. A bitmap gets them
//! when the view is built and when GDI32 makes it, for 16-bit or 32-bit
//! code, and loses them when GDI32 frees it (`bitmap_added`,
//! `bitmap_freed`): a handle GDI32 gives again never finds the device
//! BITMAP of the bitmap that had it before.
//!
//! GDI32 keeps a bitmap's bits in the emulator, not in the guest's
//! memory, so the block can't share them: it has a copy of the bits as
//! they were when the bitmap joined the view. The view writes them back
//! nowhere. What it does read back is the device's BITMAP: once the
//! program changes it (Lunicus's LUNIFX.DLL sets the size and points
//! bmBits at its own frame), a 16-bit BitBlt or StretchBlt from a DC the
//! bitmap is selected into takes the size from it and the rows it reads
//! from where bmBits points, as Windows 3.1's display driver did. Until
//! then, GDI32's bits are the bitmap's.
//!
//! Windows 3.1's GDI heap was GDI's, there as long as Windows ran. The
//! Win16 process's view, which every 16-bit program shares, has blocks
//! of the system's (owner 0), which stay while 16-bit programs come and
//! go. A Win32 process's view, for the 16-bit code a client task runs
//! for it (Win32s's 16-bit DLLs), has blocks of that task, and goes with
//! it.
//!
//! [`Shim::Win31GdiHeap`]: crate::win32::shims::Shim::Win31GdiHeap

use crate::cpu::Cpu;
use crate::win16::{global, ldt};
use crate::win32::shims::Shim;

/// The W32 section's version that added each process's view.
pub const GDI_HEAP_SAVED: u16 = 94;

/// The segment's size: a local heap's, all of 64K.
const SEGMENT_SIZE: u32 = 0x1_0000;
/// A bitmap object's size, header included, and its magic.
const OBJECT_SIZE: u32 = 0x1C;
const BITMAP_MAGIC: u16 = 0x4F4B;
/// The device's BITMAP: its size, and how much more than the bits its
/// block has.
const DEVICE_BITMAP: u32 = 0x20;
const BLOCK_EXTRA: u32 = 0x24;
/// The object's flags (+1Ah): its bits cross 64K.
const HUGE: u16 = 1;

/// A process's view of GDI's heap.
#[derive(Clone, Debug, Default)]
pub struct GdiHeapView {
    /// The task whose blocks the view's are, 0 for the system's.
    pub owner: u16,
    /// The segment's global handle, hGDISegment: 0 while there is no
    /// view.
    pub segment: u16,
    /// The serial number the next object gets (+04h).
    pub serial: u32,
    pub bitmaps: Vec<Viewed>,
    /// The tasks the log has its hint for, each a program without the
    /// shim: a 16-bit program's, or a Win32 process's client task.
    pub hinted: Vec<u16>,
}

/// A bitmap in the view.
#[derive(Clone, Debug, Default)]
pub struct Viewed {
    /// GDI32's handle, the offset of its handle table entry.
    pub handle: u16,
    /// The offset of its object.
    pub object: u16,
    /// The global handles of the device's BITMAP and of the bits, when
    /// they have a block of their own (0 otherwise).
    pub block: u16,
    pub bits: u16,
    /// The device's BITMAP as the view wrote it.
    pub written: Vec<u8>,
    /// Whether the program changed it since: BitBlt then draws from it.
    pub taken: bool,
}

crate::state_fields!(GdiHeapView {
    owner,
    segment,
    serial,
    bitmaps,
    hinted,
});

crate::state_fields!(Viewed {
    handle,
    object,
    block,
    bits,
    written,
    taken,
});

/// The index of the process the running 16-bit code serves, and whether
/// it has the shim.
fn served(cpu: &Cpu) -> Option<(usize, bool)> {
    let pid = crate::win16::flat::served_pid(cpu);
    let p = cpu.bus.win32.processes.iter().position(|p| p.id == pid)?;
    Some((p, cpu.bus.win32.processes[p].shims.has(Shim::Win31GdiHeap)))
}

/// The view of the process the running 16-bit code serves, taken out to
/// work on; `put` puts it back.
fn take(cpu: &mut Cpu) -> Option<(usize, GdiHeapView)> {
    let (p, shimmed) = served(cpu)?;
    if !shimmed {
        return None;
    }
    let view = std::mem::take(&mut *cpu.bus.win32.processes[p].gdi_heap);
    Some((p, view))
}

fn put(cpu: &mut Cpu, p: usize, view: GdiHeapView) {
    *cpu.bus.win32.processes[p].gdi_heap = view;
}

/// TOOLHELP's SystemHeapInfo: hGDISegment, the view's segment, built
/// now if the process has none yet (or brought up to date with the
/// bitmaps it has), or 0 for a program without the shim, which gets a
/// hint in the log the first time.
pub fn system_heap_info(cpu: &mut Cpu) -> u16 {
    let Some((p, shimmed)) = served(cpu) else {
        return 0;
    };
    if !shimmed {
        hint(cpu, p);
        return 0;
    }
    let Some((p, mut view)) = take(cpu) else {
        return 0;
    };
    if view.segment == 0 {
        build(cpu, p, &mut view);
    }
    if view.segment != 0 {
        for handle in bitmaps_of(cpu) {
            add(cpu, &mut view, handle);
        }
    }
    let segment = view.segment;
    put(cpu, p, view);
    segment
}

/// The log's hint for a program without the shim that asks where GDI's
/// heap is, once a task.
fn hint(cpu: &mut Cpu, p: usize) {
    let (task, path) = running_program(cpu, p);
    let hinted = &mut cpu.bus.win32.processes[p].gdi_heap.hinted;
    if hinted.contains(&task) {
        return;
    }
    hinted.push(task);
    let line = format!(
        "[W16] {} asked TOOLHELP for GDI's heap (SystemHeapInfo), which \
         Windows 98 doesn't have: a program that reads GDI's objects there \
         needs the shim \"{}\" (the win31 and win32s sets, win32_shims=win31)",
        path,
        Shim::Win31GdiHeap.describe()
    );
    cpu.bus.log_string(&line);
}

/// The running task's handle (0 without one) and the program it runs:
/// in the Win16 process, whose own path is KRNL386.EXE's, the 16-bit
/// program's module file, and else the Win32 process `p`'s.
fn running_program(cpu: &Cpu, p: usize) -> (u16, String) {
    let win16 = &cpu.bus.win32.win16;
    let task = crate::win16::current_task(cpu).map(|t| &win16.tasks[t]);
    let process = &cpu.bus.win32.processes[p];
    let module = task
        .filter(|_| process.id == win16.pid)
        .and_then(|t| win16.modules.iter().find(|m| m.handle == t.module));
    let path = module.map_or_else(|| process.path.clone(), |m| m.path.clone());
    (task.map_or(0, |t| t.handle), path)
}

/// Make the view's segment for the process `p`: of the system's in the
/// Win16 process, of the running client task's in a Win32 process.
fn build(cpu: &mut Cpu, p: usize, view: &mut GdiHeapView) {
    let owner = if cpu.bus.win32.processes[p].id == cpu.bus.win32.win16.pid {
        0
    } else {
        let Some(t) = crate::win16::current_task(cpu) else {
            return;
        };
        cpu.bus.win32.win16.tasks[t].handle
    };
    let flags = global::GMEM_MOVEABLE | global::GMEM_ZEROINIT;
    let segment = global::alloc(cpu, flags, SEGMENT_SIZE, owner, false);
    if segment == 0 {
        return;
    }
    *view = GdiHeapView {
        owner,
        segment,
        serial: 1,
        bitmaps: Vec::new(),
        hinted: std::mem::take(&mut view.hinted),
    };
}

/// The bitmaps the running 16-bit code can have: those of the process it
/// serves and of the Win16 process, which those 16-bit code makes are.
fn bitmaps_of(cpu: &Cpu) -> Vec<u32> {
    let served = crate::win16::flat::served_pid(cpu);
    let win16 = cpu.bus.win32.win16.pid;
    cpu.bus
        .win32
        .gdi
        .handles()
        .filter(|(_, o)| {
            let ours = o.pid == served || o.pid == win16;
            let bitmap = matches!(o.kind, crate::win32::gdi32::Kind::Bitmap { .. });
            ours && bitmap && !o.stock && !o.deleted
        })
        .map(|(h, _)| h)
        .collect()
}

/// The linear address of the segment's `offset`.
fn at(cpu: &Cpu, view: &GdiHeapView, offset: u32) -> u32 {
    global::get(cpu, view.segment).map_or(0, |a| a.base) + offset
}

/// Whether `[from, from + len)` of the segment is free of the view's
/// handle table entries and objects, but for `except`'s object.
fn free_at(view: &GdiHeapView, from: u32, len: u32, except: Option<u16>) -> bool {
    let overlaps = |start: u32, size: u32| from < start + size && start < from + len;
    view.bitmaps.iter().all(|b| {
        !overlaps(b.handle as u32, 4)
            && (Some(b.handle) == except || !overlaps(b.object as u32, OBJECT_SIZE))
    })
}

/// A free place for an object, from the top of the segment down, which
/// keeps clear of the handles below: GDI32's start at 80h and go up.
fn place(view: &GdiHeapView, except: Option<u16>) -> Option<u16> {
    (0x10..=SEGMENT_SIZE - OBJECT_SIZE)
        .rev()
        .step_by(4)
        .find(|&o| free_at(view, o, OBJECT_SIZE, except))
        .map(|o| o as u16)
}

/// Write `b`'s handle table entry: its object's offset, unlocked.
fn write_entry(cpu: &mut Cpu, view: &GdiHeapView, b: &Viewed) {
    let entry = at(cpu, view, b.handle as u32);
    crate::win16::write_32(cpu, entry, b.object as u32);
}

/// Write `b`'s object: the header with `serial`, its blocks and flags.
fn write_object(cpu: &mut Cpu, view: &GdiHeapView, b: &Viewed, serial: u32) {
    let mut object = [0u8; OBJECT_SIZE as usize];
    object[2..4].copy_from_slice(&BITMAP_MAGIC.to_le_bytes());
    object[4..8].copy_from_slice(&serial.to_le_bytes());
    object[0x0A..0x0C].copy_from_slice(&b.block.to_le_bytes());
    object[0x14..0x16].copy_from_slice(&b.bits.to_le_bytes());
    let flags = if b.bits != 0 { HUGE } else { 0 };
    object[0x1A..0x1C].copy_from_slice(&flags.to_le_bytes());
    let to = at(cpu, view, b.object as u32);
    crate::win16::poke(cpu, to, &object);
}

/// Put the GDI32 bitmap `handle` in the view, unless it is there.
fn add(cpu: &mut Cpu, view: &mut GdiHeapView, handle: u32) {
    let Ok(handle) = u16::try_from(handle) else {
        return;
    };
    if view.bitmaps.iter().any(|b| b.handle == handle) {
        return;
    }
    let Some(bitmap) = cpu.bus.win32.gdi.bitmap(handle as u32).cloned() else {
        return;
    };
    // An object where the new entry goes moves out of its way.
    let moved: Vec<usize> = (0..view.bitmaps.len())
        .filter(|&i| {
            let o = view.bitmaps[i].object as u32;
            (handle as u32) < o + OBJECT_SIZE && o < handle as u32 + 4
        })
        .collect();
    view.bitmaps.push(Viewed {
        handle,
        ..Viewed::default()
    });
    for i in moved {
        let Some(object) = place(view, Some(view.bitmaps[i].handle)) else {
            continue;
        };
        let serial = crate::win16::read_32(cpu, at(cpu, view, view.bitmaps[i].object as u32 + 4));
        view.bitmaps[i].object = object;
        write_object(cpu, view, &view.bitmaps[i], serial);
        write_entry(cpu, view, &view.bitmaps[i]);
    }
    let Some(object) = place(view, None) else {
        view.bitmaps.pop();
        return;
    };
    let device = device_bitmap(&bitmap);
    let Some((block, bits)) = alloc_blocks(cpu, view.owner, &device) else {
        view.bitmaps.pop();
        return;
    };
    let b = view.bitmaps.last_mut().expect("pushed above");
    b.object = object;
    b.block = block;
    b.bits = bits;
    let b = b.clone();
    let written = write_device_bitmap(cpu, &b, &device, &bitmap);
    let serial = view.serial;
    view.serial = view.serial.wrapping_add(1);
    write_object(cpu, view, &b, serial);
    write_entry(cpu, view, &b);
    let last = view.bitmaps.last_mut().expect("pushed above");
    last.written = written;
}

/// A device's BITMAP's sizes, as Windows 3.1's GDI works them out.
#[derive(Clone, Copy, Debug)]
struct Device {
    width: u16,
    height: u16,
    width_bytes: u16,
    planes: u8,
    bpp: u8,
    /// The bytes of the bits, and whether they cross 64K, when they get
    /// a block of their own laid out in scan segments.
    size: u32,
    huge: bool,
    scan_segment: u16,
    fill_bytes: u16,
}

fn device_bitmap(bitmap: &crate::win32::gdi32::Bitmap) -> Device {
    let width = bitmap.width.clamp(0, 0xFFFF) as u16;
    let height = bitmap.height.clamp(0, 0xFFFF) as u16;
    let planes = bitmap.planes.min(0xFF) as u8;
    let bpp = bitmap.bpp.min(0xFF) as u8;
    let width_bytes = ((width as u32 * bpp as u32).div_ceil(16) * 2) as u16;
    let scan = (width_bytes as u32 * planes as u32).max(1);
    let size = scan * height as u32;
    let huge = size + BLOCK_EXTRA > 0x1_0000;
    let (scan_segment, fill_bytes) = (0x1_0000 / scan, 0x1_0000 % scan);
    Device {
        width,
        height,
        width_bytes,
        planes,
        bpp,
        size,
        huge,
        scan_segment: scan_segment.min(0xFFFF) as u16,
        fill_bytes: fill_bytes as u16,
    }
}

/// The blocks of a bitmap's device BITMAP and of its bits, when they
/// cross 64K (0 otherwise), for `owner`.
fn alloc_blocks(cpu: &mut Cpu, owner: u16, d: &Device) -> Option<(u16, u16)> {
    let flags = global::GMEM_MOVEABLE | global::GMEM_ZEROINIT | global::GMEM_DDESHARE;
    if !d.huge {
        let block = global::alloc(cpu, flags, d.size + BLOCK_EXTRA, owner, false);
        return (block != 0).then_some((block, 0));
    }
    let segments = (d.height as u32).div_ceil(d.scan_segment.max(1) as u32);
    let block = global::alloc(cpu, flags, DEVICE_BITMAP, owner, false);
    let bits = global::alloc(cpu, flags, segments * 0x1_0000, owner, false);
    if block == 0 || bits == 0 {
        global::free(cpu, block);
        global::free(cpu, bits);
        return None;
    }
    Some((block, bits))
}

/// Write `b`'s device BITMAP and a copy of `bitmap`'s bits: the
/// BITMAP's bytes.
fn write_device_bitmap(
    cpu: &mut Cpu,
    b: &Viewed,
    d: &Device,
    bitmap: &crate::win32::gdi32::Bitmap,
) -> Vec<u8> {
    let block_sel = global::handle_to_sel(b.block);
    let (bits_sel, bits_off, segment_index) = if b.bits != 0 {
        (global::handle_to_sel(b.bits), 0, ldt::AHINCR)
    } else {
        (block_sel, DEVICE_BITMAP as u16, 0)
    };
    let mut s = vec![0u8; DEVICE_BITMAP as usize];
    let mut w16 = |at: usize, v: u16| s[at..at + 2].copy_from_slice(&v.to_le_bytes());
    w16(0x02, d.width);
    w16(0x04, d.height);
    w16(0x06, d.width_bytes);
    w16(0x0A, bits_off);
    w16(0x0C, bits_sel);
    w16(0x16, segment_index);
    w16(0x18, d.scan_segment);
    w16(0x1A, d.fill_bytes);
    s[0x08] = d.planes;
    s[0x09] = d.bpp;
    let width_planes = d.width_bytes as u32 * d.height as u32;
    s[0x0E..0x12].copy_from_slice(&width_planes.to_le_bytes());
    let base = crate::win16::lin(cpu, block_sel, 0);
    crate::win16::poke(cpu, base, &s);
    // GDI32's rows, a plane after another in each, as Windows 3.1's.
    let scan = d.width_bytes as usize * d.planes as usize;
    let stride = bitmap.stride as usize;
    for r in 0..d.height as u32 {
        let Some(to) = row_address(cpu, &s, r) else {
            break;
        };
        let from = bitmap.bits.get(r as usize * stride..).unwrap_or_default();
        let row = &from[..scan.min(stride).min(from.len())];
        crate::win16::poke(cpu, to, row);
    }
    s
}

/// The linear address of row `r` of the bits the device BITMAP `s`
/// describes: within bmBits' segment, or in the scan segments that
/// bmSegmentIndex steps through.
fn row_address(cpu: &Cpu, s: &[u8], r: u32) -> Option<u32> {
    let r16 = |at: usize| u16::from_le_bytes([s[at], s[at + 1]]) as u32;
    let (width_bytes, planes) = (r16(0x06), s[0x08].max(1) as u32);
    let (off, sel) = (r16(0x0A), r16(0x0C));
    let (index, scan_segment) = (r16(0x16), r16(0x18));
    let scan = width_bytes * planes;
    let (sel, off) = if index != 0 && scan_segment != 0 {
        (
            sel + r / scan_segment * index,
            off + r % scan_segment * scan,
        )
    } else {
        (sel, off + r * scan)
    };
    let sel = u16::try_from(sel).ok()?;
    (sel != 0).then(|| crate::win16::linear(cpu, (sel as u32) << 16) + off)
}

/// The processes whose views the GDI32 bitmap of the process `pid`
/// belongs in, as `bitmaps_of` has them: its own, and every process's
/// for a bitmap of the Win16 process, which 16-bit code makes.
fn views_for(cpu: &Cpu, pid: u32) -> Vec<usize> {
    let win16 = cpu.bus.win32.win16.pid;
    let processes = &cpu.bus.win32.processes;
    (0..processes.len())
        .filter(|&p| {
            let process = &processes[p];
            let ours = process.id == pid || (win16 != 0 && pid == win16);
            ours && process.gdi_heap.segment != 0 && process.shims.has(Shim::Win31GdiHeap)
        })
        .collect()
}

/// GDI32 made the bitmap `handle` for the process `pid`, for 16-bit or
/// 32-bit code (`gdi32::add`): it joins the views it belongs in.
pub fn bitmap_added(cpu: &mut Cpu, handle: u32, pid: u32) {
    for p in views_for(cpu, pid) {
        let mut view = std::mem::take(&mut *cpu.bus.win32.processes[p].gdi_heap);
        add(cpu, &mut view, handle);
        put(cpu, p, view);
    }
}

/// GDI32 freed the bitmap `handle` (`gdi32::free`), whichever code
/// deleted it, or deselected it once deleted: it leaves every view, and
/// its handle table entry is 0 until GDI32 gives the handle again.
pub fn bitmap_freed(cpu: &mut Cpu, handle: u32) {
    for p in 0..cpu.bus.win32.processes.len() {
        let view = &mut cpu.bus.win32.processes[p].gdi_heap;
        let Some(i) = view.bitmaps.iter().position(|b| b.handle as u32 == handle) else {
            continue;
        };
        let (b, segment) = (view.bitmaps.remove(i), view.segment);
        if let Some(segment) = global::get(cpu, segment) {
            crate::win16::write_32(cpu, segment.base + b.handle as u32, 0);
            let object = segment.base + b.object as u32;
            crate::win16::poke(cpu, object, &[0; OBJECT_SIZE as usize]);
        }
        global::free(cpu, b.block);
        if b.bits != 0 {
            global::free(cpu, b.bits);
        }
    }
}

/// A 16-bit BitBlt or StretchBlt is about to read rows `top` to
/// `bottom` (either way round) of the bitmap selected into `dc`: if the
/// program changed its device BITMAP, GDI32's bitmap takes the size it
/// has and those rows from where its bmBits points.
pub fn before_blit(cpu: &mut Cpu, dc: u32, top: i32, bottom: i32) {
    let Some((p, mut view)) = take(cpu) else {
        return;
    };
    if view.segment != 0 {
        sync(cpu, &mut view, dc, top.min(bottom), top.max(bottom));
    }
    put(cpu, p, view);
}

fn sync(cpu: &mut Cpu, view: &mut GdiHeapView, dc: u32, from: i32, to: i32) {
    let Some(handle) = cpu.bus.win32.gdi.dc(dc).map(|d| d.attrs.bitmap) else {
        return;
    };
    let Some(b) = view.bitmaps.iter_mut().find(|b| b.handle as u32 == handle) else {
        return;
    };
    let base = crate::win16::lin(cpu, global::handle_to_sel(b.block), 0);
    let Some(s) = crate::win16::peek(cpu, base, DEVICE_BITMAP as usize) else {
        return;
    };
    if !b.taken && s == b.written {
        return;
    }
    b.taken = true;
    let r16 = |at: usize| u16::from_le_bytes([s[at], s[at + 1]]);
    let (width, height, width_bytes) = (r16(0x02) as i32, r16(0x04) as i32, r16(0x06));
    let (planes, bpp) = (s[0x08] as u16, s[0x09] as u16);
    let Some(bitmap) = cpu.bus.win32.gdi.bitmap(handle) else {
        return;
    };
    if (planes, bpp) != (bitmap.planes, bitmap.bpp) || width <= 0 || height <= 0 {
        return;
    }
    let stride = crate::win32::gdi32::surface::ddb_stride(width, bpp as u32 * planes as u32);
    let scan = (width_bytes as usize * planes as usize).min(stride);
    let (from, to) = (from.clamp(0, height), to.clamp(0, height));
    let mut rows = Vec::with_capacity((to - from) as usize);
    for r in from..to {
        let row = row_address(cpu, &s, r as u32).and_then(|at| crate::win16::peek(cpu, at, scan));
        rows.push(row);
    }
    let Some(bitmap) = cpu.bus.win32.gdi.bitmap_mut(handle) else {
        return;
    };
    if (bitmap.width, bitmap.height, bitmap.stride as usize) != (width, height, stride) {
        bitmap.width = width;
        bitmap.height = height;
        bitmap.stride = stride as u32;
        bitmap.bits.resize(stride * height as usize, 0);
    }
    for (r, row) in (from..to).zip(rows) {
        if let Some(row) = row {
            let at = r as usize * stride;
            bitmap.bits[at..at + row.len()].copy_from_slice(&row);
        }
    }
}

/// The task `task` ended, and its blocks with it: a view they were goes,
/// and the task's hint with it.
pub fn task_ended(cpu: &mut Cpu, task: u16) {
    if task == 0 {
        return;
    }
    for p in cpu.bus.win32.processes.iter_mut() {
        let view = &mut *p.gdi_heap;
        view.hinted.retain(|&t| t != task);
        if view.owner == task && view.segment != 0 {
            *view = GdiHeapView {
                hinted: std::mem::take(&mut view.hinted),
                ..GdiHeapView::default()
            };
        }
    }
}
