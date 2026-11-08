//! The CPU and the FPU: instructions on the interpreter and the dynamic
//! recompiler, protected mode, the MMX, SSE and K6 extensions, and the
//! instruction suites (SingleStepTests, test386).
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/dyndiff/mod.rs"]
mod dyndiff;
#[path = "../common/pmrig/mod.rs"]
mod pmrig;
mod testrunners;
#[path = "../common/win16/mod.rs"]
mod win16;

mod alu_tests;
mod cpu32_tests;
mod cpu_control_tests;
mod cpu_logic_tests;
mod cpu_math_tests;
mod cpu_misc_tests;
mod cpu_string_tests;
mod cpu_tests;
mod cpu_transfer_tests;
mod dyndiff_tests;
mod dynrec_speed;
mod dynrec_tests;
mod f80_tests;
mod fpu_arithmetic_tests;
mod fpu_comparison_tests;
mod fpu_control_tests;
mod fpu_data_tests;
mod fpu_speed;
mod fpu_transcendental_tests;
mod idle_tests;
mod instr_tests;
mod k6_tests;
mod misc_tests;
mod mmx_tests;
mod p6_tests;
mod pentium_tests;
mod platform_tests;
mod pm_tests;
mod sse_tests;
mod sst386;
mod string_bulk_tests;
mod test386;
