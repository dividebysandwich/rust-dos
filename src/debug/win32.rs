//! The built-in Windows for the debugger (`/api/win32`): its processes,
//! with their modules, threads and handles, and DirectDraw's surfaces
//! (`/api/win32/ddraw`).

use rust_dos::win32::snapshot::{
    Field, ObjectInfo, ProcessInfo, Snapshot, SurfaceInfo, ThreadInfo,
};
use serde_json::{Map, Value, json};

use crate::cpu::Cpu;

fn hex(v: u32) -> String {
    format!("{:08X}", v)
}

/// The built-in Windows as it is now.
pub fn win32_json(cpu: &Cpu) -> Value {
    let mut json = snapshot_json(&rust_dos::win32::snapshot(cpu));
    json["trace"] = json!(rust_dos::win32::trace::level(cpu).name());
    json["page_file"] = page_file_json(cpu);
    json
}

/// The page file, in pages: its size, the committed pages in it, the
/// physical pages that may go out to it, and since start the pages of
/// zeros given a physical page, those read back and those written out.
/// Null before the built-in Windows has its memory.
fn page_file_json(cpu: &Cpu) -> Value {
    rust_dos::win32::page_file_status(&cpu.bus).map_or(Value::Null, |p| {
        json!({
            "size": p.size,
            "used": p.used,
            "pageable": p.pageable,
            "zero_ins": p.counts.zero_ins,
            "ins": p.counts.ins,
            "outs": p.counts.outs,
        })
    })
}

/// The pages of the address space the processor runs in (CR3) that are
/// in the page file: runs of pages next to each other, each with its
/// first page's linear address, its length in pages, and whether they
/// are pages of zeros with no contents kept there. `truncated` when
/// there are more than `limit` runs. `ranges` is null before the
/// built-in Windows has its memory.
pub fn paged_json(cpu: &Cpu, limit: usize) -> Value {
    let Some((ranges, truncated)) = rust_dos::win32::paged_ranges(cpu, limit) else {
        return json!({"cr3": hex(cpu.cr3), "ranges": Value::Null, "truncated": false});
    };
    let ranges: Vec<Value> = ranges
        .iter()
        .map(|r| json!({"addr": hex(r.start), "pages": r.pages, "zeros": r.zeros}))
        .collect();
    json!({"cr3": hex(cpu.cr3), "ranges": ranges, "truncated": truncated})
}

pub fn snapshot_json(s: &Snapshot) -> Value {
    json!({
        "enabled": s.enabled,
        "active": s.active,
        "pending": s.pending,
        "processes": s.processes.iter().map(process_json).collect::<Vec<_>>(),
    })
}

fn process_json(p: &ProcessInfo) -> Value {
    let modules: Vec<Value> = p
        .modules
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "base": hex(m.base),
                "size": hex(m.size),
                "entry": (m.entry != 0).then(|| hex(m.entry)),
                "builtin": m.builtin,
            })
        })
        .collect();
    let handles: Vec<Value> = p
        .handles
        .iter()
        .map(|h| {
            let mut v = object_json(&h.object);
            v["handle"] = h.handle.into();
            v
        })
        .collect();
    json!({
        "id": hex(p.id),
        "name": p.name,
        "path": p.path,
        "command_line": p.command_line,
        "image_base": hex(p.image_base),
        "exit_code": p.exit_code.map(hex),
        "running": p.running,
        "root": p.root,
        "std": p.std,
        "modules": modules,
        "threads": p.threads.iter().map(thread_json).collect::<Vec<_>>(),
        "handles": handles,
    })
}

fn thread_json(t: &ThreadInfo) -> Value {
    let wait = t.wait.as_ref().map(|w| {
        let objects: Vec<Value> = w
            .objects
            .iter()
            .map(|(handle, object)| {
                let mut v = object_json(object);
                v["handle"] = json!(handle);
                v
            })
            .collect();
        json!({
            "objects": objects,
            "all": w.all,
            "timeout_ms": w.timeout_ms,
            "critical_section": (w.section != 0).then(|| hex(w.section)),
        })
    });
    json!({
        "id": hex(t.id),
        "state": t.state.name(),
        "registers": t.registers.name(),
        "cs": format!("{:04X}", t.cs),
        "eip": hex(t.eip),
        "esp": hex(t.esp),
        "tib": hex(t.tib),
        "fs": format!("{:04X}", t.selector),
        "stack": {"base": hex(t.stack.0), "top": hex(t.stack.1)},
        "last_error": t.last_error,
        "priority": t.priority,
        "suspend_count": t.suspended,
        "exit_code": t.exit_code.map(hex),
        "wait": wait,
    })
}

fn object_json(o: &ObjectInfo) -> Value {
    let state: Map<String, Value> = o
        .state
        .iter()
        .map(|(name, field)| {
            let v = match field {
                Field::Bool(b) => json!(b),
                Field::Number(n) => json!(n),
                Field::Id(0) => Value::Null,
                Field::Id(id) => json!(hex(*id)),
                Field::Code(code) => json!(code.map(hex)),
                Field::Text(text) => json!(text),
            };
            (name.to_string(), v)
        })
        .collect();
    json!({
        "object": o.id,
        "kind": o.kind,
        "state": state,
        "name": o.name,
        "refs": o.refs,
    })
}

/// DirectDraw's display mode and surfaces (`/api/win32/ddraw`).
pub fn ddraw_json(cpu: &Cpu) -> Value {
    let d = rust_dos::win32::snapshot::ddraw(cpu);
    json!({
        "mode": (d.mode.0 != 0).then(|| json!({"width": d.mode.0, "height": d.mode.1, "bits": d.mode.2})),
        "mode_owner": d.mode_owner,
        "exclusive": d.exclusive,
        "binds": json!({"all": d.binds[0], "kept": d.binds[1]}),
        "surfaces": d.surfaces.iter().map(surface_json).collect::<Vec<_>>(),
    })
}

fn surface_json(s: &SurfaceInfo) -> Value {
    let (flags, fourcc, bits, [r, g, b, a]) = s.format;
    let keys: Map<String, Value> = s
        .keys
        .iter()
        .map(|&(place, low, high)| (place.to_string(), json!([hex(low), hex(high)])))
        .collect();
    json!({
        "id": s.id,
        "pid": hex(s.pid),
        "interfaces": s.interfaces.iter().map(|&i| hex(i)).collect::<Vec<_>>(),
        "width": s.width,
        "height": s.height,
        "pitch": s.pitch,
        "format": {
            "flags": hex(flags),
            "fourcc": (fourcc != 0).then(|| hex(fourcc)),
            "bits": bits,
            "masks": [hex(r), hex(g), hex(b), hex(a)],
        },
        "caps": s.caps.map(hex),
        "memory": s.memory,
        "address": hex(s.address),
        "locked": s.locked,
        "lost": s.lost,
        "keys": keys,
        "palette": s.palette,
        "clipper": s.clipper,
        "root": s.root,
        "attached": s.attached,
        "version": s.version,
    })
}
