use iced_x86::{Instruction, Mnemonic};
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::interrupts;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {

        // INT n: Software Interrupt
        // Triggers the interrupt handler for the vector specified by the immediate operand.
        Mnemonic::Int => {
            let int_num = instr.immediate8();
            interrupts::handle_interrupt(cpu, int_num);
        }

        // INTO: Interrupt on Overflow
        // Triggers Interrupt 4 if the Overflow Flag (OF) is set.
        Mnemonic::Into => {
            if cpu.get_cpu_flag(CpuFlags::OF) {
                interrupts::handle_interrupt(cpu, 4);
            }
        }

        // IRET: Interrupt Return
        // Pops IP, CS, and Flags from the stack.
        Mnemonic::Iret => {
            cpu.ip = cpu.pop();
            cpu.cs = cpu.pop();
            let flags = cpu.pop();
    
            // Restore flags (preserving reserved bits 1, 3, 5, 15)
            // 8086 Reserved: 1111_0000_0000_0010 (0xF002) are usually stuck/reserved
            // Simple mask: Preserve current reserved bits, write writable ones.
            // For simplicity in emulator: Write all, force bit 1 always ON.
            cpu.set_cpu_flags(CpuFlags::from_bits_truncate(flags));
        }

        // HLT: Halt Processor
        // Stops execution until an interrupt occurs.
        Mnemonic::Hlt => {
            cpu.state = CpuState::Halted;
        }

        // LEAVE: High Level Procedure Exit
        // Reverses the action of a previous ENTER instruction.
        // 1. MOV SP, BP (Release stack frame)
        // 2. POP BP     (Restore caller's base pointer)
        Mnemonic::Leave => {
            cpu.sp = cpu.bp;
            cpu.bp = cpu.pop();
        }

        // ENTER: High Level Procedure Entry
        // Creates a stack frame for a procedure.
        // Op0: Size of local variables (bytes)
        // Op1: Nesting Level (0-31)
        Mnemonic::Enter => {
            let size = instr.immediate16();
            //let level = instr.immediate8() & 0x1F; // Level is modulo 32

            // Explicitly read the level byte from memory to avoid decoding ambiguity.
            // ENTER is 4 bytes: [Opcode, SizeLO, SizeHI, Level]
            // cpu.ip points to the NEXT instruction, so back up 1 byte to find Level.
            // (Instruction is 4 bytes long. Level is at offset 3)
            let level_addr = (cpu.cs as u32 * 16 + cpu.ip as u32).wrapping_sub(1);
            let level = cpu.bus.read_8(level_addr as usize) & 0x1F;

            // Push Caller's BP
            cpu.push(cpu.bp);
    
            // Capture Frame Pointer (Current SP)
            let frame_ptr = cpu.sp;

            // If Nested, copy pointers from previous frame
            if level > 0 {
                // We walk down the previous frame's display array
                // Loop runs level-1 times
                let mut temp_bp = cpu.bp; 
        
                for _ in 1..level {
                    temp_bp = temp_bp.wrapping_sub(2);
                    // Read 16-bit pointer from Stack Segment
                    let addr = cpu.get_physical_addr(cpu.ss, temp_bp);
                    let ptr_val = cpu.bus.read_16(addr);
                    cpu.push(ptr_val);
                }
        
                // Push the new frame pointer to finish the display array
                cpu.push(frame_ptr);
            }

            // Set BP to the new Frame Pointer
            cpu.bp = frame_ptr;

            // Allocate Local Variables space
            cpu.sp = cpu.sp.wrapping_sub(size);
        }

        Mnemonic::Stc => cpu.set_cpu_flag(CpuFlags::CF, true),
        Mnemonic::Clc => cpu.set_cpu_flag(CpuFlags::CF, false),
        Mnemonic::Std => cpu.set_dflag(true),
        Mnemonic::Cld => cpu.set_dflag(false),
        Mnemonic::Cmc => {
            let cf = cpu.get_cpu_flag(CpuFlags::CF);
            cpu.set_cpu_flag(CpuFlags::CF, !cf);
        }
        Mnemonic::Sti => { /* Enable Interrupts */ },
        Mnemonic::Cli => { /* Disable Interrupts */ },
        Mnemonic::Wait => { /* Wait for Interrupt */ },
        Mnemonic::Nop => { /* No Operation */ },
        
        _ => { cpu.bus.log_string(&format!("[MISC] Unsupported instruction: {:?}", instr.mnemonic())); }
    }
}