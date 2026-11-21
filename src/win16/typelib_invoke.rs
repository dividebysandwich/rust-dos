//! ITypeInfo::Invoke of the 16-bit TYPELIB: a call of a member of the
//! program's object by its description in the type library, which
//! DispInvoke and a program's own IDispatch::Invoke hand on to. Visual
//! Basic 4's runtime calls it on the types of its forms before it looks
//! for a member of its own, and goes on to its own when the type has no
//! member of the DISPID (DISP_E_MEMBERNOTFOUND).
//!
//! As the 32-bit one (win32/oleaut32/invoke.rs) and Wine's
//! ITypeInfo_fnInvoke do it: the function is looked for by its DISPID and
//! the kind of call in the type and the interfaces it inherits from. The
//! DISPPARAMS' arguments, last first, then the named ones, are changed to
//! the parameters' types as VariantChangeType changes them, a missing
//! optional VARIANT gets VT_ERROR with DISP_E_PARAMNOTFOUND, and one with
//! a default its default. The method is called through the object's
//! vtable; its [retval] parameter, or its value if it doesn't return an
//! HRESULT, goes in the result's VARIANT. A failed HRESULT comes back as
//! DISP_E_EXCEPTION, with the HRESULT as the EXCEPINFO's scode. A
//! dispinterface's member (FUNC_DISPATCH) is called through the object's
//! IDispatch::Invoke, as Wine does.
//!
//! What is 16-bit: the structures of the Windows 3.1 SDK's dispatch.h
//! (DISPPARAMS' counts are words, an unsigned int is a word), the 16-bit
//! VARIANT (ole2disp.rs), and the stack. An int and an enum take a word,
//! a long, a float, a pointer and a BSTR a dword, a double, a CURRENCY
//! and a DATE 8 bytes, a VARIANT 16. A C method (CC_CDECL, what the
//! SDK's STDMETHOD declares) has its arguments pushed last first, the
//! object's pointer last; a Pascal one (CC_PASCAL) has them pushed first
//! first, and the object's pointer still last, where Microsoft's 16-bit
//! C++ compilers have it for a __pascal member function. Other calling
//! conventions give DISP_E_BADCALLEE. A value comes back in AX or DX:AX,
//! so a method returning a floating-point value or a VARIANT, which
//! 16-bit C returns elsewhere, gives DISP_E_BADVARTYPE.
//!
//! Not done: a module's functions (FUNC_STATIC and FUNC_NONVIRTUAL),
//! which need AddressOfMember, give E_NOTIMPL.

use super::ole2::{self, dx_ax, method_address};
use super::ole2disp::{alloc_bstr, ansi, changed, free_bstr};
use super::thunk;
use super::typelib::{self, find_ref, info_parts, log_not_implemented, search};
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::win32::com::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, S_OK};
use crate::win32::oleaut32::model::*;
use crate::win32::oleaut32::variant::Out;

const DISP_E_MEMBERNOTFOUND: u32 = 0x8002_0003;
const DISP_E_PARAMNOTFOUND: u32 = 0x8002_0004;
const DISP_E_TYPEMISMATCH: u32 = 0x8002_0005;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;
const DISP_E_EXCEPTION: u32 = 0x8002_0009;
const DISP_E_BADPARAMCOUNT: u32 = 0x8002_000E;
const DISP_E_BADCALLEE: u32 = 0x8002_0010;
const DISPID_PROPERTYPUT: i32 = -3;
const LOCALE_USER_DEFAULT: u32 = 0x400;

const FUNC_VIRTUAL: u32 = 0;
const CC_CDECL: u32 = 1;
const CC_PASCAL: u32 = 2;

const VT_R4: u16 = 4;
const VT_R8: u16 = 5;
const VT_CY: u16 = 6;
const VT_DATE: u16 = 7;
const VT_ARRAY: u16 = 0x2000;
const VT_BYREF: u16 = 0x4000;

/// The 16-bit DISPPARAMS: the arguments, the named arguments' DISPIDs,
/// and their counts, words.
const DP_ARGS: u32 = 0;
const DP_NAMED: u32 = 4;
const DP_COUNT: u32 = 8;
const DP_NAMED_COUNT: u32 = 10;

/// The 16-bit EXCEPINFO: its size and its scode.
const EXCEPINFO_SIZE: usize = 32;
const EI_SCODE: u32 = 28;

/// IDispatch's interface ID, and its methods QueryInterface, Release and
/// Invoke by their place in the vtable.
const IID_IDISPATCH: [u8; 16] = ole2::ole_guid(0x0002_0400);
const QUERY_INTERFACE: u32 = 0;
const RELEASE: u32 = 2;
const DISPATCH_INVOKE: u32 = 6;

/// What the method returns.
const RET_HRESULT: u32 = 0;
const RET_VOID: u32 = 1;
/// Otherwise the VARTYPE of its value, plus this.
const RET_VALUE: u32 = 0x1_0000;

/// What `then` goes on with, the first of its data.
const STEP_METHOD: u32 = 0;
const STEP_QUERY: u32 = 1;
const STEP_DISPATCH: u32 = 2;

/// The data of STEP_METHOD: the result's VARIANT, the EXCEPINFO (16:16),
/// the block of the call's values, the [retval] parameter's VARTYPE and
/// its place in the block (16:16, 0 for none), what the method returns,
/// the number of parameters; then the BSTRs made for the call's values.
const D_RESULT: usize = 1;
const D_EXCEPINFO: usize = 2;
const D_BLOCK: usize = 3;
const D_RETVAL_VT: usize = 4;
const D_RETVAL: usize = 5;
const D_RET: usize = 6;
const D_PARAMS: usize = 7;
const D_BSTRS: usize = 8;

/// The 16:16 address `n` bytes after `p`, in its segment.
fn add16(p: u32, n: u32) -> u32 {
    p & 0xFFFF_0000 | (p as u16).wrapping_add(n as u16) as u32
}

fn linear(cpu: &Cpu, p: u32) -> u32 {
    super::linear(cpu, p)
}

/// The VARTYPE a parameter of the type `ty` of the type at `index` of
/// `m` takes, VT_BYREF with it for a pointer, and whether `ty` is an
/// interface's own type, whose pointer is the value.
fn param_vt(
    cpu: &mut Cpu,
    m: &TypeLib,
    index: usize,
    ty: &TypeDesc,
    depth: usize,
) -> Option<(u16, bool)> {
    if depth > 16 {
        return None;
    }
    match ty {
        TypeDesc::Base(vt) => {
            let vt = match *vt {
                VT_INT | VT_UINT => VT_I2,
                VT_HRESULT => VT_ERROR,
                vt @ (VT_I2 | VT_I4 | VT_R4 | VT_R8 | VT_CY | VT_DATE | VT_BSTR | VT_DISPATCH
                | VT_ERROR | VT_BOOL | VT_VARIANT | VT_UNKNOWN | VT_UI1) => vt,
                _ => return None,
            };
            Some((vt, false))
        }
        TypeDesc::SafeArray(inner) => {
            let (vt, _) = param_vt(cpu, m, index, inner, depth + 1)?;
            (vt & (VT_BYREF | VT_ARRAY) == 0).then_some((VT_ARRAY | vt, false))
        }
        TypeDesc::CArray(..) => None,
        TypeDesc::Ptr(inner) => match param_vt(cpu, m, index, inner, depth + 1)? {
            (vt, true) => Some((vt, false)),
            (vt, false) if vt & VT_BYREF == 0 => Some((vt | VT_BYREF, false)),
            _ => None,
        },
        TypeDesc::User(href) => {
            let (other, i) = find_ref(cpu, m, index, *href).ok()?;
            let m = match &other {
                Some((_, other)) => other,
                None => m,
            };
            let t = &m.infos[i];
            match t.kind {
                // An enum is an int.
                TKIND_ENUM => Some((VT_I2, false)),
                TKIND_DISPATCH => Some((VT_DISPATCH, true)),
                TKIND_INTERFACE if t.is_dual() => Some((VT_DISPATCH, true)),
                TKIND_INTERFACE | TKIND_COCLASS => Some((VT_UNKNOWN, true)),
                TKIND_ALIAS => {
                    let alias = t.alias.clone()?;
                    param_vt(cpu, m, i, &alias, depth + 1)
                }
                _ => None,
            }
        }
    }
}

/// The bytes a value of the type `vt` takes on the stack.
fn size(vt: u16) -> usize {
    match vt {
        VT_I2 | VT_BOOL | VT_UI1 => 2,
        VT_R8 | VT_CY | VT_DATE => 8,
        VT_VARIANT => 16,
        _ => 4,
    }
}

/// The value of an `Out` of the type `vt`, as `size(vt)` bytes, and the
/// BSTR made for it (0 for none).
fn out_bytes(cpu: &mut Cpu, out: Out, vt: u16) -> Result<(Vec<u8>, u32), u32> {
    let (bits, bstr) = match out {
        Out::Bits(_, bits) => (bits, 0),
        Out::Str(units) => match alloc_bstr(cpu, &ansi(&units)) {
            0 => return Err(E_OUTOFMEMORY),
            s => (s as u64, s),
        },
        Out::Iface(_, p) => (p as u64, 0),
    };
    Ok((bits.to_le_bytes()[..size(vt).min(8)].to_vec(), bstr))
}

/// Free the BSTRs the parameters' slots in the call's block at `block`
/// hold, but the [retval] parameter's at `retval`. A slot holds a
/// VARIANT: a missing argument's default, or an argument changed to the
/// type of a parameter passed by reference, whose method may have
/// replaced the BSTR it was given.
fn free_slots(cpu: &mut Cpu, block: u32, params: u32, retval: u32) {
    for i in 0..params {
        let slot = add16(block, 16 * i);
        let at = linear(cpu, slot);
        if slot != retval && at != 0 && super::read_16(cpu, at) == VT_BSTR {
            free_bstr(cpu, super::read_32(cpu, at + 8));
        }
    }
}

/// A call while it is put together: each argument's bytes as they lie
/// in memory, and the BSTRs made for the values passed, freed after it.
/// The BSTRs the slots of the call's block hold are freed with them
/// (`free_slots`).
struct Call {
    args: Vec<Vec<u8>>,
    bstrs: Vec<u32>,
}

impl Call {
    /// The argument for a parameter of the type `vt` (not VT_BYREF) from
    /// the VARIANT at `src` (flat).
    fn value(&mut self, cpu: &mut Cpu, src: u32, vt: u16) -> Result<Vec<u8>, u32> {
        if vt == VT_VARIANT {
            let mut src = src;
            if super::read_16(cpu, src) == VT_BYREF | VT_VARIANT {
                src = linear(cpu, super::read_32(cpu, src + 8));
            }
            return super::peek(cpu, src, 16).ok_or(E_INVALIDARG);
        }
        // The argument's own value, when it is of the type already.
        if super::read_16(cpu, src) == vt {
            return super::peek(cpu, src + 8, size(vt)).ok_or(E_INVALIDARG);
        }
        let out = changed(cpu, src, vt, 0)?;
        let (bytes, bstr) = out_bytes(cpu, out, vt)?;
        if bstr != 0 {
            self.bstrs.push(bstr);
        }
        Ok(bytes)
    }

    /// The pointer a parameter of the type `vt` (VT_BYREF with the type)
    /// takes, for the VARIANT at `src16` (16:16): `slot` (16:16) is a
    /// VARIANT to change a value of another type into, whose value the
    /// pointer then points to.
    fn reference(&self, cpu: &mut Cpu, src16: u32, vt: u16, slot: u32) -> Result<u32, u32> {
        let src = linear(cpu, src16);
        let src_vt = super::read_16(cpu, src);
        if src_vt == vt {
            return Ok(super::read_32(cpu, src + 8));
        }
        if vt == VT_BYREF | VT_VARIANT {
            return Ok(src16);
        }
        if src_vt & VT_BYREF != 0 {
            return Err(DISP_E_TYPEMISMATCH);
        }
        // A value of the type: a pointer to the argument's own, which the
        // method may change.
        if src_vt == vt & !VT_BYREF {
            return Ok(add16(src16, 8));
        }
        // A value of another type: a copy changed to the type.
        let base = vt & !VT_BYREF;
        if base & VT_ARRAY != 0 {
            return Err(DISP_E_TYPEMISMATCH);
        }
        let out = changed(cpu, src, base, 0)?;
        let (bytes, _) = out_bytes(cpu, out, base)?;
        let at = linear(cpu, slot);
        super::write_16(cpu, at, base);
        super::poke(cpu, at + 8, &bytes);
        Ok(add16(slot, 8))
    }

    fn free(&self, cpu: &mut Cpu) {
        for &s in &self.bstrs {
            free_bstr(cpu, s);
        }
    }
}

/// The function of the DISPID `memid` that a call of the kind `flags`
/// calls, in the type at `index` of `m` or an interface it inherits
/// from: the function, its library and the place of its type there.
fn find_func(
    cpu: &mut Cpu,
    m: TypeLib,
    index: usize,
    memid: i32,
    flags: u32,
) -> Option<(Func, TypeLib, usize)> {
    search(cpu, m, index, |lib, i, t| {
        let f = t
            .funcs
            .iter()
            .find(|f| f.memid == memid && f.invkind & flags != 0)?;
        Some((f.clone(), lib.clone(), i))
    })
}

/// Invoke(instance, memid, flags, params, result, excepinfo, argerr),
/// the pointers 16:16.
pub fn invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [
        this,
        instance,
        memid,
        flags,
        params,
        result,
        excepinfo,
        argerr,
    ] = args[..]
    else {
        unreachable!()
    };
    let flags = flags & 0xFFFF;
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let dp = linear(cpu, params);
    if linear(cpu, instance) == 0 || dp == 0 {
        return Some(E_INVALIDARG);
    }
    let Some((f, lib, at)) = find_func(cpu, m, index, memid as i32, flags) else {
        return Some(DISP_E_MEMBERNOTFOUND);
    };
    match f.kind {
        FUNC_DISPATCH => {
            let rest = [memid, flags, params, result, excepinfo, argerr];
            dispatch(cpu, instance, &rest)
        }
        FUNC_VIRTUAL | FUNC_PUREVIRTUAL => {
            let call = Invocation {
                instance,
                flags,
                dp,
                result,
                excepinfo,
                argerr,
            };
            call.start(cpu, &f, &lib, at)
        }
        _ => {
            log_not_implemented(cpu, "ITypeInfo::Invoke of a module's function");
            Some(E_NOTIMPL)
        }
    }
}

/// The arguments of an Invoke of a function through the vtable.
struct Invocation {
    instance: u32,
    flags: u32,
    dp: u32,
    result: u32,
    excepinfo: u32,
    argerr: u32,
}

impl Invocation {
    /// Put `arg` at the program's `argerr` and give `e`.
    fn fail(&self, cpu: &mut Cpu, e: u32, arg: u32) -> Option<u32> {
        let at = linear(cpu, self.argerr);
        if at != 0 {
            super::write_16(cpu, at, arg as u16);
        }
        Some(e)
    }

    /// Call `f`, a function of the type at `index` of `lib`, on the
    /// object.
    fn start(&self, cpu: &mut Cpu, f: &Func, lib: &TypeLib, index: usize) -> Option<u32> {
        if f.callconv != CC_CDECL && f.callconv != CC_PASCAL {
            cpu.bus.log_string(&format!(
                "[W16] TYPELIB: Invoke of {}: calling convention {} isn't a 16-bit one",
                f.name.as_deref().unwrap_or("?"),
                f.callconv
            ));
            return Some(DISP_E_BADCALLEE);
        }
        let ret = match &f.ret {
            TypeDesc::Base(VT_HRESULT) => RET_HRESULT,
            TypeDesc::Base(VT_VOID) => RET_VOID,
            ty => match param_vt(cpu, lib, index, ty, 0) {
                Some((vt, _)) if size(vt) <= 4 && vt != VT_R4 => RET_VALUE | vt as u32,
                _ => return Some(DISP_E_BADVARTYPE),
            },
        };
        let target = method_address(cpu, self.instance, (f.vft as u16 / 4) as u32);
        if target == 0 {
            return Some(E_INVALIDARG);
        }

        let dp = self.dp;
        let args16 = super::read_32(cpu, dp + DP_ARGS);
        let named16 = super::read_32(cpu, dp + DP_NAMED);
        let count = super::read_16(cpu, dp + DP_COUNT) as u32;
        let named_count = super::read_16(cpu, dp + DP_NAMED_COUNT) as u32;
        let named = linear(cpu, named16);
        if named_count > count
            || (count > 0 && linear(cpu, args16) == 0)
            || (named_count > 0 && named == 0)
        {
            return Some(E_INVALIDARG);
        }
        // The parameters the arguments go to: not [retval] or [lcid].
        let inputs: Vec<usize> = f
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.flags & (PARAMFLAG_FRETVAL | PARAMFLAG_FLCID) == 0)
            .map(|(i, _)| i)
            .collect();
        let positional = count - named_count;
        if positional as usize > inputs.len() {
            return Some(DISP_E_BADPARAMCOUNT);
        }
        // The argument of each input, by its place in the arguments: the
        // positional ones last first, after the named ones.
        let mut sources: Vec<Option<u32>> = vec![None; inputs.len()];
        for j in 0..positional {
            sources[j as usize] = Some(count - 1 - j);
        }
        let put = self.flags & (INVOKE_PROPERTYPUT | INVOKE_PROPERTYPUTREF) != 0;
        for k in 0..named_count {
            let id = super::read_32(cpu, named + 4 * k) as i32;
            let place = if id == DISPID_PROPERTYPUT && put {
                inputs.len().checked_sub(1)
            } else if id >= 0 {
                inputs.iter().position(|&i| i == id as usize)
            } else {
                None
            };
            match place {
                Some(p) if sources[p].is_none() => sources[p] = Some(k),
                _ => return self.fail(cpu, DISP_E_PARAMNOTFOUND, k),
            }
        }

        let block = ole2::alloc(cpu, 16 * (f.params.len() as u32 + 1));
        let block_at = linear(cpu, block);
        if block_at == 0 {
            return Some(E_OUTOFMEMORY);
        }
        super::poke(cpu, block_at, &vec![0; 16 * (f.params.len() + 1)]);
        let mut call = Call {
            args: Vec::new(),
            bstrs: Vec::new(),
        };
        let mut retval = (0, 0);
        let mut built = Ok(());
        for (i, p) in f.params.iter().enumerate() {
            let slot = add16(block, 16 * i as u32);
            let slot_at = block_at + 16 * i as u32;
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
                call.args.push(slot.to_le_bytes().to_vec());
                continue;
            }
            if p.flags & PARAMFLAG_FLCID != 0 {
                call.args.push(LOCALE_USER_DEFAULT.to_le_bytes().to_vec());
                continue;
            }
            let input = inputs.iter().position(|&n| n == i).unwrap_or(0);
            let place = sources[input];
            let src16 = match place {
                Some(k) => add16(args16, 16 * k),
                None => {
                    // A missing argument: the parameter's default, or "not
                    // there" for an optional VARIANT.
                    let has_default = p.flags & PARAMFLAG_FHASDEFAULT != 0;
                    match (&p.default, vt & !VT_BYREF) {
                        (Some(Value::Bits(dvt, bits)), _) if has_default => {
                            super::write_16(cpu, slot_at, *dvt);
                            super::poke(cpu, slot_at + 8, &bits.to_le_bytes());
                        }
                        (Some(Value::Str(s)), _) if has_default => {
                            let text =
                                typelib::ansi(typelib::page(cpu), s.as_deref().unwrap_or(""));
                            match alloc_bstr(cpu, &text) {
                                0 => {
                                    built = Err((E_OUTOFMEMORY, 0));
                                    break;
                                }
                                b => {
                                    super::write_16(cpu, slot_at, VT_BSTR);
                                    super::write_32(cpu, slot_at + 8, b);
                                }
                            }
                        }
                        (_, VT_VARIANT) if p.flags & PARAMFLAG_FOPT != 0 => {
                            super::write_16(cpu, slot_at, VT_ERROR);
                            super::write_32(cpu, slot_at + 8, DISP_E_PARAMNOTFOUND);
                        }
                        _ => {
                            built = Err((DISP_E_BADPARAMCOUNT, 0));
                            break;
                        }
                    }
                    // The slot holds the argument; a VARIANT* is passed a
                    // pointer to it.
                    if vt == VT_BYREF | VT_VARIANT {
                        call.args.push(slot.to_le_bytes().to_vec());
                        continue;
                    }
                    slot
                }
            };
            let pushed = if vt & VT_BYREF != 0 {
                call.reference(cpu, src16, vt, slot)
                    .map(|p| p.to_le_bytes().to_vec())
            } else {
                let src = linear(cpu, src16);
                call.value(cpu, src, vt)
            };
            match pushed {
                Ok(bytes) => call.args.push(bytes),
                Err(e) => {
                    built = Err((e, place.unwrap_or(0)));
                    break;
                }
            }
        }
        if let Err((e, arg)) = built {
            call.free(cpu);
            free_slots(cpu, block, f.params.len() as u32, retval.1);
            ole2::free(cpu, block);
            return self.fail(cpu, e, arg);
        }

        // The words in the order they are pushed, each argument's high
        // word first so that it lies in memory as it is.
        let mut words: Vec<u16> = Vec::new();
        let mut push = |bytes: &[u8]| {
            for pair in bytes.chunks(2).rev() {
                words.push(u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
            }
        };
        if f.callconv == CC_CDECL {
            call.args.iter().rev().for_each(|a| push(a));
        } else {
            call.args.iter().for_each(|a| push(a));
        }
        push(&self.instance.to_le_bytes());
        let mut data = vec![
            STEP_METHOD,
            self.result,
            self.excepinfo,
            block,
            retval.0 as u32,
            retval.1,
            ret,
            f.params.len() as u32,
        ];
        data.extend(&call.bstrs);
        let resume = Context::of(cpu);
        thunk::call16(
            cpu,
            target,
            &words,
            0,
            resume,
            thunk::THEN_TYPELIB_INVOKE,
            data,
        );
        None
    }
}

/// Call `method` of the 16-bit interface `object` with the dwords and
/// words `args` (`sizes`: `l` a dword, `w` a word) as C calls it; `then`
/// gets what it returned with `data`, and the context `resume` goes on.
fn call_c(
    cpu: &mut Cpu,
    object: u32,
    method: u32,
    args: &[u32],
    sizes: &str,
    resume: Context,
    data: Vec<u32>,
) {
    let target = method_address(cpu, object, method);
    let mut words = Vec::new();
    for (&a, size) in args.iter().zip(sizes.bytes()).rev() {
        if size == b'l' {
            words.push((a >> 16) as u16);
        }
        words.push(a as u16);
    }
    words.push((object >> 16) as u16);
    words.push(object as u16);
    thunk::call16(
        cpu,
        target,
        &words,
        0,
        resume,
        thunk::THEN_TYPELIB_INVOKE,
        data,
    );
}

/// A dispinterface's member: the object's IDispatch, from its
/// QueryInterface, then its Invoke with the arguments `rest` (memid,
/// flags, params, result, excepinfo, argerr), then its Release.
fn dispatch(cpu: &mut Cpu, instance: u32, rest: &[u32; 6]) -> Option<u32> {
    // The IID, IID_NULL for Invoke's riid, and the interface's pointer.
    let block = ole2::alloc(cpu, 36);
    let at = linear(cpu, block);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    super::poke(cpu, at, &IID_IDISPATCH);
    super::poke(cpu, at + 16, &[0; 20]);
    if method_address(cpu, instance, QUERY_INTERFACE) == 0 {
        ole2::free(cpu, block);
        return Some(E_INVALIDARG);
    }
    let mut data = vec![STEP_QUERY, block];
    data.extend(rest);
    let resume = Context::of(cpu);
    call_c(
        cpu,
        instance,
        QUERY_INTERFACE,
        &[block, add16(block, 32)],
        "ll",
        resume,
        data,
    );
    None
}

/// A call of `invoke` or `dispatch` returned `value`: `data` says which.
pub(super) fn then(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    match data.first() {
        Some(&STEP_QUERY) => then_query(cpu, value, data, ctx),
        Some(&STEP_DISPATCH) => then_dispatch(cpu, value, data, ctx),
        _ => then_method(cpu, value, data, ctx),
    }
}

/// QueryInterface for IDispatch returned `value`: Invoke of the
/// interface, or the failure.
fn then_query(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let block = data[1];
    let disp = super::read_32(cpu, linear(cpu, block) + 32);
    if value & 0x8000_0000 != 0 || disp == 0 || method_address(cpu, disp, DISPATCH_INVOKE) == 0 {
        ole2::free(cpu, block);
        return dx_ax(
            ctx,
            if value & 0x8000_0000 != 0 {
                value
            } else {
                E_INVALIDARG
            },
        );
    }
    let [memid, flags, params, result, excepinfo, argerr] = data[2..8] else {
        unreachable!()
    };
    let args = [
        memid,
        add16(block, 16),
        LOCALE_USER_DEFAULT,
        flags,
        params,
        result,
        excepinfo,
        argerr,
    ];
    call_c(
        cpu,
        disp,
        DISPATCH_INVOKE,
        &args,
        "lllwllll",
        *ctx,
        vec![STEP_DISPATCH, block, disp],
    );
    None
}

/// IDispatch::Invoke returned `value`, what Invoke returns after the
/// interface's Release.
fn then_dispatch(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    ole2::free(cpu, data[1]);
    release_then(cpu, data[2], value, ctx)
}

/// Release `object`, then return `value` to Invoke's caller.
fn release_then(cpu: &mut Cpu, object: u32, value: u32, ctx: &mut Context) -> Option<u32> {
    let target = method_address(cpu, object, RELEASE);
    if target == 0 {
        return dx_ax(ctx, value);
    }
    let words = [(object >> 16) as u16, object as u16];
    thunk::call16(
        cpu,
        target,
        &words,
        0,
        *ctx,
        thunk::THEN_OLE_METHOD,
        vec![1, value],
    );
    None
}

/// The method returned `value` in DX:AX: its result in the VARIANT, or
/// its failure in the EXCEPINFO, and what was made for it freed.
fn then_method(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    for &s in &data[D_BSTRS..] {
        free_bstr(cpu, s);
    }
    let result = linear(cpu, data[D_RESULT]);
    let block = data[D_BLOCK];
    let (retval_vt, retval) = (data[D_RETVAL_VT] as u16, data[D_RETVAL]);
    let ret = data[D_RET];
    free_slots(cpu, block, data[D_PARAMS], retval);
    if ret == RET_HRESULT && value & 0x8000_0000 != 0 {
        let excepinfo = linear(cpu, data[D_EXCEPINFO]);
        if excepinfo != 0 {
            super::poke(cpu, excepinfo, &[0; EXCEPINFO_SIZE]);
            super::write_32(cpu, excepinfo + EI_SCODE, value);
        }
        ole2::free(cpu, block);
        return dx_ax(ctx, DISP_E_EXCEPTION);
    }
    let mut out = [0u8; 16];
    if retval != 0 {
        let at = linear(cpu, retval);
        if retval_vt == VT_VARIANT {
            if let Some(v) = super::peek(cpu, at, 16) {
                out.copy_from_slice(&v);
            }
        } else {
            out[..2].copy_from_slice(&retval_vt.to_le_bytes());
            if let Some(v) = super::peek(cpu, at, size(retval_vt)) {
                out[8..8 + v.len()].copy_from_slice(&v);
            }
        }
    } else if ret & RET_VALUE != 0 {
        let vt = ret as u16;
        out[..2].copy_from_slice(&vt.to_le_bytes());
        let n = size(vt);
        out[8..8 + n].copy_from_slice(&value.to_le_bytes()[..n]);
    }
    ole2::free(cpu, block);
    if result != 0 {
        super::poke(cpu, result, &out);
        return dx_ax(ctx, S_OK);
    }
    // Nobody takes the value: what it holds goes.
    let vt = u16::from_le_bytes([out[0], out[1]]);
    let p = u32::from_le_bytes([out[8], out[9], out[10], out[11]]);
    match vt {
        VT_BSTR if p != 0 => free_bstr(cpu, p),
        VT_DISPATCH | VT_UNKNOWN if p != 0 => return release_then(cpu, p, S_OK, ctx),
        _ => {}
    }
    dx_ax(ctx, S_OK)
}
