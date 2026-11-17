//! ITypeInfo::Invoke: a call of a method of the program's object by its
//! description in the type library. A program's IDispatch::Invoke hands
//! its work on to it (InstallShield 6's engine does, for the objects it
//! gives its setup), as CreateStdDispatch's does.
//!
//! The function is looked up by its DISPID and the kind of call, on the
//! interface side of a dual interface, and of each dual interface it
//! inherits from, whose vtable the object has. The DISPPARAMS'
//! arguments, last first, then the named ones, are changed to the types
//! of the method's parameters as VariantChangeType changes them. A
//! parameter passed by reference takes the argument's pointer, or a
//! pointer to the argument's own value when it is of the type by value
//! (a SAFEARRAY passed for a SAFEARRAY*), or to a changed copy.
//! A missing optional VARIANT gets VT_ERROR with DISP_E_PARAMNOTFOUND,
//! and a parameter with a default its default. The method is called
//! through the object's vtable, and its [retval] parameter, or its
//! return value if it doesn't return an HRESULT, comes back in the
//! result's VARIANT. A failed HRESULT comes back as DISP_E_EXCEPTION
//! with the HRESULT as the EXCEPINFO's scode; the EXCEPINFO's strings
//! stay empty, as the error object of the object isn't asked for.

use super::model::*;
use super::typelib;
use super::variant::{self, Out};
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_OUTOFMEMORY, S_OK};
use crate::win32::ole32::{task_alloc, task_free};
use crate::win32::task::{self, Next, Task};

const DISP_E_MEMBERNOTFOUND: u32 = 0x8002_0003;
const DISP_E_PARAMNOTFOUND: u32 = 0x8002_0004;
const DISP_E_TYPEMISMATCH: u32 = variant::DISP_E_TYPEMISMATCH;
const DISP_E_BADVARTYPE: u32 = variant::DISP_E_BADVARTYPE;
const DISP_E_EXCEPTION: u32 = 0x8002_0009;
const DISP_E_BADPARAMCOUNT: u32 = 0x8002_000E;
const DISP_E_BADCALLEE: u32 = 0x8002_0010;
const DISPID_PROPERTYPUT: i32 = -3;
const LOCALE_USER_DEFAULT: u32 = 0x400;
const VT_BYREF: u16 = 0x4000;
const VT_ARRAY: u16 = 0x2000;
const VT_R4: u16 = 4;
const VT_R8: u16 = 5;
const VT_CY: u16 = 6;
const VT_DATE: u16 = 7;
const VT_I8: u16 = 20;
const VT_UI8: u16 = 21;
const CC_CDECL: u32 = 1;

/// DISPPARAMS.
const DP_ARGS: u32 = 0;
const DP_NAMED: u32 = 4;
const DP_COUNT: u32 = 8;
const DP_NAMED_COUNT: u32 = 12;

/// EXCEPINFO: its size and its scode.
const EXCEPINFO_SIZE: usize = 32;
const EI_SCODE: u32 = 28;

/// What the method returns.
const RET_HRESULT: u32 = 0;
const RET_VOID: u32 = 1;
/// Otherwise the VARTYPE of its value, plus this.
const RET_VALUE: u32 = 0x1_0000;

/// The task's variables.
const V_TARGET: usize = 0;
const V_RESULT: usize = 1;
const V_EXCEPINFO: usize = 2;
const V_BLOCK: usize = 3;
/// The [retval] parameter's VARTYPE and its place, 0 for none.
const V_RETVAL_VT: usize = 4;
const V_RETVAL: usize = 5;
const V_RET: usize = 6;
/// The HRESULT, after the call.
const V_HRESULT: usize = 7;
const V_COUNT: usize = 8;
/// Then the arguments, then the BSTRs made for the call.
const V_ARGS: usize = 9;

fn read16(cpu: &mut Cpu, at: u32) -> u16 {
    cpu.bus.guest_read_16(at)
}

fn read32(cpu: &mut Cpu, at: u32) -> u32 {
    cpu.bus.guest_read_32(at)
}

/// The VARTYPE a parameter of the type `ty` of the type at `index` of
/// `lib` takes, VT_BYREF with it for a pointer; and whether `ty` is an
/// interface's own type, whose pointer is the value.
fn param_vt(
    cpu: &mut Cpu,
    lib: u32,
    index: u32,
    ty: &TypeDesc,
    depth: usize,
) -> Option<(u16, bool)> {
    if depth > 16 {
        return None;
    }
    match ty {
        TypeDesc::Base(vt) => {
            let vt = match *vt {
                VT_INT => VT_I4,
                VT_UINT => VT_UI4,
                VT_HRESULT => VT_ERROR,
                VT_LPSTR | VT_LPWSTR | VT_VOID | VT_PTR | VT_CARRAY | VT_USERDEFINED => {
                    return None;
                }
                vt => vt,
            };
            Some((vt, false))
        }
        TypeDesc::SafeArray(inner) => {
            let (vt, _) = param_vt(cpu, lib, index, inner, depth + 1)?;
            (vt & (VT_BYREF | VT_ARRAY) == 0).then_some((VT_ARRAY | vt, false))
        }
        TypeDesc::CArray(..) => None,
        TypeDesc::Ptr(inner) => match param_vt(cpu, lib, index, inner, depth + 1)? {
            (vt, true) => Some((vt, false)),
            (vt, false) if vt & VT_BYREF == 0 => Some((vt | VT_BYREF, false)),
            _ => None,
        },
        TypeDesc::User(href) => {
            let (l, i, _) = typelib::resolve_in(cpu, lib, index, *href).ok()?;
            let m = typelib::model(cpu, l);
            let t = &m.infos[i as usize];
            let found = match t.kind {
                TKIND_ENUM => Some((VT_I4, false)),
                TKIND_DISPATCH => Some((VT_DISPATCH, true)),
                TKIND_INTERFACE if t.is_dual() => Some((VT_DISPATCH, true)),
                TKIND_INTERFACE | TKIND_COCLASS => Some((VT_UNKNOWN, true)),
                TKIND_ALIAS => match t.alias.clone() {
                    Some(alias) => param_vt(cpu, l, i, &alias, depth + 1),
                    None => None,
                },
                _ => None,
            };
            typelib::release_lib(cpu, l);
            found
        }
    }
}

/// The dwords a value of the type `vt` takes on the stack.
fn dwords(vt: u16) -> u32 {
    match vt {
        VT_VARIANT => 4,
        VT_R8 | VT_CY | VT_DATE | VT_I8 | VT_UI8 => 2,
        _ => 1,
    }
}

/// What a call is made of, while it is put together: the BSTRs made for
/// it, freed after it, and where the argument that failed is.
struct Call {
    stack: Vec<u32>,
    bstrs: Vec<u32>,
}

impl Call {
    /// Push the value of the type `vt` that the VARIANT at `src` holds.
    fn push_value(&mut self, cpu: &mut Cpu, src: u32, vt: u16) -> Result<(), u32> {
        if vt == VT_VARIANT {
            let mut src = src;
            if read16(cpu, src) == VT_BYREF | VT_VARIANT {
                src = read32(cpu, src + 8);
                if !cpu.bus.guest_probe(src, 16, false) {
                    return Err(E_INVALIDARG);
                }
            }
            for i in 0..4 {
                let dword = read32(cpu, src + 4 * i);
                self.stack.push(dword);
            }
            return Ok(());
        }
        // The argument's own value, when it is of the type already.
        if read16(cpu, src) == vt {
            for i in 0..dwords(vt) {
                let dword = read32(cpu, src + 8 + 4 * i);
                self.stack.push(dword);
            }
            return Ok(());
        }
        match variant::change_from(cpu, src, vt, 0)? {
            Out::Bits(_, bits) => {
                self.stack.push(bits as u32);
                if dwords(vt) == 2 {
                    self.stack.push((bits >> 32) as u32);
                }
            }
            Out::Str(units) => {
                let s = new_bstr(cpu, &units)?;
                self.bstrs.push(s);
                self.stack.push(s);
            }
            Out::Iface(_, p) => self.stack.push(p),
        }
        Ok(())
    }

    /// Push the pointer a parameter of the type `vt` (VT_BYREF with the
    /// type) takes, for the VARIANT at `src`; `slot` is 16 bytes to
    /// change a value passed by value into.
    fn push_ref(&mut self, cpu: &mut Cpu, src: u32, vt: u16, slot: u32) -> Result<(), u32> {
        let src_vt = read16(cpu, src);
        if src_vt == vt {
            let p = read32(cpu, src + 8);
            self.stack.push(p);
            return Ok(());
        }
        if vt == VT_BYREF | VT_VARIANT {
            self.stack.push(src);
            return Ok(());
        }
        if src_vt & VT_BYREF != 0 {
            return Err(DISP_E_TYPEMISMATCH);
        }
        // A value of the type: a pointer to the argument's own, which the
        // method may change. An array passed by value goes so too.
        if src_vt == vt & !VT_BYREF {
            self.stack.push(src + 8);
            return Ok(());
        }
        // A value for a pointer: a copy of it changed to the type.
        let base = vt & !VT_BYREF;
        let value = match variant::change_from(cpu, src, base, 0)? {
            Out::Bits(_, bits) => bits,
            Out::Str(units) => {
                let s = new_bstr(cpu, &units)?;
                self.bstrs.push(s);
                s as u64
            }
            Out::Iface(_, p) => p as u64,
        };
        cpu.bus.guest_write_bytes(slot, &value.to_le_bytes());
        self.stack.push(slot);
        Ok(())
    }
}

fn new_bstr(cpu: &mut Cpu, units: &[u16]) -> Result<u32, u32> {
    let s = super::alloc(cpu, 0, 2 * units.len() as u32);
    if s == 0 {
        return Err(E_OUTOFMEMORY);
    }
    for (i, u) in units.iter().enumerate() {
        cpu.bus.guest_write_16(s + 2 * i as u32, *u);
    }
    Ok(s)
}

/// Invoke(instance, memid, flags, params, result, excepinfo, argerr).
pub fn invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        this,
        instance,
        memid,
        flags,
        params,
        result,
        excepinfo,
        argerr,
    ] = args
    else {
        unreachable!()
    };
    let Some(ctx) = typelib::this_ctx(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if instance == 0 || params == 0 || !cpu.bus.guest_probe(params, 16, false) {
        return Some(E_INVALIDARG);
    }
    let Some((func, lib, index)) = typelib::invokable(cpu, &ctx, memid as i32, flags) else {
        return Some(DISP_E_MEMBERNOTFOUND);
    };
    let vars = prepare(cpu, &func, lib, index, instance, flags, params, argerr);
    typelib::release_lib(cpu, lib);
    match vars {
        Ok(mut vars) => {
            vars[V_RESULT] = result;
            vars[V_EXCEPINFO] = excepinfo;
            task::start(cpu, task::OLEAUT32_INVOKE, vars, 0)
        }
        Err(e) => Some(e),
    }
}

/// The task's variables for the call of `f` of the type at `index` of
/// `lib` on `instance`, from the DISPPARAMS at `params`; Err with the
/// HRESULT when the arguments don't fit the parameters, after writing
/// the place of the one that doesn't to `argerr`.
#[allow(clippy::too_many_arguments)]
fn prepare(
    cpu: &mut Cpu,
    f: &Func,
    lib: u32,
    index: u32,
    instance: u32,
    flags: u32,
    params: u32,
    argerr: u32,
) -> Result<Vec<u32>, u32> {
    if f.kind == FUNC_DISPATCH || f.vft < 0 {
        return Err(DISP_E_MEMBERNOTFOUND);
    }
    if f.callconv != CC_STDCALL && f.callconv != CC_CDECL {
        return Err(DISP_E_BADCALLEE);
    }
    let ret = match &f.ret {
        TypeDesc::Base(VT_HRESULT) => RET_HRESULT,
        TypeDesc::Base(VT_VOID) => RET_VOID,
        ty => match param_vt(cpu, lib, index, ty, 0) {
            Some((vt, _)) if dwords(vt) == 1 && vt != VT_R4 && vt != VT_VARIANT => {
                RET_VALUE | vt as u32
            }
            _ => return Err(DISP_E_BADVARTYPE),
        },
    };
    let vtable = read32(cpu, instance);
    let at = vtable.wrapping_add(f.vft as u16 as u32);
    if !cpu.bus.guest_probe(at, 4, false) {
        return Err(E_INVALIDARG);
    }
    let target = read32(cpu, at);

    let args = read32(cpu, params + DP_ARGS);
    let named = read32(cpu, params + DP_NAMED);
    let count = read32(cpu, params + DP_COUNT);
    let named_count = read32(cpu, params + DP_NAMED_COUNT);
    if named_count > count
        || (count > 0 && !cpu.bus.guest_probe(args, 16 * count as usize, false))
        || (named_count > 0 && !cpu.bus.guest_probe(named, 4 * named_count as usize, false))
    {
        return Err(E_INVALIDARG);
    }
    // The parameters the arguments go to: not [retval] or [lcid].
    let inputs: Vec<usize> = f
        .params
        .iter()
        .enumerate()
        .filter(|(_, p)| p.flags & (PARAMFLAG_FRETVAL | PARAMFLAG_FLCID) == 0)
        .map(|(i, _)| i)
        .collect();
    let fail = |cpu: &mut Cpu, e: u32, arg: u32| {
        if argerr != 0 && cpu.bus.guest_probe(argerr, 4, true) {
            cpu.bus.guest_write_32(argerr, arg);
        }
        Err(e)
    };
    let positional = count - named_count;
    if positional as usize > inputs.len() {
        return Err(DISP_E_BADPARAMCOUNT);
    }
    // The argument of each input, by its place in `args`: the positional
    // ones last first, after the named ones.
    let mut sources: Vec<Option<u32>> = vec![None; inputs.len()];
    for j in 0..positional {
        sources[j as usize] = Some(count - 1 - j);
    }
    let put = flags & (INVOKE_PROPERTYPUT | INVOKE_PROPERTYPUTREF) != 0;
    for k in 0..named_count {
        let id = read32(cpu, named + 4 * k) as i32;
        let place = if id == DISPID_PROPERTYPUT && put {
            inputs.len().checked_sub(1)
        } else if id >= 0 {
            inputs.iter().position(|&i| i == id as usize)
        } else {
            None
        };
        match place {
            Some(p) if sources[p].is_none() => sources[p] = Some(k),
            _ => return fail(cpu, DISP_E_PARAMNOTFOUND, k),
        }
    }

    let block = task_alloc(cpu, 16 * (f.params.len() as u32 + 1));
    if block == 0 {
        return Err(E_OUTOFMEMORY);
    }
    cpu.bus.guest_fill(block, 16 * (f.params.len() + 1), 0);
    let mut call = Call {
        stack: vec![instance],
        bstrs: Vec::new(),
    };
    let mut retval = (0, 0);
    let mut built = Ok(());
    for (i, p) in f.params.iter().enumerate() {
        let slot = block + 16 * i as u32;
        let Some((vt, _)) = param_vt(cpu, lib, index, &p.ty, 0) else {
            built = Err((DISP_E_BADVARTYPE, 0));
            break;
        };
        if p.flags & PARAMFLAG_FRETVAL != 0 {
            if vt & VT_BYREF == 0 {
                built = Err((DISP_E_BADVARTYPE, 0));
                break;
            }
            retval = (vt & !VT_BYREF, slot);
            call.stack.push(slot);
            continue;
        }
        if p.flags & PARAMFLAG_FLCID != 0 {
            call.stack.push(LOCALE_USER_DEFAULT);
            continue;
        }
        let input = inputs.iter().position(|&n| n == i).unwrap_or(0);
        let place = sources[input];
        let src = match place {
            Some(k) => args + 16 * k,
            None => {
                // A missing argument: the parameter's default, or "not
                // there" for an optional VARIANT.
                match (&p.default, vt & !VT_BYREF) {
                    (Some(Value::Bits(dvt, bits)), _) if p.flags & PARAMFLAG_FHASDEFAULT != 0 => {
                        cpu.bus.guest_write_16(slot, *dvt);
                        cpu.bus.guest_write_bytes(slot + 8, &bits.to_le_bytes());
                    }
                    (Some(Value::Str(s)), _) if p.flags & PARAMFLAG_FHASDEFAULT != 0 => {
                        let units: Vec<u16> = s.as_deref().unwrap_or("").encode_utf16().collect();
                        match new_bstr(cpu, &units) {
                            Ok(b) => {
                                call.bstrs.push(b);
                                cpu.bus.guest_write_16(slot, VT_BSTR);
                                cpu.bus.guest_write_32(slot + 8, b);
                            }
                            Err(e) => {
                                built = Err((e, 0));
                                break;
                            }
                        }
                    }
                    (_, VT_VARIANT) if p.flags & PARAMFLAG_FOPT != 0 => {
                        cpu.bus.guest_write_16(slot, VT_ERROR);
                        cpu.bus.guest_write_32(slot + 8, DISP_E_PARAMNOTFOUND);
                    }
                    _ => {
                        built = Err((DISP_E_PARAMNOTFOUND, 0));
                        break;
                    }
                }
                // The slot holds the argument; a pointer to it is passed
                // for a VARIANT*.
                if vt == VT_BYREF | VT_VARIANT {
                    call.stack.push(slot);
                    continue;
                }
                slot
            }
        };
        let pushed = if vt & VT_BYREF != 0 {
            call.push_ref(cpu, src, vt, slot)
        } else {
            call.push_value(cpu, src, vt)
        };
        if let Err(e) = pushed {
            let src_vt = read16(cpu, src);
            cpu.bus.log_string(&format!(
                "[W32] OLEAUT32: Invoke of {}: its parameter {} takes VARTYPE {:04X}, \
                 the argument is {:04X} ({:08X})",
                f.name.as_deref().unwrap_or("?"),
                i,
                vt,
                src_vt,
                e
            ));
            built = Err((e, place.unwrap_or(0)));
            break;
        }
    }
    if let Err((e, arg)) = built {
        for &s in &call.bstrs {
            super::free(cpu, s);
        }
        task_free(cpu, block);
        return fail(cpu, e, arg);
    }
    let mut vars = vec![0; V_ARGS];
    vars[V_TARGET] = target;
    vars[V_BLOCK] = block;
    vars[V_RETVAL_VT] = retval.0 as u32;
    vars[V_RETVAL] = retval.1;
    vars[V_RET] = ret;
    vars[V_COUNT] = call.stack.len() as u32;
    vars.extend(&call.stack);
    vars.extend(&call.bstrs);
    Ok(vars)
}

/// The task: the call, then its result put in the VARIANT and the
/// EXCEPINFO, and what was made for it freed.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let count = t.var(V_COUNT) as usize;
    match t.step {
        0 => Next::call(t.var(V_TARGET), &t.vars[V_ARGS..V_ARGS + count], 1),
        1 => {
            for i in V_ARGS + count..t.vars.len() {
                let s = t.var(i);
                super::free(cpu, s);
            }
            let result = t.var(V_RESULT);
            let block = t.var(V_BLOCK);
            let (retval_vt, retval) = (t.var(V_RETVAL_VT) as u16, t.var(V_RETVAL));
            let ret = t.var(V_RET);
            let failed = ret == RET_HRESULT && value & 0x8000_0000 != 0;
            let mut release = 0;
            if failed {
                let excepinfo = t.var(V_EXCEPINFO);
                if excepinfo != 0 && cpu.bus.guest_probe(excepinfo, EXCEPINFO_SIZE, true) {
                    cpu.bus.guest_fill(excepinfo, EXCEPINFO_SIZE, 0);
                    cpu.bus.guest_write_32(excepinfo + EI_SCODE, value);
                }
                t.set(V_HRESULT, DISP_E_EXCEPTION);
            } else {
                t.set(V_HRESULT, S_OK);
                let mut out = [0u8; 16];
                if retval != 0 {
                    if retval_vt == VT_VARIANT {
                        cpu.bus.guest_read_bytes(retval, &mut out);
                    } else {
                        out[..2].copy_from_slice(&retval_vt.to_le_bytes());
                        cpu.bus.guest_read_bytes(retval, &mut out[8..]);
                    }
                } else if ret & RET_VALUE != 0 {
                    out[..2].copy_from_slice(&(ret as u16).to_le_bytes());
                    out[8..12].copy_from_slice(&value.to_le_bytes());
                }
                if result != 0 && cpu.bus.guest_probe(result, 16, true) {
                    cpu.bus.guest_write_bytes(result, &out);
                } else {
                    // Nobody takes the value: what it holds goes.
                    let vt = u16::from_le_bytes([out[0], out[1]]);
                    let p = u32::from_le_bytes([out[8], out[9], out[10], out[11]]);
                    match vt {
                        VT_BSTR => super::free(cpu, p),
                        VT_DISPATCH | VT_UNKNOWN if p != 0 => release = p,
                        _ => {}
                    }
                }
            }
            task_free(cpu, block);
            if release != 0
                && let Some(m) = crate::win32::ole32::method(cpu, release, 2)
            {
                return Next::call(m, &[release], 2);
            }
            Next::Done(t.var(V_HRESULT))
        }
        _ => Next::Done(t.var(V_HRESULT)),
    }
}

/// DispGetParam(params, position, vt, result, argerr): the argument at
/// `position` among the parameters, named by it or the one at that
/// place among the positional arguments, changed to `vt` into `result`
/// as VariantChangeType changes it. DISP_E_PARAMNOTFOUND when there is
/// none; for one that can't be changed, its place in the arguments goes
/// to `argerr`.
pub fn disp_get_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[params, position, vt, result, argerr] = args else {
        unreachable!()
    };
    if params == 0 || result == 0 || !cpu.bus.guest_probe(params, 16, false) {
        return Some(E_INVALIDARG);
    }
    let rgvarg = read32(cpu, params + DP_ARGS);
    let named = read32(cpu, params + DP_NAMED);
    let count = read32(cpu, params + DP_COUNT);
    let named_count = read32(cpu, params + DP_NAMED_COUNT);
    if named_count > count
        || (named_count > 0 && !cpu.bus.guest_probe(named, 4 * named_count as usize, false))
    {
        return Some(E_INVALIDARG);
    }
    let by_name = (0..named_count).find(|&k| read32(cpu, named + 4 * k) == position);
    let place = match by_name {
        Some(k) => k,
        None if position < count - named_count => count - 1 - position,
        None => return Some(DISP_E_PARAMNOTFOUND),
    };
    let src = rgvarg + 16 * place;
    if !cpu.bus.guest_probe(src, 16, false) || !cpu.bus.guest_probe(result, 16, true) {
        return Some(E_INVALIDARG);
    }
    // The result is taken as empty: callers pass a VARIANT of their
    // stack.
    cpu.bus.guest_fill(result, 16, 0);
    let hr = variant::change_type(cpu, result, src, 0, vt as u16);
    if hr == Some(DISP_E_TYPEMISMATCH) && argerr != 0 && cpu.bus.guest_probe(argerr, 4, true) {
        cpu.bus.guest_write_32(argerr, place);
    }
    hr
}
