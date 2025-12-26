use iced_x86::{Instruction, Mnemonic};
use crate::cpu::Cpu;

pub mod utils;
pub mod fpu;
pub mod math;
pub mod logic;
pub mod control;
pub mod transfer;
pub mod string;
pub mod misc;

pub fn execute_instruction(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
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
        Mnemonic::Inc | Mnemonic::Dec | Mnemonic::Neg |
        Mnemonic::Mul | Mnemonic::Imul | Mnemonic::Div | Mnemonic::Idiv |
        Mnemonic::Cmp | Mnemonic::Aaa | Mnemonic::Das | Mnemonic::Daa=> {
            math::handle(cpu, instr);
        }

        // --- FPU ---
        Mnemonic::Fld | Mnemonic::Fild | Mnemonic::Fistp | 
        Mnemonic::Fdiv | Mnemonic::Fsubp | Mnemonic::Fninit | 
        Mnemonic::Fnclex | Mnemonic::Fldcw | Mnemonic::Fstp => {
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
        Mnemonic::Jo | Mnemonic::Js | Mnemonic::Jns | Mnemonic::Iret=> {
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
        Mnemonic::Cli | Mnemonic::Sti | Mnemonic::Cmc=> { 
            misc::handle(cpu, instr);
        }

        _ => {
            cpu.bus.log_string(&format!("[CPU] Unhandled: {}", instr));
        }
    }
}