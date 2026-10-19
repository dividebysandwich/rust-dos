//! The built-in Windows for the debugger (`/api/win32`): its processes,
//! with their modules, threads and handles.

use rust_dos::win32::snapshot::{Field, ObjectInfo, ProcessInfo, Snapshot, ThreadInfo};
use serde_json::{Map, Value, json};

use crate::cpu::Cpu;

fn hex(v: u32) -> String {
    format!("{:08X}", v)
}

/// The built-in Windows as it is now.
pub fn win32_json(cpu: &Cpu) -> Value {
    snapshot_json(&rust_dos::win32::snapshot(cpu))
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
