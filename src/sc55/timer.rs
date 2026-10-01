//! The H8/532's timers: three 16-bit free-running timers and an 8-bit
//! one. A port of Nuked-SC55's `mcu_timer.cpp`.

use super::machine::{
    DEV_TMR_TCNT, DEV_TMR_TCORA, DEV_TMR_TCORB, DEV_TMR_TCR, DEV_TMR_TCSR, INT_FRT0_FOVI, INT_FRT0_OCIA,
    INT_TIMER_CMIA, INT_TIMER_CMIB, INT_TIMER_OVI, set_request,
};

const TMR_TCR_CCLR0: u8 = 1 << 3;
const TMR_TCR_CCLR1: u8 = 1 << 4;
const TMR_TCR_OVIE: u8 = 1 << 5;
const TMR_TCR_CMIEA: u8 = 1 << 6;
const TMR_TCR_CMIEB: u8 = 1 << 7;
const TMR_TCSR_BIT4: u8 = 1 << 4;
const TMR_TCSR_OVF: u8 = 1 << 5;
const TMR_TCSR_CMFA: u8 = 1 << 6;
const TMR_TCSR_CMFB: u8 = 1 << 7;

const FRT_TCR_OVIE: u8 = 1 << 4;
const FRT_TCR_OCIEA: u8 = 1 << 5;
const FRT_TCR_OCIEB: u8 = 1 << 6;
const FRT_TCSR_CCLRA: u8 = 1 << 0;
const FRT_TCSR_OVF: u8 = 1 << 4;
const FRT_TCSR_OCFA: u8 = 1 << 5;
const FRT_TCSR_OCFB: u8 = 1 << 6;

/// Cycles between counts, by the clock select bits.
const FRT_STEP_GENERIC: [u8; 4] = [4, 8, 32, 2];
const FRT_STEP_MK1: [u8; 4] = [4, 8, 32, 4];
/// 0: doesn't count.
const TMR_STEP_GENERIC: [u16; 8] = [0, 8, 64, 1024, 0, 2, 2, 2];
const TMR_STEP_MK1: [u16; 8] = [0, 8, 64, 1024, 0, 4, 4, 4];

#[derive(Clone, Copy, Default)]
struct Frt {
    deadline: u64,
    tcr: u8,
    tcsr: u8,
    frc: u16,
    ocra: u16,
    ocrb: u16,
    icr: u16,
    status_rd: u8,
    stride: u8,
}

#[derive(Clone, Copy, Default)]
struct Tmr {
    deadline: u64,
    stride: u16,
    tcr: u8,
    tcsr: u8,
    tcora: u8,
    tcorb: u8,
    tcnt: u8,
    status_rd: u8,
}

pub(super) struct Timer {
    cycles: u64,
    frt: [Frt; 3],
    tmr: Tmr,
    tempreg: u8,
    frt_step: [u8; 4],
    tmr_step: [u16; 8],
}

/// The next multiple of `interval`, a power of two, from `value`.
fn align_forward(value: u64, interval: u64) -> u64 {
    (value + (interval - 1)) & !(interval - 1)
}

impl Timer {
    pub fn new(is_mk1: bool) -> Timer {
        let mut timer = Timer {
            cycles: 0,
            frt: [Frt::default(); 3],
            tmr: Tmr::default(),
            tempreg: 0,
            frt_step: if is_mk1 { FRT_STEP_MK1 } else { FRT_STEP_GENERIC },
            tmr_step: if is_mk1 { TMR_STEP_MK1 } else { TMR_STEP_GENERIC },
        };
        timer.reset();
        timer
    }

    pub fn reset(&mut self) {
        for frt in &mut self.frt {
            *frt = Frt { ocra: 0xffff, ocrb: 0xffff, stride: 4, ..Frt::default() };
        }
        self.tmr = Tmr { deadline: u64::MAX, tcsr: TMR_TCSR_BIT4, tcora: 0xff, tcorb: 0xff, ..Tmr::default() };
    }

    pub fn write_frt(&mut self, address: usize, data: u8, irq: &mut u32) {
        let t = (address >> 4).wrapping_sub(1);
        if t > 2 {
            return;
        }
        let cycles = self.cycles;
        let step = self.frt_step;
        let tempreg = self.tempreg;
        let frt = &mut self.frt[t];
        match address & 0x0f {
            0 => {
                frt.tcr = data;
                let stride = step[(frt.tcr & 3) as usize];
                frt.deadline = align_forward(cycles, stride as u64);
                frt.stride = stride;
            }
            1 => {
                frt.tcsr &= !0xf;
                frt.tcsr |= data & 0xf;
                let t = t as u32;
                for (flag, source) in [
                    (FRT_TCSR_OVF, INT_FRT0_FOVI + t * 4),
                    (FRT_TCSR_OCFA, INT_FRT0_OCIA + t * 4),
                    (FRT_TCSR_OCFB, INT_FRT0_OCIA + 1 + t * 4),
                ] {
                    if data & flag == 0 && frt.status_rd & flag != 0 {
                        frt.tcsr &= !flag;
                        frt.status_rd &= !flag;
                        set_request(irq, source, false);
                    }
                }
            }
            2 | 4 | 6 | 8 => self.tempreg = data,
            3 => frt.frc = ((tempreg as u16) << 8) | data as u16,
            5 => frt.ocra = ((tempreg as u16) << 8) | data as u16,
            7 => frt.ocrb = ((tempreg as u16) << 8) | data as u16,
            9 => frt.icr = ((tempreg as u16) << 8) | data as u16,
            _ => {}
        }
    }

    pub fn read_frt(&mut self, address: usize) -> u8 {
        let t = (address >> 4).wrapping_sub(1);
        if t > 2 {
            return 0xff;
        }
        let frt = &mut self.frt[t];
        let latch = |value: u16, tempreg: &mut u8| {
            *tempreg = value as u8;
            (value >> 8) as u8
        };
        match address & 0x0f {
            0 => frt.tcr,
            1 => {
                let ret = frt.tcsr;
                frt.status_rd |= frt.tcsr & 0xf0;
                ret
            }
            2 => latch(frt.frc, &mut self.tempreg),
            4 => latch(frt.ocra, &mut self.tempreg),
            6 => latch(frt.ocrb, &mut self.tempreg),
            8 => latch(frt.icr, &mut self.tempreg),
            3 | 5 | 7 | 9 => self.tempreg,
            _ => 0xff,
        }
    }

    pub fn write_tmr(&mut self, address: usize, data: u8, irq: &mut u32) {
        let tmr = &mut self.tmr;
        match address {
            DEV_TMR_TCR => {
                tmr.tcr = data;
                let stride = self.tmr_step[(tmr.tcr & 7) as usize];
                tmr.deadline = if stride == 0 { u64::MAX } else { align_forward(self.cycles, stride as u64) };
                tmr.stride = stride;
            }
            DEV_TMR_TCSR => {
                tmr.tcsr &= !0xf;
                tmr.tcsr |= data & 0xf;
                for (flag, source) in
                    [(TMR_TCSR_OVF, INT_TIMER_OVI), (TMR_TCSR_CMFA, INT_TIMER_CMIA), (TMR_TCSR_CMFB, INT_TIMER_CMIB)]
                {
                    if data & flag == 0 && tmr.status_rd & flag != 0 {
                        tmr.tcsr &= !flag;
                        tmr.status_rd &= !flag;
                        set_request(irq, source, false);
                    }
                }
            }
            DEV_TMR_TCORA => tmr.tcora = data,
            DEV_TMR_TCORB => tmr.tcorb = data,
            DEV_TMR_TCNT => tmr.tcnt = data,
            _ => {}
        }
    }

    pub fn read_tmr(&mut self, address: usize) -> u8 {
        let tmr = &mut self.tmr;
        match address {
            DEV_TMR_TCR => tmr.tcr,
            DEV_TMR_TCSR => {
                let ret = tmr.tcsr;
                tmr.status_rd |= tmr.tcsr & (TMR_TCSR_OVF | TMR_TCSR_CMFA | TMR_TCSR_CMFB);
                ret
            }
            DEV_TMR_TCORA => tmr.tcora,
            DEV_TMR_TCORB => tmr.tcorb,
            DEV_TMR_TCNT => tmr.tcnt,
            _ => 0xff,
        }
    }

    fn clock_frt(&mut self, id: usize, irq: &mut u32) {
        let frt = &mut self.frt[id];
        let matcha = frt.frc == frt.ocra;
        let matchb = frt.frc == frt.ocrb;
        if frt.tcsr & FRT_TCSR_CCLRA != 0 && matcha {
            frt.frc = 0;
        } else {
            frt.frc = frt.frc.wrapping_add(1);
            if frt.frc == 0 {
                frt.tcsr |= FRT_TCSR_OVF;
            }
        }
        if matcha {
            frt.tcsr |= FRT_TCSR_OCFA;
        }
        if matchb {
            frt.tcsr |= FRT_TCSR_OCFB;
        }
        let id = id as u32;
        if frt.tcr & FRT_TCR_OVIE != 0 && frt.tcsr & FRT_TCSR_OVF != 0 {
            set_request(irq, INT_FRT0_FOVI + id * 4, true);
        }
        if frt.tcr & FRT_TCR_OCIEA != 0 && frt.tcsr & FRT_TCSR_OCFA != 0 {
            set_request(irq, INT_FRT0_OCIA + id * 4, true);
        }
        if frt.tcr & FRT_TCR_OCIEB != 0 && frt.tcsr & FRT_TCSR_OCFB != 0 {
            set_request(irq, INT_FRT0_OCIA + 1 + id * 4, true);
        }
    }

    fn clock_tmr(&mut self, irq: &mut u32) {
        let tmr = &mut self.tmr;
        let matcha = tmr.tcnt == tmr.tcora;
        let matchb = tmr.tcnt == tmr.tcorb;
        let clear = tmr.tcr & (TMR_TCR_CCLR0 | TMR_TCR_CCLR1);
        if (clear == TMR_TCR_CCLR0 && matcha) || (clear == TMR_TCR_CCLR1 && matchb) {
            tmr.tcnt = 0;
        } else {
            tmr.tcnt = tmr.tcnt.wrapping_add(1);
            if tmr.tcnt == 0 {
                tmr.tcsr |= TMR_TCSR_OVF;
            }
        }
        if matcha {
            tmr.tcsr |= TMR_TCSR_CMFA;
        }
        if matchb {
            tmr.tcsr |= TMR_TCSR_CMFB;
        }
        if tmr.tcr & TMR_TCR_OVIE != 0 && tmr.tcsr & TMR_TCSR_OVF != 0 {
            set_request(irq, INT_TIMER_OVI, true);
        }
        if tmr.tcr & TMR_TCR_CMIEA != 0 && tmr.tcsr & TMR_TCSR_CMFA != 0 {
            set_request(irq, INT_TIMER_CMIA, true);
        }
        if tmr.tcr & TMR_TCR_CMIEB != 0 && tmr.tcsr & TMR_TCSR_CMFB != 0 {
            set_request(irq, INT_TIMER_CMIB, true);
        }
    }

    /// Count up to `cycles` of the processor, raising the interrupts.
    #[inline]
    pub fn clock(&mut self, cycles: u64, irq: &mut u32) {
        let target = cycles / 2;
        self.cycles = target;
        for i in 0..3 {
            while self.frt[i].deadline < target {
                self.clock_frt(i, irq);
                self.frt[i].deadline += self.frt[i].stride as u64;
            }
        }
        while self.tmr.deadline < target {
            self.clock_tmr(irq);
            self.tmr.deadline += self.tmr.stride as u64;
        }
    }
}
