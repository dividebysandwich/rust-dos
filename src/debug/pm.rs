//! Protected-mode views for the debugger: address parsing through
//! selectors and page tables, the descriptor tables, the TSS, page walks,
//! XMS handles and the exception log. Everything here only reads: nothing
//! faults, sets accessed bits or fills the TLB.

use serde_json::{Value, json};

use crate::cpu::fault::exception_name;
use crate::cpu::seg::{self, Descriptor};
use crate::cpu::{ATTR_DB, Cpu, Seg};

/// An address the debugger was given, resolved.
#[derive(Clone, Copy, Debug)]
pub struct DebugAddr {
    /// Physical address of the first byte (None: its page isn't mapped).
    pub phys: Option<usize>,
    /// The linear address, when the address was segmented or linear:
    /// following bytes are translated page by page.
    pub lin: Option<u32>,
    /// The segment or selector and offset, when given that way.
    pub segoff: Option<(u16, u32)>,
}

impl DebugAddr {
    /// Physical address of byte `i`, or None if its page isn't mapped.
    pub fn byte(&self, cpu: &Cpu, i: usize) -> Option<usize> {
        match self.lin {
            Some(lin) => cpu.peek_translate(lin.wrapping_add(i as u32)).map(|p| p as usize),
            None => self.phys.map(|p| p + i),
        }
    }

    /// `len` bytes from here; unmapped bytes read as FFh.
    pub fn read(&self, cpu: &Cpu, len: usize) -> Vec<u8> {
        (0..len).map(|i| self.byte(cpu, i).map_or(0xFF, |p| cpu.bus.peek_8(p))).collect()
    }
}

pub fn parse_hex(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    let t = t.strip_suffix('h').or_else(|| t.strip_suffix('H')).unwrap_or(t);
    u32::from_str_radix(t, 16).map_err(|_| format!("invalid hex number '{}'", s))
}

/// Value of a register named in an address: general-purpose (8 and 16-bit
/// forms of the 32-bit registers too), EIP/IP or a segment register.
fn register(cpu: &Cpu, name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    Some(match n.as_str() {
        "eax" => cpu.eax(),
        "ebx" => cpu.ebx(),
        "ecx" => cpu.ecx(),
        "edx" => cpu.edx(),
        "esi" => cpu.esi(),
        "edi" => cpu.edi(),
        "ebp" => cpu.ebp(),
        "esp" => cpu.esp(),
        "eip" => cpu.eip(),
        "ax" => cpu.ax() as u32,
        "bx" => cpu.bx() as u32,
        "cx" => cpu.cx() as u32,
        "dx" => cpu.dx() as u32,
        "si" => cpu.si() as u32,
        "di" => cpu.di() as u32,
        "bp" => cpu.bp() as u32,
        "sp" => cpu.sp() as u32,
        "ip" => cpu.ip() as u32,
        _ => return Seg::ALL.iter().find(|s| seg_name(**s) == n).map(|s| cpu.seg_cache(*s).selector as u32),
    })
}

fn seg_name(seg: Seg) -> &'static str {
    match seg {
        Seg::ES => "es",
        Seg::CS => "cs",
        Seg::SS => "ss",
        Seg::DS => "ds",
        Seg::FS => "fs",
        Seg::GS => "gs",
    }
}

/// The descriptor a selector names, read without side effects.
pub fn peek_descriptor(cpu: &Cpu, selector: u16) -> Option<Descriptor> {
    let (base, limit) = if selector & 4 != 0 {
        if cpu.ldtr.attr & 0x80 == 0 {
            return None;
        }
        (cpu.ldtr.base, cpu.ldtr.limit)
    } else {
        (cpu.gdtr.base, cpu.gdtr.limit as u32)
    };
    let index = (selector & 0xFFF8) as u32;
    if index + 7 > limit {
        return None;
    }
    peek_u64(cpu, base.wrapping_add(index)).map(Descriptor)
}

fn peek_u32(cpu: &Cpu, lin: u32) -> Option<u32> {
    let mut v = 0;
    for i in 0..4 {
        let p = cpu.peek_translate(lin.wrapping_add(i))?;
        v |= (cpu.bus.peek_8(p as usize) as u32) << (8 * i);
    }
    Some(v)
}

fn peek_u64(cpu: &Cpu, lin: u32) -> Option<u64> {
    Some(peek_u32(cpu, lin)? as u64 | (peek_u32(cpu, lin.wrapping_add(4))? as u64) << 32)
}

/// Base address and code size of a segment for an address: a segment
/// register named in it uses the register's cache; a number is a real-mode
/// segment, or in protected mode a selector looked up in the GDT or LDT.
fn segment(cpu: &Cpu, part: &str) -> Result<(u16, u32, bool), String> {
    let p = part.trim().to_ascii_lowercase();
    if let Some(seg) = Seg::ALL.iter().find(|s| seg_name(**s) == p) {
        let c = cpu.seg_cache(*seg);
        return Ok((c.selector, c.base, c.attr & ATTR_DB != 0));
    }
    let value = parse_hex(part)?;
    let sel = u16::try_from(value).map_err(|_| format!("segment '{}' exceeds 16 bits", part))?;
    if !cpu.pm() {
        return Ok((sel, (sel as u32) << 4, false));
    }
    let desc = peek_descriptor(cpu, sel).ok_or_else(|| format!("selector {:04X} is outside its table", sel))?;
    Ok((sel, desc.base(), desc.attr() & ATTR_DB != 0))
}

/// Parse an address (hex numbers, as in DEBUG.COM):
///
/// * `SEG:OFF`: a real-mode segment, or in protected mode a selector, and
///   an offset of up to 32 bits. Either part may be a register name:
///   `CS:EIP`, `DS:ESI`, `SS:SP`.
/// * `lin:ADDR`: a linear address, translated through the page tables.
/// * `phys:ADDR`, or just `ADDR`: a physical address.
pub fn parse_addr(cpu: &Cpu, s: &str) -> Result<DebugAddr, String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("lin:") {
        let lin = parse_hex(rest)?;
        return Ok(DebugAddr { phys: cpu.peek_translate(lin).map(|p| p as usize), lin: Some(lin), segoff: None });
    }
    if let Some(rest) = lower.strip_prefix("phys:") {
        return physical(cpu, rest);
    }
    if let Some((seg, off)) = s.split_once(':') {
        let (sel, base, _) = segment(cpu, seg)?;
        let off = match register(cpu, off.trim()) {
            Some(v) => v,
            None => parse_hex(off)?,
        };
        let lin = if cpu.pm() { base.wrapping_add(off) } else { ((sel as u32) << 4).wrapping_add(off) };
        return Ok(DebugAddr {
            phys: cpu.peek_translate(lin).map(|p| p as usize),
            lin: Some(lin),
            segoff: Some((sel, off)),
        });
    }
    physical(cpu, s)
}

fn physical(cpu: &Cpu, s: &str) -> Result<DebugAddr, String> {
    let v = parse_hex(s)? as usize;
    if v >= cpu.bus.ram().len() {
        return Err(format!("address {:X} beyond the end of RAM ({:X})", v, cpu.bus.ram().len()));
    }
    Ok(DebugAddr { phys: Some(v), lin: None, segoff: None })
}

/// Whether code at `sel` runs as 32-bit code.
pub fn code32(cpu: &Cpu, sel: u16) -> bool {
    if sel == cpu.cs() {
        return cpu.seg_cache(Seg::CS).attr & ATTR_DB != 0;
    }
    cpu.pm() && peek_descriptor(cpu, sel).is_some_and(|d| d.attr() & ATTR_DB != 0)
}

/// The processor mode, as the debugger reports it.
pub fn mode_name(cpu: &Cpu) -> &'static str {
    if cpu.v86() {
        "v86"
    } else if cpu.pe() {
        "protected"
    } else {
        "real"
    }
}

/// The registers protected mode adds: mode, CPL, control registers,
/// descriptor table registers and the segment register caches.
pub fn system_regs(cpu: &Cpu) -> Value {
    let h32 = |v: u32| format!("{:08X}", v);
    let cache = |c: &crate::cpu::SegCache| {
        json!({
            "selector": format!("{:04X}", c.selector),
            "base": h32(c.base),
            "limit": h32(c.limit),
            "attr": format!("{:04X}", c.attr),
        })
    };
    let mut segs = serde_json::Map::new();
    for seg in Seg::ALL {
        segs.insert(seg_name(seg).into(), cache(cpu.seg_cache(seg)));
    }
    json!({
        "mode": mode_name(cpu),
        "cpl": cpu.cpl,
        "code32": cpu.seg_cache(Seg::CS).attr & ATTR_DB != 0,
        "cr2": h32(cpu.cr2),
        "cr3": h32(cpu.cr3),
        "gdtr": {"base": h32(cpu.gdtr.base), "limit": format!("{:04X}", cpu.gdtr.limit)},
        "idtr": {"base": h32(cpu.idtr.base), "limit": format!("{:04X}", cpu.idtr.limit)},
        "ldtr": cache(&cpu.ldtr),
        "tr": cache(&cpu.tr),
        "segments": segs,
    })
}

/// A descriptor, decoded.
pub fn descriptor_json(selector: u16, d: &Descriptor) -> Value {
    let h32 = |v: u32| format!("{:08X}", v);
    let mut v = json!({
        "selector": format!("{:04X}", selector),
        "raw": format!("{:016X}", d.0),
        "present": d.present(),
        "dpl": d.dpl(),
    });
    if d.is_segment() {
        let kind = if d.is_code() {
            format!("code{}{}", if d.conforming() { " conforming" } else { "" }, if d.readable() { " readable" } else { "" })
        } else {
            format!(
                "data{}{}",
                if d.typ() & 4 != 0 { " expand-down" } else { "" },
                if d.writable_data() { " writable" } else { " read-only" }
            )
        };
        v["type"] = kind.into();
        v["base"] = h32(d.base()).into();
        v["limit"] = h32(d.limit()).into();
        v["bits"] = if d.attr() & ATTR_DB != 0 { 32 } else { 16 }.into();
        v["accessed"] = d.accessed().into();
    } else {
        let name = match d.typ() {
            seg::TSS16_AVAILABLE => "tss16",
            seg::LDT => "ldt",
            seg::TSS16_BUSY => "tss16 busy",
            seg::CALL_GATE16 => "call gate16",
            seg::TASK_GATE => "task gate",
            seg::INT_GATE16 => "interrupt gate16",
            seg::TRAP_GATE16 => "trap gate16",
            seg::TSS32_AVAILABLE => "tss32",
            seg::TSS32_BUSY => "tss32 busy",
            seg::CALL_GATE32 => "call gate32",
            seg::INT_GATE32 => "interrupt gate32",
            seg::TRAP_GATE32 => "trap gate32",
            _ => "reserved",
        };
        v["type"] = name.into();
        match d.typ() {
            seg::CALL_GATE16 | seg::CALL_GATE32 | seg::INT_GATE16 | seg::INT_GATE32 | seg::TRAP_GATE16
            | seg::TRAP_GATE32 => {
                v["target"] = format!("{:04X}:{:08X}", d.gate_selector(), d.gate_offset()).into();
                if matches!(d.typ(), seg::CALL_GATE16 | seg::CALL_GATE32) {
                    v["params"] = d.gate_params().into();
                }
            }
            seg::TASK_GATE => v["tss"] = format!("{:04X}", d.gate_selector()).into(),
            _ => {
                v["base"] = h32(d.base()).into();
                v["limit"] = h32(d.limit()).into();
            }
        }
    }
    v
}

/// The GDT or the LDT: every descriptor that isn't all zeros, up to `max`.
pub fn table_json(cpu: &Cpu, ldt: bool, max: usize) -> Value {
    let (base, limit) = if ldt {
        if cpu.ldtr.attr & 0x80 == 0 {
            return json!({"error": "no LDT loaded", "entries": []});
        }
        (cpu.ldtr.base, cpu.ldtr.limit)
    } else {
        (cpu.gdtr.base, cpu.gdtr.limit as u32)
    };
    let mut entries = Vec::new();
    let mut index = 0u32;
    while index + 7 <= limit && entries.len() < max {
        if let Some(raw) = peek_u64(cpu, base.wrapping_add(index))
            && raw != 0
        {
            let selector = index as u16 | if ldt { 4 } else { 0 };
            entries.push(descriptor_json(selector, &Descriptor(raw)));
        }
        index += 8;
    }
    json!({
        "base": format!("{:08X}", base),
        "limit": format!("{:X}", limit),
        "entries": entries,
    })
}

/// The protected-mode IDT: every present gate.
pub fn idt_json(cpu: &Cpu) -> Value {
    let mut entries = Vec::new();
    for vector in 0..=255u32 {
        if vector * 8 + 7 > cpu.idtr.limit as u32 {
            break;
        }
        if let Some(raw) = peek_u64(cpu, cpu.idtr.base.wrapping_add(vector * 8)) {
            let d = Descriptor(raw);
            if d.present() {
                let mut v = descriptor_json(vector as u16 * 8, &d);
                v["vector"] = format!("{:02X}", vector).into();
                if let Some(o) = v.as_object_mut() {
                    o.remove("selector");
                }
                entries.push(v);
            }
        }
    }
    json!({
        "base": format!("{:08X}", cpu.idtr.base),
        "limit": format!("{:04X}", cpu.idtr.limit),
        "gates": entries,
    })
}

/// The current TSS's contents.
pub fn tss_json(cpu: &Cpu) -> Value {
    let tr = cpu.tr;
    if tr.attr & 0x80 == 0 {
        return json!({"error": "no TSS loaded (TR is null)"});
    }
    let rd = |off: u32| peek_u32(cpu, tr.base.wrapping_add(off)).unwrap_or(0xFFFF_FFFF);
    let h32 = |v: u32| format!("{:08X}", v);
    let h16 = |v: u32| format!("{:04X}", v & 0xFFFF);
    if tr.attr & 0x08 != 0 {
        let names = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
        let mut regs = serde_json::Map::new();
        for (i, n) in names.iter().enumerate() {
            regs.insert(n.to_string(), h32(rd(0x28 + 4 * i as u32)).into());
        }
        let segs = ["es", "cs", "ss", "ds", "fs", "gs"];
        for (i, n) in segs.iter().enumerate() {
            regs.insert(n.to_string(), h16(rd(0x48 + 4 * i as u32)).into());
        }
        json!({
            "selector": format!("{:04X}", tr.selector),
            "type": "tss32",
            "base": h32(tr.base),
            "limit": h32(tr.limit),
            "back_link": h16(rd(0)),
            "esp0": h32(rd(4)), "ss0": h16(rd(8)),
            "esp1": h32(rd(0xC)), "ss1": h16(rd(0x10)),
            "esp2": h32(rd(0x14)), "ss2": h16(rd(0x18)),
            "cr3": h32(rd(0x1C)),
            "eip": h32(rd(0x20)),
            "eflags": h32(rd(0x24)),
            "registers": regs,
            "ldt": h16(rd(0x60)),
            "iomap_base": h16(rd(0x64) >> 16),
        })
    } else {
        json!({
            "selector": format!("{:04X}", tr.selector),
            "type": "tss16",
            "base": h32(tr.base),
            "limit": h32(tr.limit),
            "back_link": h16(rd(0)),
            "sp0": h16(rd(2)), "ss0": h16(rd(4)),
            "sp1": h16(rd(6)), "ss1": h16(rd(8)),
            "sp2": h16(rd(0xA)), "ss2": h16(rd(0xC)),
            "ip": h16(rd(0xE)),
            "flags": h16(rd(0x10)),
            "ldt": h16(rd(0x2A)),
        })
    }
}

/// The page directory and table entries for a linear address.
pub fn pagewalk_json(cpu: &Cpu, lin: u32) -> Value {
    let flags = |e: u32| {
        let mut f = Vec::new();
        for (bit, name) in [(1, "P"), (2, "RW"), (4, "US"), (0x20, "A"), (0x40, "D")] {
            if e & bit != 0 {
                f.push(name);
            }
        }
        f
    };
    let paging = cpu.cr0 & crate::cpu::CR0_PG != 0;
    let (pde_addr, pde, pte) = cpu.page_walk(lin);
    let mut v = json!({
        "linear": format!("{:08X}", lin),
        "paging": paging,
        "cr3": format!("{:08X}", cpu.cr3),
        "pde": {"addr": format!("{:08X}", pde_addr), "value": format!("{:08X}", pde), "flags": flags(pde)},
        "physical": cpu.peek_translate(lin).map(|p| format!("{:08X}", p)),
    });
    if let Some((addr, e)) = pte {
        v["pte"] = json!({"addr": format!("{:08X}", addr), "value": format!("{:08X}", e), "flags": flags(e)});
    }
    v
}

/// XMS handles and the A20 gate.
pub fn xms_json(cpu: &Cpu) -> Value {
    let handles: Vec<Value> = cpu
        .bus
        .xms
        .handles()
        .into_iter()
        .map(|(handle, base, kb, locks)| {
            json!({
                "handle": handle,
                "base": format!("{:08X}", base),
                "size_kb": kb,
                "locks": locks,
            })
        })
        .collect();
    json!({
        "a20": cpu.bus.a20(),
        "hma_allocated": cpu.bus.xms.hma_allocated(),
        "handles": handles,
    })
}

/// The most recent exceptions, oldest first.
pub fn exceptions_json(cpu: &Cpu) -> Value {
    let list: Vec<Value> = cpu
        .exception_log
        .iter()
        .map(|e| {
            let mut v = json!({
                "vector": format!("{:02X}", e.vector),
                "name": exception_name(e.vector),
                "at": format!("{:04X}:{:08X}", e.cs, e.eip),
                "protected": e.protected,
                "icount": e.icount,
            });
            if let Some(code) = e.error.filter(|_| e.protected) {
                v["error"] = format!("{:04X}", code).into();
            }
            if e.vector == 14 {
                v["cr2"] = format!("{:08X}", e.cr2).into();
            }
            v
        })
        .collect();
    json!({"total": cpu.exceptions, "mode_switches": cpu.mode_switches, "recent": list})
}
