//! Status and JSON views of the machine for replies.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::trace::TraceEntry;
use super::{DebugHub, Need, Reply, TraceQuery, Watch, parse_addr, parse_hex, pm, trace};
use crate::cpu::{Cpu, CpuFlags};
use crate::disk::drive_letter;
use crate::mount::display_host_path;
use crate::video::{self, VideoMode};

impl DebugHub {
    pub(super) fn status(&self, cpu: &Cpu) -> Value {
        let (w, h) = cpu.bus.display_size();
        let (frame_w, frame_h) = video::frame_size(&cpu.bus);
        let crtc = &cpu.bus.vga.crtc_regs;
        let timing = cpu.bus.vga.peek_timing();
        let mut status = json!({
            "paused": self.paused,
            "settings_window": self.divert,
            "icount": cpu.executed,
            "uptime_ms": cpu.bus.start_time.elapsed().as_millis() as u64,
            "fps": (self.fps * 10.0).round() / 10.0,
            "cycles_per_ms": cpu.bus.clock.cycles_per_ms(),
            // Deterministic mode: the clock's start, the machine's clock
            // now, emulated time, and every how many emulated ms input is
            // delivered. Null when off.
            "deterministic": self.deterministic.map(|start| json!({
                "start_time": start.format("%Y-%m-%d %H:%M:%S").to_string(),
                "clock": cpu.bus.cmos.now().format("%Y-%m-%d %H:%M:%S").to_string(),
                "emulated_ms": cpu.bus.clock.now_ns() / 1_000_000,
                "input_tick_ms": rust_dos::deterministic::TICK_MS,
                "until_ms": self.stop_at_ns.map(|ns| ns / 1_000_000),
            })),
            // What `cycles=auto` goes by, counted since start: emulated
            // time, the frames the program drew (at most one a retrace),
            // its bursts of writes to video memory the size of a frame and
            // its page flips (the frames it draws, however many), its
            // checks for keys that weren't there and idle calls, and its
            // reads of the input status register (retrace polling).
            "activity": {
                "emulated_ns": cpu.bus.clock.now_ns(),
                "frames_drawn": cpu.bus.frames_drawn,
                "bursts": cpu.bus.activity.bursts,
                "flips": cpu.bus.activity.flips,
                "polls": cpu.bus.activity.polls,
                "status_reads": cpu.bus.activity.status_reads,
            },
            "cs_ip": cs_ip(cpu),
            "cpu_mode": pm::mode_name(cpu),
            "cpu_state": format!("{:?}", cpu.state),
            "shell_idle": cpu.shell_idle(),
            "process_depth": cpu.process_stack.len(),
            "current_psp": format!("{:04X}", cpu.current_psp),
            "video": {
                // The display adapter programs see (`machine`).
                "adapter": cpu.bus.vga.adapter.name(),
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
                // The picture as screenshots have it.
                "frame": {"width": frame_w, "height": frame_h},
                // Page flipping: the Start Address the program set and the
                // one latched for display, plus the registers that decide
                // the memory layout.
                "vga": {
                    "start_address": format!("{:02X}{:02X}", crtc[0x0C], crtc[0x0D]),
                    "displayed_start": format!("{:04X}", cpu.bus.vga.latched_start_addr),
                    "seq_memory_mode": format!("{:02X}", cpu.bus.vga.sequencer_regs[0x04]),
                    "crtc_offset": format!("{:02X}", crtc[0x13]),
                    "crtc_underline": format!("{:02X}", crtc[0x14]),
                    "crtc_mode_control": format!("{:02X}", crtc[0x17]),
                    "crtc": crtc.iter().map(|r| format!("{:02X}", r)).collect::<Vec<_>>().join(" "),
                    "attribute": cpu.bus.vga.attribute_regs.iter().map(|r| format!("{:02X}", r)).collect::<Vec<_>>().join(" "),
                    // The DAC's first 64 colours, six bits a component.
                    "dac": cpu.bus.vga.palette[..192].chunks(3).map(|c| format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])).collect::<Vec<_>>().join(" "),
                },
                // The VESA mode, when one is set.
                "vbe": cpu.bus.vbe.mode.filter(|_| cpu.bus.video_mode == VideoMode::Vesa).map(|mode| json!({
                    "mode": format!("{:03X}", mode.number),
                    "width": mode.width, "height": mode.height, "bpp": mode.bpp,
                    "lfb": cpu.bus.vbe.lfb,
                    "bank": cpu.bus.vbe.bank,
                    "pitch": cpu.bus.vbe.pitch,
                    "display_start": format!("{:X}", cpu.bus.vbe.latched_start),
                    "dac_bits": if cpu.bus.vga.dac_8bit { 8 } else { 6 },
                })),
                // An S3's extended CRTC registers (30h-6Fh, a row of 16
                // each) and its hardware cursor.
                "voodoo": cpu.bus.voodoo_status(),
                "powervr": cpu.bus.powervr_status(),
                "verite": cpu.bus.verite().then(|| cpu.bus.verite.describe()),
                "s3": cpu.bus.vga.adapter.is_s3().then(|| {
                    let s3 = &cpu.bus.vga.s3;
                    json!({
                        "crtc": (0x30..0x70usize).step_by(16).map(|row| {
                            (row..row + 16).map(|i| format!("{:02X}", s3.crtc(i as u8))).collect::<Vec<_>>().join(" ")
                        }).collect::<Vec<_>>(),
                        "cursor": s3.cursor().map(|c| json!({
                            "x": c.x, "y": c.y, "skip_x": c.skip_x, "skip_y": c.skip_y,
                            "address": format!("{:X}", c.address), "x11": c.x11,
                        })),
                        // A ViRGE's engines: the commands they ran, and the
                        // last 3D command.
                        // A ViRGE's engines.
                        "virge": cpu.bus.vga.adapter.is_virge().then(|| virge_status(&cpu.bus.virge, cpu.bus.vga.adapter == crate::video::adapter::Adapter::S3VirgeVx)),
                    })
                }),
                // An ET4000's extended registers: CR30-3F, the KEY, the
                // segment select (write bank low, read bank high) and the
                // Sierra DAC's command register.
                "et4000": cpu.bus.vga.adapter.is_et4000().then(|| {
                    let chip = &cpu.bus.vga.et4000;
                    json!({
                        "crtc": (0x30..0x40u8).map(|i| format!("{:02X}", chip.crtc(i))).collect::<Vec<_>>().join(" "),
                        "keyed": chip.keyed,
                        "segment": format!("{:02X}", chip.segment),
                        "dac_command": format!("{:02X}", chip.dac_command),
                        "hicolor": chip.hicolor(),
                        "vbe_mode": (chip.vbe_mode != 0).then(|| format!("{:03X}", chip.vbe_mode)),
                    })
                }),
                // The display timing programs see through port 3DAh.
                "crt": {
                    "hz": (timing.hz() * 100.0).round() / 100.0,
                    "lines": timing.total,
                    "display_lines": timing.display,
                },
            },
            "drive_c": cpu
                .bus
                .disk
                .drive_info(crate::disk::DRIVE_C)
                .and_then(|info| info.root.or(info.image))
                .map(|path| display_host_path(&path)),
            "current_drive": drive_letter(cpu.bus.disk.get_current_drive()).to_string(),
            "trace": self.trace_status(),
            "breakpoints": self.breakpoints.len(),
            "watchpoints": self.watchpoints.len(),
            "input_queue": self.input_pending(),
            "keyboard_buffer": cpu.bus.keyboard_buffer.len(),
            // The interrupt controllers: requests waiting, masked lines and
            // lines in service (bit n = line n of that chip).
            "pic": {
                "vectors": [format!("{:02X}", cpu.bus.pic.master.base), format!("{:02X}", cpu.bus.pic.slave.base)],
                "irr": [format!("{:02X}", cpu.bus.pic.master.irr), format!("{:02X}", cpu.bus.pic.slave.irr)],
                "imr": [format!("{:02X}", cpu.bus.pic.master.imr), format!("{:02X}", cpu.bus.pic.slave.imr)],
                "isr": [format!("{:02X}", cpu.bus.pic.master.isr), format!("{:02X}", cpu.bus.pic.slave.isr)],
            },
            "audio": {
                "peak": cpu.bus.audio_peak,
                "underruns": cpu.bus.audio_underruns,
                "frames": cpu.bus.audio_frames(),
                "queued_frames": cpu.bus.audio_device.as_ref().map_or(0, |d| d.queued_frames()),
                "sound_blaster": cpu.bus.sb_blaster(),
                "opl3": cpu.bus.opl.is_opl3(),
                "ultrasound": cpu.bus.gus.as_ref().map(|gus| gus.config.ultrasnd()),
                "midi_synth": cpu.bus.mpu.synth_name(),
                "midi_voices": cpu.bus.mpu.gus_synth().map(|s| s.active_voices()),
                "missing_patches": cpu.bus.mpu.gus_synth().map(|s| s.missing_patches()),
                // CD audio from an image drive (MSCDEX Play Audio).
                "cd": cpu.bus.cdaudio.drive().map(|drive| {
                    let (m, s, f) = rust_dos::cdrom::lba_to_msf(cpu.bus.cdaudio.position());
                    json!({
                        "drive": drive_letter(drive).to_string(),
                        "state": format!("{:?}", cpu.bus.cdaudio.state()).to_ascii_lowercase(),
                        "track": cpu.bus.cdaudio.track(),
                        "position": format!("{:02}:{:02}:{:02}", m, s, f),
                    })
                }),
            },
            // The keyboard controller: its output buffer and queues.
            "kbc": cpu.bus.kbc.debug_json(),
            "mouse": {
                "installed": cpu.bus.mouse.installed,
                "x": cpu.bus.mouse.x, "y": cpu.bus.mouse.y,
                "buttons": cpu.bus.mouse.buttons,
                "visible": cpu.bus.mouse.hide_counter <= 0,
                "event_handler": format!(
                    "{:04X}:{:04X} mask {:04X}",
                    cpu.bus.mouse.callback_cs, cpu.bus.mouse.callback_ip, cpu.bus.mouse.callback_mask
                ),
                "ps2": {
                    "enabled": cpu.bus.mouse.ps2.enabled,
                    "handler": format!("{:04X}:{:04X}", cpu.bus.mouse.ps2.handler.0, cpu.bus.mouse.ps2.handler.1),
                    "rate": cpu.bus.mouse.ps2.rate,
                },
            },
        });
        // The program running (empty at the prompt), and how the last one
        // to end did.
        status["program"] = cpu.program.clone().into();
        status["last_exit"] = exit_json(cpu);
        // The journal: its last and oldest entries, the last that changed
        // the machine, the stop it is in, the control operation under way,
        // and whether a strict-match save could be done now.
        if let Some(shared) = &self.shared {
            let (last, first, state_seq) = self.with_journal(|j| (j.last(), j.first(), j.state_seq)).unwrap_or_default();
            status["seq"] = last.into();
            status["first_seq"] = first.into();
            status["state_seq"] = state_seq.into();
            status["stop"] = self.current_stop().unwrap_or(Value::Null);
            status["op"] = self.current_op.into();
            status["quiescent"] = self.check(Need::Quiescent { input_ok: false }).is_ok().into();
            status["instance"] = shared.instance_hex().into();
        }
        status
    }

    pub(super) fn trace_status(&self) -> Value {
        json!({
            "enabled": self.trace_enabled,
            "active": self.tracing_now,
            "entries": self.trace.len(),
            "capacity": self.trace.capacity(),
            "total_recorded": self.trace.total(),
            // The instructions a `count` still has to record.
            "remaining": self.trace_until.map(|n| n.saturating_sub(self.trace.total())),
            "stream_max_per_frame": self.trace_stream_max,
        })
    }

    pub(super) fn trace_query(&self, cpu: &Cpu, q: TraceQuery) -> Reply {
        let cs_filter = match q.cs.as_deref().map(parse_hex) {
            Some(Ok(v)) => Some(v as u16),
            Some(Err(e)) => return Reply::bad(e),
            None => None,
        };
        let now_us = cpu.bus.start_time.elapsed().as_micros() as u64;
        let (from_us, to_us) = if let Some(last) = q.last_ms {
            (now_us.saturating_sub((last * 1000.0) as u64), u64::MAX)
        } else {
            (
                q.from_ms.map_or(0, |v| (v * 1000.0) as u64),
                q.to_ms.map_or(u64::MAX, |v| (v * 1000.0) as u64),
            )
        };
        // `last_n` asks for that many, and `limit` caps it only when given.
        // A page read with `since` holds up to `limit`, 10000 unless given.
        let limit = match (q.last_n, q.limit) {
            (Some(n), Some(limit)) => n.min(limit),
            (Some(n), None) if q.since.is_none() => n,
            (last_n, limit) => last_n.unwrap_or(usize::MAX).min(limit.unwrap_or(10_000)),
        };
        let matches = |e: &TraceEntry| {
            e.t_us >= from_us
                && e.t_us <= to_us
                && cs_filter.is_none_or(|cs| e.cs == cs)
                && !(q.no_bios && e.is_bios_code())
        };
        if let Some(since) = q.since {
            // A cursor past the end (from an earlier run of the emulator)
            // reads from the end, so new entries are not skipped until the
            // total catches up with it.
            let since = since.min(self.trace.total());
            // Oldest first, so the next page goes on where this one ends.
            let (after, dropped) = self.trace.after(since);
            let mut next = since.max(self.trace.total() - self.trace.len() as u64);
            let mut entries = Vec::new();
            let mut truncated = false;
            for (n, e) in after {
                if entries.len() == limit {
                    truncated = true;
                    break;
                }
                next = n;
                if matches(e) {
                    entries.push(*e);
                }
            }
            return Reply::Trace { entries, next, dropped, truncated };
        }
        // Newest-first collection so `limit` keeps the most recent entries.
        // One more than `limit` tells whether `limit` left out entries that
        // `last_n` (or a window) asked for.
        let mut entries: Vec<TraceEntry> =
            self.trace.iter().rev().filter(|e| matches(e)).take(limit.saturating_add(1)).copied().collect();
        let truncated = entries.len() > limit && q.last_n.is_none_or(|n| n > limit);
        entries.truncate(limit);
        entries.reverse();
        Reply::Trace { entries, next: self.trace.total(), dropped: 0, truncated }
    }

    pub(super) fn disasm(&self, cpu: &Cpu, addr: Option<String>, count: usize) -> Reply {
        let addr = addr.unwrap_or_else(|| "CS:EIP".to_string());
        let start = match parse_addr(cpu, &addr) {
            Ok(a) => a,
            Err(e) => return Reply::bad(e),
        };
        let code32 = start.segoff.is_some_and(|(sel, _)| pm::code32(cpu, sel));
        let cur = cpu.peek_translate(cpu.seg_cache(crate::cpu::Seg::CS).base.wrapping_add(cpu.eip())).map(|p| p as usize);
        let mut lines = Vec::new();
        let mut rows = Vec::new();
        let mut at = 0usize;
        for _ in 0..count.min(1000) {
            let p = start.byte(cpu, at);
            let bytes: Vec<u8> = (0..15)
                .map(|i| start.byte(cpu, at + i).map_or(0xFF, |a| cpu.bus.peek_8(a)))
                .collect();
            let (len, text) = if let Some(name) = trace::trap_name(&bytes, None, cpu.pm()) {
                (3, name)
            } else {
                let ip = start.segoff.map_or(at as u32, |(_, off)| off.wrapping_add(at as u32));
                trace::disasm_one(&bytes, ip, code32)
            };
            let current = p.is_some() && p == cur;
            let breakpoint = p.is_some_and(|p| self.breakpoints.contains(&p));
            let marker = match (current, breakpoint) {
                (true, true) => "=>*",
                (true, false) => "=> ",
                (false, true) => "  *",
                _ => "   ",
            };
            let label = match start.segoff {
                Some((sel, off)) if code32 || off > 0xFFFF => format!("{:04X}:{:08X}", sel, off.wrapping_add(at as u32)),
                Some((sel, off)) => format!("{:04X}:{:04X}", sel, (off as usize + at) as u16),
                None => format!("{:08X}", start.lin.map_or(start.phys.unwrap_or(0) + at, |l| l as usize + at)),
            };
            let hex = trace::hex_bytes(&bytes[..len]);
            lines.push(format!("{} {}  {:<20} {}", marker, label, hex, text));
            rows.push(json!({
                "label": label,
                "phys": p.map(|p| format!("{:05X}", p)),
                "bytes": hex,
                "asm": text,
                "len": len,
                "current": current,
                "breakpoint": breakpoint,
            }));
            at += len;
        }
        Reply::Json(json!({"lines": lines, "rows": rows}))
    }

    pub(super) fn watchpoints_json(&self, cpu: &Cpu) -> Value {
        let watches: Vec<Value> = self
            .watchpoints
            .iter()
            .map(|w| {
                json!({
                    "addr": format!("{:05X}", w.phys),
                    "len": w.len,
                    "value": format!("{:0width$X}", Watch::read(cpu, w.phys, w.len), width = w.len as usize * 2),
                })
            })
            .collect();
        json!({"watchpoints": watches})
    }

    pub(super) fn breakpoints_json(&self) -> Value {
        let mut v: Vec<_> = self.breakpoints.iter().copied().collect();
        v.sort();
        let mut once: Vec<_> = self.once.iter().copied().collect();
        once.sort();
        let exceptions: Vec<String> =
            (0..32).filter(|i| self.break_exceptions & (1 << i) != 0).map(|i| format!("{:02X}", i)).collect();
        json!({
            "breakpoints": v.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>(),
            // The breakpoints among them that go once they stop the machine.
            "once": once.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>(),
            "exceptions": exceptions,
            "mode_switch": self.break_mode_switch,
            "program_start": self.break_program_start,
            "program_exit": self.break_program_exit,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------


/// The fields of a stop as replies have them since before the journal
/// (`paused`, `reason`, `icount`, `registers` and the rest), and the whole
/// `paused` entry as `stop`.
pub(crate) fn stop_fields(stop: &Value) -> Value {
    let mut reply = json!({"paused": true});
    if let Value::Object(entry) = stop {
        for (k, v) in entry {
            if !matches!(k.as_str(), "seq" | "type" | "t_ms" | "emu_ns" | "cause" | "request_id" | "op") {
                reply[k] = v.clone();
            }
        }
    }
    reply["stop"] = stop.clone();
    reply
}

/// The reply to a pause or a wait: `stop_fields`, with the instruction
/// count and the registers as they are now, which a write after the stop
/// may have changed.
pub(super) fn stop_reply(stop: Option<Value>, cpu: &Cpu) -> Value {
    let mut reply = stop_fields(&stop.unwrap_or(Value::Null));
    reply["icount"] = cpu.executed.into();
    reply["registers"] = regs_json(cpu);
    reply
}

/// Bytes as a `memory_written` entry keeps them: hex up to 256 bytes, else
/// their SHA-256.
pub(super) fn bytes_record(bytes: &[u8]) -> Value {
    use sha2::Digest;
    if bytes.len() <= 256 {
        trace::hex_bytes(bytes).into()
    } else {
        let digest = sha2::Sha256::digest(bytes);
        json!({"sha256": digest.iter().map(|b| format!("{:02x}", b)).collect::<String>()})
    }
}

/// A load's `folders` as its request had them: the host folder by drive letter.
pub fn folders_json(folders: &[(u8, PathBuf)]) -> Value {
    let map: Map<String, Value> = folders
        .iter()
        .map(|(drive, folder)| (crate::disk::drive_key(*drive), folder.display().to_string().into()))
        .collect();
    Value::Object(map)
}

/// CS:IP, or CS:EIP past FFFFh.
pub(super) fn cs_ip(cpu: &Cpu) -> String {
    if cpu.eip() > 0xFFFF {
        format!("{:04X}:{:08X}", cpu.cs(), cpu.eip())
    } else {
        format!("{:04X}:{:04X}", cpu.cs(), cpu.ip())
    }
}

/// Whether the shell waits at its prompt with nothing to run: no program,
/// no batch line or command queued, no PAUSE, CHOICE or EDIT open.
pub(super) fn at_idle_prompt(cpu: &Cpu) -> bool {
    cpu.shell_idle() && !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_wait.is_none()
}

/// The program started last: its name and entry point.
pub(super) fn program_json(cpu: &Cpu) -> Value {
    let p = &cpu.programs;
    json!({
        "name": p.name,
        "entry": format!("{:04X}:{:04X}", p.entry_cs_ip.0, p.entry_cs_ip.1),
        "psp": format!("{:04X}", p.psp),
    })
}

/// How the program that ended last did, or null before any did.
pub(super) fn exit_json(cpu: &Cpu) -> Value {
    match &cpu.programs.last_exit {
        Some(e) => json!({"name": e.name, "code": e.code, "resident": e.resident, "aborted": e.code.is_none()}),
        None => Value::Null,
    }
}

/// 16-bit registers and segment registers `set_regs` accepts.
fn reg16(name: &str) -> bool {
    matches!(name, "ax" | "bx" | "cx" | "dx" | "si" | "di" | "bp" | "sp" | "cs" | "ds" | "es" | "ss" | "ip")
}

pub fn regs_json(cpu: &Cpu) -> Value {
    let flags = cpu.get_cpu_flags();
    let names: Vec<&str> = [
        (CpuFlags::OF, "OF"),
        (CpuFlags::DF, "DF"),
        (CpuFlags::IF, "IF"),
        (CpuFlags::TF, "TF"),
        (CpuFlags::SF, "SF"),
        (CpuFlags::ZF, "ZF"),
        (CpuFlags::AF, "AF"),
        (CpuFlags::PF, "PF"),
        (CpuFlags::CF, "CF"),
    ]
    .iter()
    .filter(|(f, _)| flags.contains(*f))
    .map(|(_, n)| *n)
    .collect();
    let h = |v: u16| format!("{:04X}", v);
    let h32 = |v: u32| format!("{:08X}", v);
    json!({
        "ax": h(cpu.ax()), "bx": h(cpu.bx()), "cx": h(cpu.cx()), "dx": h(cpu.dx()),
        "si": h(cpu.si()), "di": h(cpu.di()), "bp": h(cpu.bp()), "sp": h(cpu.sp()),
        "eax": h32(cpu.eax()), "ebx": h32(cpu.ebx()), "ecx": h32(cpu.ecx()), "edx": h32(cpu.edx()),
        "esi": h32(cpu.esi()), "edi": h32(cpu.edi()), "ebp": h32(cpu.ebp()), "esp": h32(cpu.esp()),
        "cs": h(cpu.cs()), "ds": h(cpu.ds()), "es": h(cpu.es()), "ss": h(cpu.ss()),
        "fs": h(cpu.fs()), "gs": h(cpu.gs()),
        "ip": h(cpu.ip()), "eip": h32(cpu.eip()),
        "flags": h(flags.bits() as u16), "eflags": h32(flags.bits()),
        "flags_set": names,
        "cr0": h32(cpu.cr0),
        "system": pm::system_regs(cpu),
    })
}

/// The FPU, MMX and SSE registers: the x87 control, status and tag words,
/// ST(0)-ST(7) in stack order with the physical register each is, and,
/// on a CPU that has them, MM0-MM7 (the physical registers' significands)
/// and XMM0-XMM7 with MXCSR. A value is the register's double: one that
/// JSON can't hold (a NaN, an infinity, or an extended number beyond a
/// double's range) is null, and one below a double's normal range is 0;
/// its bits and tag say which it is.
pub(super) fn fpu_json(cpu: &Cpu) -> Value {
    let number = |v: f64| if v.is_finite() { json!(v) } else { Value::Null };
    let tags = cpu.fpu_tag_word();
    let st: Vec<Value> = (0..8)
        .map(|i| {
            let reg = cpu.fpu_get_phys_index(i);
            let tag = ["valid", "zero", "special", "empty"][(tags >> (reg * 2) & 3) as usize];
            json!({
                "reg": reg,
                "tag": tag,
                "bits": format!("{:020X}", cpu.fpu_reg(reg).st),
                "value": number(cpu.fpu_reg_f64(reg)),
            })
        })
        .collect();
    let mut reply = json!({
        "control": format!("{:04X}", cpu.fpu_control),
        "status": format!("{:04X}", cpu.fpu_status_word()),
        "tag": format!("{:04X}", tags),
        "top": cpu.fpu_top,
        "st": st,
    });
    if cpu.model.has_mmx() {
        let mm: Vec<String> = (0..8).map(|i| format!("{:016X}", cpu.fpu_reg(i).st as u64)).collect();
        reply["mm"] = json!(mm);
    }
    if cpu.model.has_sse() {
        let xmm: Vec<Value> = cpu
            .sse
            .xmm
            .iter()
            .map(|&x| {
                let ps: Vec<Value> = (0..4).map(|l| number(f32::from_bits((x >> (32 * l)) as u32) as f64)).collect();
                json!({"bits": format!("{:032X}", x), "ps": ps})
            })
            .collect();
        reply["xmm"] = json!(xmm);
        reply["mxcsr"] = json!(format!("{:08X}", cpu.sse.mxcsr));
    }
    reply
}

pub(super) fn set_regs(cpu: &mut Cpu, map: &Map<String, Value>) -> Result<(), String> {
    // Validate everything first so a bad entry doesn't leave a partial update.
    let mut updates = Vec::new();
    for (k, v) in map {
        let val = match v {
            Value::Number(n) => n.as_u64().ok_or_else(|| format!("{}: not an unsigned integer", k))? as u32,
            Value::String(s) => parse_hex(s)?,
            _ => return Err(format!("{}: value must be a number or hex string", k)),
        };
        let key = k.to_ascii_lowercase();
        let wide = matches!(
            key.as_str(),
            "eax" | "ebx" | "ecx" | "edx" | "esi" | "edi" | "ebp" | "esp" | "eip" | "eflags"
        );
        if !wide && !matches!(key.as_str(), "flags" | "fs" | "gs") && !reg16(&key) {
            return Err(format!("unknown register '{}'", k));
        }
        if !wide && val > 0xFFFF {
            return Err(format!("{}: value exceeds 16 bits", k));
        }
        updates.push((key, val));
    }
    for (k, v) in updates {
        match k.as_str() {
            "ax" => cpu.set_ax(v as u16),
            "bx" => cpu.set_bx(v as u16),
            "cx" => cpu.set_cx(v as u16),
            "dx" => cpu.set_dx(v as u16),
            "si" => cpu.set_si(v as u16),
            "di" => cpu.set_di(v as u16),
            "bp" => cpu.set_bp(v as u16),
            "sp" => cpu.set_sp(v as u16),
            "eax" => cpu.set_eax(v),
            "ebx" => cpu.set_ebx(v),
            "ecx" => cpu.set_ecx(v),
            "edx" => cpu.set_edx(v),
            "esi" => cpu.set_esi(v),
            "edi" => cpu.set_edi(v),
            "ebp" => cpu.set_ebp(v),
            "esp" => cpu.set_esp(v),
            "cs" => cpu.set_cs(v as u16),
            "ds" => cpu.set_ds(v as u16),
            "es" => cpu.set_es(v as u16),
            "ss" => cpu.set_ss(v as u16),
            "fs" => cpu.set_fs(v as u16),
            "gs" => cpu.set_gs(v as u16),
            "ip" => cpu.set_ip(v as u16),
            "eip" => cpu.set_eip(v),
            "flags" => cpu.load_flags16(v as u16),
            "eflags" => cpu.load_eflags(v),
            _ => unreachable!(),
        }
    }
    Ok(())
}

pub(super) fn screen_text(cpu: &Cpu) -> Reply {
    let mode = cpu.bus.video_mode;
    let (Some(geometry), Some(lines)) = (crate::video::text::geometry(&cpu.bus), crate::video::text::screen_lines(&cpu.bus))
    else {
        return Reply::Error(409, format!("video mode {:?} is a graphics mode; use /api/screenshot", mode));
    };
    let (cols, rows) = (geometry.cols, geometry.rows);
    let vram = cpu.bus.display_mem();
    // Each row's attributes as runs of the same one: a highlighted menu
    // item or a selected field shows as a run of its own.
    let attrs: Vec<Vec<Value>> = (0..rows)
        .map(|r| {
            let mut runs: Vec<(usize, usize, u8)> = Vec::new();
            for c in 0..cols {
                let attr = vram[geometry.offset(r, c) + 1];
                match runs.last_mut() {
                    Some((_, len, a)) if *a == attr => *len += 1,
                    _ => runs.push((c, 1, attr)),
                }
            }
            runs.into_iter().map(|(col, len, a)| json!({"col": col, "len": len, "attr": format!("{:02X}", a)})).collect()
        })
        .collect();
    // The cursor of the active display page (BIOS data area 462h), whose
    // text the reply shows.
    let cursor = 0x0450 + 2 * (cpu.bus.peek_8(0x0462) as usize & 7);
    Reply::Json(json!({
        "mode": format!("{:?}", mode),
        "cols": cols,
        "rows": rows,
        "cursor": {"col": cpu.bus.peek_8(cursor), "row": cpu.bus.peek_8(cursor + 1)},
        "lines": lines,
        "attrs": attrs,
        // How to read an attribute byte: `blink` says whether bit 7 blinks
        // the character (when false it selects a bright background), and
        // `mono` whether the bytes are the MDA and Hercules attributes (07
        // normal, 70 reverse, 01 underline, 08 bright) in place of colours.
        "blink": cpu.bus.vga.blinks(),
        "mono": geometry.mono,
    }))
}

/// What the sound visualizer shows: the FM channels, the Ultrasound's
/// voices, the MIDI notes held, and what is muted.
pub(super) fn sound_json(cpu: &Cpu) -> Value {
    let bus = &cpu.bus;
    let fm: Vec<Value> = bus
        .opl
        .channels()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({ "channel": i + 1, "key_on": c.key_on, "f_num": c.f_num, "block": c.block, "level": c.level,
                    "four_op": c.four_op, "drum": c.drum, "muted": bus.opl.mute() >> i & 1 != 0 })
        })
        .collect();
    let gus: Option<Vec<Value>> = bus.gus.as_ref().map(|gus| {
        gus.voice_views()
            .iter()
            .enumerate()
            .map(|(i, v)| json!({ "voice": i + 1, "playing": v.playing, "level": v.level, "pan": v.pan, "muted": gus.mute() >> i & 1 != 0 }))
            .collect()
    });
    let midi: Vec<Value> = bus
        .mpu
        .held_notes()
        .iter()
        .enumerate()
        .filter_map(|(ch, notes)| {
            let held: Vec<Value> = notes.iter().enumerate().filter(|(_, v)| **v > 0).map(|(n, v)| json!([n, v])).collect();
            let muted = bus.mpu.mute() >> ch & 1 != 0;
            (!held.is_empty() || muted).then(|| json!({ "channel": ch + 1, "notes": held, "muted": muted }))
        })
        .collect();
    let silenced: Vec<&str> =
        rust_dos::mixer::Channel::ALL.iter().filter(|c| bus.mixer.silenced >> c.index() & 1 != 0).map(|c| c.key()).collect();
    json!({ "fm": { "opl3": bus.opl.is_opl3(), "idle": bus.opl.is_idle(), "channels": fm }, "gus": gus, "midi": midi, "silenced": silenced })
}

pub(super) fn ivt_json(cpu: &Cpu) -> Value {
    let entries: Vec<Value> = (0..256usize)
        .map(|v| {
            let off = cpu.bus.peek_8(v * 4) as u16 | (cpu.bus.peek_8(v * 4 + 1) as u16) << 8;
            let seg = cpu.bus.peek_8(v * 4 + 2) as u16 | (cpu.bus.peek_8(v * 4 + 3) as u16) << 8;
            let phys = cpu.get_physical_addr(seg, off);
            let hle = cpu.bus.peek_8(phys) == 0xFE && cpu.bus.peek_8(phys + 1) == 0x38;
            json!({"int": format!("{:02X}", v), "vector": format!("{:04X}:{:04X}", seg, off), "hle": hle})
        })
        .collect();
    json!({"ivt": entries})
}

/// True while the built-in shell (segment 0000) is running with no program
/// loaded. At the prompt the shell spends half its time inside the INT 16h
/// BIOS trap at F000, so a trap whose caller (the CS in the IRET frame on
/// top of the stack) is the shell counts as well.
pub(super) fn drives_json(cpu: &Cpu) -> Value {
    let disk = &cpu.bus.disk;
    let drives: Map<String, Value> = disk
        .all_drives()
        .into_iter()
        .map(|info| {
            let entry = json!({
                "type": info.kind.name(),
                "path": info.root.as_ref().or(info.image.as_ref()).map(|p| display_host_path(p)),
                "image": info.image.is_some(),
                "images": info.images.iter().map(|p| display_host_path(p)).collect::<Vec<_>>(),
                "image_index": info.image_index,
                "label": info.label,
                "read_only": info.read_only,
                "current_dir": info.current_dir,
            });
            (crate::disk::drive_key(info.drive), entry)
        })
        .collect();
    json!({
        "current_drive": drive_letter(cpu.bus.disk.get_current_drive()).to_string(),
        "current_dir": cpu.bus.disk.get_current_directory(),
        "drives": drives,
    })
}

/// `/api/printer`: the printer on LPT1, what it printed and where it
/// went. With `eject`, the job ends first and its files are written.
pub(super) fn printer_json(cpu: &mut Cpu, eject: bool) -> serde_json::Value {
    let output = cpu.bus.printer_output_settings().output;
    let lpt_dac = cpu.bus.lpt_dac.is_some();
    let Some(p) = &mut cpu.bus.printer else {
        return json!({"present": false, "output": output.name(), "lpt_dac": lpt_dac});
    };
    if eject {
        p.eject();
    }
    p.sync();
    json!({
        "present": true,
        "output": p.settings.output.name(),
        "dpi": p.settings.dpi,
        "paper": p.settings.paper.name(),
        "docpath": p.settings.docpath.display().to_string(),
        "printing": p.busy(),
        "bytes": p.bytes,
        "pages": p.pages,
        "jobs": p.jobs,
        "last": p.last,
    })
}

/// `/api/net`: the IPX driver's sockets, ECBs and packets, and the LAN.
pub(super) fn serial_json(cpu: &Cpu) -> serde_json::Value {
    use rust_dos::serial::Backend;
    let ports: Vec<_> = cpu
        .bus
        .serial
        .ports
        .iter()
        .enumerate()
        .map(|(n, port)| {
            let Some(port) = port else { return json!({"port": format!("COM{}", n + 1), "present": false}) };
            let uart: serde_json::Map<_, _> =
                port.uart.describe().into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::String(v))).collect();
            let backend = match &port.backend {
                Backend::Empty => json!({"type": "empty"}),
                Backend::Mouse(m) => json!({"type": "mouse", "kind": m.kind.name(), "powered": m.powered}),
                Backend::NullModem(c) => json!({"type": "nullmodem", "peer": c.peer, "lines": format!("{:02X}", c.lines())}),
                Backend::Modem(m) => json!({
                    "type": "modem",
                    "call": format!("{:?}", m.call),
                    "online": m.online,
                    "direct": m.direct,
                    "peer": m.peer,
                }),
            };
            json!({"port": format!("COM{}", n + 1), "present": true, "uart": uart, "backend": backend})
        })
        .collect();
    json!({
        "ports": ports,
        "pic_lines": format!("{:04X}", cpu.bus.serial.pic_lines),
        "link": cpu.bus.net.serial_status(),
    })
}

pub(super) fn net_json(cpu: &Cpu) -> serde_json::Value {
    let far = |(segment, offset): (u16, u16)| format!("{:04X}:{:04X}", segment, offset);
    let ipx = cpu.bus.net.ipx.as_ref().map(|ipx| {
        use rust_dos::net::ipx::Done;
        json!({
            "node": ipx.node.to_string(),
            "irq": ipx.irq,
            "frame_type": ipx.frame_type.name(),
            "sockets": ipx.sockets.iter().map(|s| json!({
                "socket": format!("{:04X}", s.number),
                "long_lived": s.long_lived,
                "owner": format!("{:04X}", s.owner),
            })).collect::<Vec<_>>(),
            "listening": ipx.listens.iter().map(|l| json!({"ecb": far(l.ecb), "socket": format!("{:04X}", l.socket)})).collect::<Vec<_>>(),
            "events": ipx.events.iter().map(|e| json!({"ecb": far(e.ecb), "due": e.due})).collect::<Vec<_>>(),
            "completions": ipx.completions.iter().map(|c| json!({
                "ecb": far(c.ecb),
                "done": match &c.done {
                    Done::Sent => "sent".to_string(),
                    Done::Event => "event".to_string(),
                    Done::Received { packet, from } => format!("received {} bytes from {}", packet.len(), from),
                },
            })).collect::<Vec<_>>(),
            "held": ipx.held.iter().map(|h| json!({"socket": format!("{:04X}", h.socket), "bytes": h.packet.len(), "from": h.from.to_string()})).collect::<Vec<_>>(),
            "stats": {
                "sent": ipx.stats.sent,
                "received": ipx.stats.received,
                "not_ours": ipx.stats.not_ours,
                "closed_socket": ipx.stats.closed_socket,
                "unheard": ipx.stats.unheard,
            },
        })
    });
    let lan = cpu.bus.net.status().hub.map(|h| json!({
        "state": format!("{:?}", h.lan),
        "room": h.room,
        "hosting": h.hosting.map(|a| a.to_string()),
        "hosted_rooms": h.hosted_rooms.iter().map(|r| json!({"room": r.name, "members": r.members})).collect::<Vec<_>>(),
        "frames_out": h.frames_out,
        "frames_in": h.frames_in,
    }));
    json!({ "settings": format!("{:?}", cpu.bus.net.settings), "ipx": ipx, "lan": lan })
}

/// A ViRGE's engines for `/api/status`: the commands they ran, the last 3D
/// command, the status and the overlay.
fn virge_status(v: &crate::video::s3::virge::Virge, vx: bool) -> serde_json::Value {
    json!({
        "bitblts": v.counts[0], "rects": v.counts[1], "lines": v.counts[2], "polygons": v.counts[3],
        "lines_3d": v.s3d.lines, "triangles": v.s3d.triangles,
        "cmd_3d": format!("{:08X}", v.s3d.tri[0x40]),
        "status": format!("{:04X}", v.status()), "advfunc": format!("{:02X}", v.advfunc),
        "transfer": v.transfer.active, "command_dma": v.dma.enable != 0,
        "overlay": v.streams.overlay(vx).map(|o| format!("{:?}", o)),
    })
}
