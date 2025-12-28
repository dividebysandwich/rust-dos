use iced_x86::{Instruction, Mnemonic};
use crate::cpu::{Cpu, CpuFlags};

pub mod utils;
pub mod fpu;
pub mod math;
pub mod logic;
pub mod control;
pub mod transfer;
pub mod string;
pub mod misc;

pub fn execute_instruction(cpu: &mut Cpu, instr: &Instruction) {
    let zf_before = cpu.get_cpu_flag(CpuFlags::ZF);
    
    match instr.mnemonic() {

        // Source: https://tizee.github.io/x86_ref_book_web/

        // --- Data Transfer ---
        Mnemonic::Mov | Mnemonic::Xchg | Mnemonic::Lea | 
        Mnemonic::Lds | Mnemonic::Les |
        Mnemonic::Push | Mnemonic::Pop | Mnemonic::Pusha | Mnemonic::Popa | 
        Mnemonic::Pushf | Mnemonic::Popf |
        Mnemonic::In | Mnemonic::Out | Mnemonic::Cbw | Mnemonic::Cwd |
        Mnemonic::Xlatb | Mnemonic::Lahf | Mnemonic::Sahf => {
            transfer::handle(cpu, instr);
        }

        // --- Math / Arithmetic ---
        Mnemonic::Add | Mnemonic::Sub | Mnemonic::Adc | Mnemonic::Sbb |
        Mnemonic::Inc | Mnemonic::Dec | Mnemonic::Neg | Mnemonic::Aam |
        Mnemonic::Mul | Mnemonic::Imul | Mnemonic::Div | Mnemonic::Idiv |
        Mnemonic::Cmp | Mnemonic::Aaa | Mnemonic::Das | Mnemonic::Daa |
        Mnemonic::Aas => {
            math::handle(cpu, instr);
        }

        // --- FPU ---
        // --- Math & Arithmetic ---
        Mnemonic::Fadd | Mnemonic::Faddp | Mnemonic::Fiadd |
        Mnemonic::Fsub | Mnemonic::Fsubp | Mnemonic::Fsubr | Mnemonic::Fsubrp |
        Mnemonic::Fisub | Mnemonic::Fisubr |
        Mnemonic::Fmul | Mnemonic::Fmulp | Mnemonic::Fimul |
        Mnemonic::Fdiv | Mnemonic::Fdivp | Mnemonic::Fdivr | Mnemonic::Fdivrp |
        Mnemonic::Fidiv | Mnemonic::Fidivr |
        Mnemonic::Fsqrt | Mnemonic::Fscale | Mnemonic::Fprem | Mnemonic::Fprem1 |
        Mnemonic::Frndint | Mnemonic::Fxtract | Mnemonic::Fabs | Mnemonic::Fchs |
        Mnemonic::F2xm1 | Mnemonic::Fyl2x | Mnemonic::Fyl2xp1 |

        // --- Transcendental ---
        Mnemonic::Fsin | Mnemonic::Fcos | Mnemonic::Fsincos |
        Mnemonic::Fptan | Mnemonic::Fpatan |

        // --- Data Transfer ---
        Mnemonic::Fld | Mnemonic::Fst | Mnemonic::Fstp |
        Mnemonic::Fild | Mnemonic::Fist | Mnemonic::Fistp | Mnemonic::Fisttp |
        Mnemonic::Fbld | Mnemonic::Fbstp |
        Mnemonic::Fxch | Mnemonic::Fld1 | Mnemonic::Fldz | 
        Mnemonic::Fldpi | Mnemonic::Fldl2e | Mnemonic::Fldl2t | 
        Mnemonic::Fldlg2 | Mnemonic::Fldln2 |
        
        // --- Comparison ---
        Mnemonic::Fcom | Mnemonic::Fcomp | Mnemonic::Fcompp |
        Mnemonic::Ficom | Mnemonic::Ficomp |
        Mnemonic::Ftst | Mnemonic::Fxam |
        Mnemonic::Fcomi | Mnemonic::Fcomip | Mnemonic::Fucomi | Mnemonic::Fucomip |

        // --- Control & State ---
        Mnemonic::Finit | Mnemonic::Fninit |
        Mnemonic::Fldcw | Mnemonic::Fstcw | Mnemonic::Fnstcw |
        Mnemonic::Fstsw | Mnemonic::Fnstsw |
        Mnemonic::Fclex | Mnemonic::Fnclex |
        Mnemonic::Fsave | Mnemonic::Fnsave | Mnemonic::Frstor |
        Mnemonic::Fstenv | Mnemonic::Fnstenv | Mnemonic::Fldenv |
        Mnemonic::Fnop | Mnemonic::Ffree | Mnemonic::Fincstp | 
        Mnemonic::Fdecstp => {
            fpu::handle(cpu, instr);
        }

        // --- Logic / Bitwise ---
        Mnemonic::And | Mnemonic::Or | Mnemonic::Xor | Mnemonic::Not | Mnemonic::Test |
        Mnemonic::Shl | Mnemonic::Shr | Mnemonic::Sal | Mnemonic::Sar |
        Mnemonic::Rol | Mnemonic::Ror | Mnemonic::Rcl | Mnemonic::Rcr => {
            logic::handle(cpu, instr);
        }

        // --- Control Flow ---
        Mnemonic::Jmp | Mnemonic::Call | Mnemonic::Ret | Mnemonic::Retf |
        Mnemonic::Loop | Mnemonic::Je | Mnemonic::Jne | Mnemonic::Jcxz |
        Mnemonic::Jb | Mnemonic::Jbe | Mnemonic::Ja | Mnemonic::Jae |
        Mnemonic::Jl | Mnemonic::Jle | Mnemonic::Jg | Mnemonic::Jge |
        Mnemonic::Jo | Mnemonic::Js | Mnemonic::Jns | Mnemonic::Loopne | 
        Mnemonic::Loope => {
            control::handle(cpu, instr);
        }

        // --- Strings ---
        Mnemonic::Movsb | Mnemonic::Movsw | Mnemonic::Stosb | Mnemonic::Stosw |
        Mnemonic::Lodsb | Mnemonic::Lodsw | Mnemonic::Cmpsb | Mnemonic::Cmpsw |
        Mnemonic::Scasb | Mnemonic::Scasw => {
            string::handle(cpu, instr);
        }

        // --- System / Misc ---
        Mnemonic::Int | Mnemonic::Nop | Mnemonic::Wait | Mnemonic::Hlt | 
        Mnemonic::Stc | Mnemonic::Clc | Mnemonic::Std | Mnemonic::Cld | 
        Mnemonic::Cli | Mnemonic::Sti | Mnemonic::Cmc | Mnemonic::Into |
        Mnemonic::Iret | Mnemonic::Leave | Mnemonic::Enter
        => { 
            misc::handle(cpu, instr);
        }

        _ => {
            cpu.bus.log_string(&format!("[CPU] Unhandled: {}", instr));
        }
    }

    let zf_after = cpu.get_cpu_flag(CpuFlags::ZF);
    if cpu.debug_qb_print && zf_before != zf_after {
        cpu.bus.log_string(&format!(
            "[ZF-CHANGED] {:?} changed ZF from {} to {} at {:04X}:{:04X}",
            instr.mnemonic(), zf_before, zf_after, cpu.cs, cpu.ip.wrapping_sub(instr.len() as u16)
        ));
    }
}