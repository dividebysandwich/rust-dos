//! Floating point: the math functions, which take doubles on the stack
//! and return them in ST(0); the `_CI` functions the compiler calls with
//! their arguments on the FPU stack; `_ftol`; and the FPU's control and
//! status words as `_controlfp` and `_statusfp` show them.

use super::*;

/// A math function of one double.
fn unary(cpu: &mut Cpu, args: &[u32], f: fn(f64) -> f64) -> Option<u32> {
    let x = f64_arg(args, 0);
    let y = f(x);
    if y.is_nan() && !x.is_nan() {
        set_errno(cpu, EDOM);
    } else if y.is_infinite() && x.is_finite() {
        set_errno(cpu, ERANGE);
    }
    ret_f64(cpu, y)
}

/// A math function of two doubles.
fn binary(cpu: &mut Cpu, args: &[u32], f: fn(f64, f64) -> f64) -> Option<u32> {
    let (x, y) = (f64_arg(args, 0), f64_arg(args, 2));
    let z = f(x, y);
    if z.is_nan() && !x.is_nan() && !y.is_nan() {
        set_errno(cpu, EDOM);
    } else if z.is_infinite() && x.is_finite() && y.is_finite() {
        set_errno(cpu, ERANGE);
    }
    ret_f64(cpu, z)
}

/// log and log10 of 0 are -HUGE_VAL with ERANGE, of a negative number
/// NaN with EDOM.
fn log_of(cpu: &mut Cpu, args: &[u32], f: fn(f64) -> f64) -> Option<u32> {
    let x = f64_arg(args, 0);
    if x == 0.0 {
        set_errno(cpu, ERANGE);
    } else if x < 0.0 {
        set_errno(cpu, EDOM);
    }
    ret_f64(cpu, f(x))
}

pub fn sqrt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::sqrt)
}
pub fn sin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::sin)
}
pub fn cos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::cos)
}
pub fn tan(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::tan)
}
pub fn asin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::asin)
}
pub fn acos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::acos)
}
pub fn atan(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::atan)
}
pub fn sinh(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::sinh)
}
pub fn cosh(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::cosh)
}
pub fn tanh(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::tanh)
}
pub fn exp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unary(cpu, args, f64::exp)
}
pub fn log(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    log_of(cpu, args, f64::ln)
}
pub fn log10(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    log_of(cpu, args, f64::log10)
}
pub fn floor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, f64_arg(args, 0).floor())
}
pub fn ceil(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, f64_arg(args, 0).ceil())
}
pub fn fabs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, f64_arg(args, 0).abs())
}
pub fn atan2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    binary(cpu, args, f64::atan2)
}
pub fn pow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    binary(cpu, args, f64::powf)
}
pub fn fmod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    binary(cpu, args, |x, y| x % y)
}
pub fn hypot(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    binary(cpu, args, f64::hypot)
}

/// _cabs(z): z is a struct of two doubles, passed by value.
pub fn cabs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    binary(cpu, args, f64::hypot)
}

/// ldexp(x, exp).
pub fn ldexp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let x = f64_arg(args, 0);
    let e = (args[2] as i32).clamp(-2200, 2200);
    let y = x * 2f64.powi(e.clamp(-1100, 1100)) * 2f64.powi(e - e.clamp(-1100, 1100));
    if y.is_infinite() && x.is_finite() {
        set_errno(cpu, ERANGE);
    }
    ret_f64(cpu, y)
}

/// frexp(x, &exp): the mantissa in [0.5, 1).
pub fn frexp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let x = f64_arg(args, 0);
    let (m, e) = if x == 0.0 || !x.is_finite() {
        (x, 0)
    } else {
        let mut e = x.abs().log2().floor() as i32 + 1;
        let mut m = x / 2f64.powi(e);
        // log2 can be one off at the edges.
        if m.abs() >= 1.0 {
            m /= 2.0;
            e += 1;
        } else if m.abs() < 0.5 {
            m *= 2.0;
            e -= 1;
        }
        (m, e)
    };
    write32(cpu, args[2], e as u32)?;
    ret_f64(cpu, m)
}

/// modf(x, &integer): the fraction.
pub fn modf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let x = f64_arg(args, 0);
    let int = x.trunc();
    put(cpu, args[2], &int.to_le_bytes())?;
    ret_f64(cpu, if x.is_infinite() { 0.0 } else { x - int })
}

/// _isnan(x), _finite(x), _fpclass(x).
pub fn isnan(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(f64_arg(args, 0).is_nan() as u32)
}

pub fn finite(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(f64_arg(args, 0).is_finite() as u32)
}

pub fn fpclass(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let x = f64_arg(args, 0);
    let negative = x.is_sign_negative();
    Some(match x.classify() {
        std::num::FpCategory::Nan => {
            if x.to_bits() & (1 << 51) != 0 {
                2
            } else {
                1
            }
        }
        std::num::FpCategory::Infinite => {
            if negative {
                4
            } else {
                0x200
            }
        }
        std::num::FpCategory::Normal => {
            if negative {
                8
            } else {
                0x100
            }
        }
        std::num::FpCategory::Subnormal => {
            if negative {
                0x10
            } else {
                0x80
            }
        }
        std::num::FpCategory::Zero => {
            if negative {
                0x20
            } else {
                0x40
            }
        }
    })
}

/// _copysign(x, y), _chgsign(x), _logb(x), _scalb(x, exp), _nextafter.
pub fn copysign(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, f64_arg(args, 0).copysign(f64_arg(args, 2)))
}

pub fn chgsign(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, -f64_arg(args, 0))
}

pub fn logb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let x = f64_arg(args, 0);
    ret_f64(
        cpu,
        if x == 0.0 {
            f64::NEG_INFINITY
        } else {
            x.abs().log2().floor()
        },
    )
}

pub fn scalb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let e = args[2] as i32;
    ret_f64(cpu, f64_arg(args, 0) * 2f64.powi(e))
}

// The _CI functions: arguments on the FPU stack, the result in ST(0).

/// A _CI function of ST(0), which it replaces.
fn ci_unary(cpu: &mut Cpu, f: fn(f64) -> f64) -> Option<u32> {
    let x = cpu.fpu_get_f64(0);
    let y = f(x);
    if y.is_nan() && !x.is_nan() {
        set_errno(cpu, EDOM);
    } else if y.is_infinite() && x.is_finite() {
        set_errno(cpu, ERANGE);
    }
    cpu.fpu_set_f64(0, y);
    Some(cpu.eax())
}

/// A _CI function of ST(1) and ST(0): both popped, the result pushed.
fn ci_binary(cpu: &mut Cpu, f: fn(f64, f64) -> f64) -> Option<u32> {
    let y = cpu.fpu_get_f64(0);
    let x = cpu.fpu_get_f64(1);
    let z = f(x, y);
    if z.is_nan() && !x.is_nan() && !y.is_nan() {
        set_errno(cpu, EDOM);
    } else if z.is_infinite() && x.is_finite() && y.is_finite() {
        set_errno(cpu, ERANGE);
    }
    cpu.fpu_drop();
    cpu.fpu_set_f64(0, z);
    Some(cpu.eax())
}

pub fn ci_sqrt(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::sqrt)
}
pub fn ci_sin(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::sin)
}
pub fn ci_cos(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::cos)
}
pub fn ci_tan(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::tan)
}
pub fn ci_asin(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::asin)
}
pub fn ci_acos(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::acos)
}
pub fn ci_atan(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::atan)
}
pub fn ci_sinh(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::sinh)
}
pub fn ci_cosh(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::cosh)
}
pub fn ci_tanh(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::tanh)
}
pub fn ci_exp(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::exp)
}
pub fn ci_log(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::ln)
}
pub fn ci_log10(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_unary(cpu, f64::log10)
}
pub fn ci_pow(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_binary(cpu, f64::powf)
}
pub fn ci_atan2(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_binary(cpu, f64::atan2)
}
pub fn ci_fmod(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    ci_binary(cpu, |x, y| x % y)
}

/// _ftol(): ST(0) popped, truncated to a 64-bit integer in EDX:EAX.
pub fn ftol(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let x = cpu.fpu_get_f64(0);
    cpu.fpu_drop();
    let v = if x.is_nan() || x.abs() >= -(i64::MIN as f64) {
        i64::MIN
    } else {
        x.trunc() as i64
    };
    ret_u64(cpu, v as u64)
}

// The FPU's control and status words.

/// The x87 control word as MSVC's flags.
fn control_flags(cw: u16) -> u32 {
    let cw = cw as u32;
    let mut flags = 0;
    for (x87, msvc) in [
        (0x01, 0x10),
        (0x02, 0x80000),
        (0x04, 0x08),
        (0x08, 0x04),
        (0x10, 0x02),
        (0x20, 0x01),
    ] {
        if cw & x87 != 0 {
            flags |= msvc;
        }
    }
    flags |= match (cw >> 10) & 3 {
        0 => 0,
        1 => 0x100,
        2 => 0x200,
        _ => 0x300,
    };
    flags |= match (cw >> 8) & 3 {
        0 => 0x20000,
        2 => 0x10000,
        _ => 0,
    };
    if cw & 0x1000 != 0 {
        flags |= 0x40000;
    }
    flags
}

/// MSVC's flags as the x87 control word.
fn control_word(flags: u32) -> u16 {
    let mut cw = 0x40u32;
    for (x87, msvc) in [
        (0x01, 0x10),
        (0x02, 0x80000),
        (0x04, 0x08),
        (0x08, 0x04),
        (0x10, 0x02),
        (0x20, 0x01),
    ] {
        if flags & msvc != 0 {
            cw |= x87;
        }
    }
    cw |= match flags & 0x300 {
        0x100 => 0x400,
        0x200 => 0x800,
        0x300 => 0xC00,
        _ => 0,
    };
    cw |= match flags & 0x30000 {
        0x20000 => 0,
        0x10000 => 0x200,
        _ => 0x300,
    };
    if flags & 0x40000 != 0 {
        cw |= 0x1000;
    }
    cw as u16
}

/// _control87(new, mask): the bits of `mask` set from `new`; the control
/// word after, as flags.
pub fn control87(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = control_flags(cpu.fpu_control);
    let new = (old & !args[1]) | (args[0] & args[1]);
    if args[1] != 0 {
        cpu.fpu_control = control_word(new);
    }
    Some(control_flags(cpu.fpu_control))
}

/// _controlfp(new, mask): _control87 without the denormal mask.
pub fn controlfp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    control87(cpu, &[args[0], args[1] & !0x80000])
}

/// The x87 status word's exceptions as MSVC's _SW flags.
fn status_flags(sw: u16) -> u32 {
    let mut flags = 0;
    for (x87, msvc) in [
        (0x01, 0x10),
        (0x02, 0x80000),
        (0x04, 0x08),
        (0x08, 0x04),
        (0x10, 0x02),
        (0x20, 0x01),
    ] {
        if sw & x87 != 0 {
            flags |= msvc;
        }
    }
    flags
}

/// _statusfp(): the exceptions there were.
pub fn statusfp(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(status_flags(cpu.fpu_status_word()))
}

/// _clearfp(): the exceptions there were, cleared.
pub fn clearfp(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let sw = cpu.fpu_status_word();
    crate::instructions::fpu::control::set_status_word(cpu, sw & !0x80FF);
    Some(status_flags(sw))
}

/// _fpreset(): the FPU as at start, its stack empty.
pub fn fpreset(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let sw = cpu.fpu_status_word();
    crate::instructions::fpu::control::set_status_word(cpu, sw & !0xB8FF);
    cpu.fpu_tags = [crate::cpu::FPU_TAG_EMPTY; 8];
    cpu.fpu_control = 0x027F;
    Some(0)
}
