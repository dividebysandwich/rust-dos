//! The PCM chip: 24 or more voices playing the wave ROMs through filters
//! and envelopes, the reverb and chorus, and the mix. A port of
//! Nuked-SC55's `pcm.cpp`, which works out the chip from its die; the
//! arithmetic is kept exactly, 20-bit wrapping included.

use super::machine::{INT_IRQ0, Machine, set_request};
use super::rom::{Loaded, Location};

#[derive(Default)]
struct Config {
    orval: u32,
    noise_mask: u8,
    write_mask: u8,
    oversampling: bool,
    reg_slots: u8,
}

impl Config {
    /// What configuration register 3Ch says. The or-value and noise mask
    /// accumulate, as in Nuked-SC55.
    fn apply(&mut self, config_byte: u8) {
        if config_byte & 0x30 != 0 {
            match (config_byte >> 2) & 3 {
                1 => self.noise_mask = 3,
                2 => self.noise_mask = 7,
                3 => self.noise_mask = 15,
                _ => {}
            }
            match config_byte & 3 {
                1 => self.orval |= 1 << 8,
                2 => self.orval |= 1 << 10,
                _ => {}
            }
            self.write_mask = 15;
        } else {
            match (config_byte >> 2) & 3 {
                2 => self.noise_mask = 1,
                3 => self.noise_mask = 3,
                _ => {}
            }
            match config_byte & 3 {
                1 => self.orval |= 1 << 6,
                2 => self.orval |= 1 << 8,
                _ => {}
            }
            self.write_mask = 3;
        }
        if config_byte & 0x80 == 0 {
            self.write_mask = 0;
        }
        if config_byte & 0x30 == 0x30 {
            self.orval |= 1 << 12;
        }
        if config_byte & 0x40 != 0 {
            self.oversampling = true;
        }
    }
}

pub(super) struct Pcm {
    ram1: [[u32; 8]; 32],
    ram2: [[u16; 16]; 32],
    pub cycles: u64,
    voice_mask: u32,
    voice_mask_pending: u32,
    write_latch: u32,
    read_latch: u32,
    wave_read_address: u32,
    tv_counter: u16,
    wave_byte_latch: u8,
    select_channel: u8,
    config_reg_3d: u8,
    irq_channel: u8,
    irq_assert: bool,
    voice_mask_updating: bool,
    nfs: bool,
    accum_l: i32,
    accum_r: i32,
    rcsum: [i32; 2],
    config: Config,
    eram: Box<[u16]>,
    waverom1: Box<[u8]>,
    waverom2: Box<[u8]>,
    waverom3: Box<[u8]>,
    pub enable_oversampling: bool,
    is_mk1: bool,
}

/// Sign-extend 20 bits.
#[inline(always)]
fn sx20(v: i32) -> i32 {
    (v << 12) >> 12
}

#[inline(always)]
fn addclip20(add1: i32, add2: i32, cin: i32) -> i32 {
    sx20(add1).wrapping_add(sx20(add2)).wrapping_add(cin)
}

#[inline(always)]
fn multi(val1: i32, val2: i8) -> i32 {
    sx20(val1).wrapping_mul(val2 as i32)
}

static INTERP_LUT: [[i32; 128]; 3] = [
    [
        3385, 3401, 3417, 3432, 3448, 3463, 3478, 3492, 3506, 3521, 3535, 3548, 3562, 3575, 3588, 3601, 3614, 3626,
        3638, 3650, 3662, 3673, 3685, 3696, 3707, 3718, 3728, 3739, 3749, 3759, 3768, 3778, 3787, 3796, 3805, 3814,
        3823, 3831, 3839, 3847, 3855, 3863, 3870, 3878, 3885, 3892, 3899, 3905, 3912, 3918, 3924, 3930, 3936, 3942,
        3948, 3953, 3958, 3963, 3968, 3973, 3978, 3983, 3987, 3991, 3995, 4000, 4004, 4007, 4011, 4015, 4018, 4022,
        4025, 4028, 4031, 4034, 4037, 4040, 4042, 4045, 4047, 4050, 4052, 4054, 4057, 4059, 4061, 4063, 4064, 4066,
        4068, 4070, 4071, 4073, 4074, 4076, 4077, 4078, 4079, 4081, 4082, 4083, 4084, 4085, 4086, 4086, 4087, 4088,
        4089, 4089, 4090, 4091, 4091, 4092, 4092, 4093, 4093, 4094, 4094, 4094, 4094, 4095, 4095, 4095, 4095, 4095,
        4095, 4095,
    ],
    [
        710, 726, 742, 758, 775, 792, 809, 826, 844, 861, 879, 897, 915, 933, 952, 971, 990, 1009, 1028, 1047, 1067,
        1087, 1106, 1126, 1147, 1167, 1188, 1208, 1229, 1250, 1271, 1292, 1314, 1335, 1357, 1379, 1400, 1423, 1445,
        1467, 1489, 1512, 1534, 1557, 1580, 1602, 1625, 1648, 1671, 1695, 1718, 1741, 1764, 1788, 1811, 1835, 1858,
        1882, 1906, 1929, 1953, 1977, 2000, 2024, 2048, 2071, 2095, 2119, 2143, 2166, 2190, 2214, 2237, 2261, 2284,
        2308, 2331, 2355, 2378, 2401, 2425, 2448, 2471, 2494, 2517, 2539, 2562, 2585, 2607, 2630, 2652, 2674, 2696,
        2718, 2740, 2762, 2783, 2805, 2826, 2847, 2868, 2889, 2910, 2931, 2951, 2971, 2991, 3011, 3031, 3051, 3070,
        3089, 3108, 3127, 3146, 3164, 3182, 3200, 3218, 3236, 3253, 3271, 3288, 3304, 3321, 3338, 3354, 3370,
    ],
    [
        0, 0, 0, 1, 1, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
        20, 22, 23, 24, 26, 27, 29, 30, 32, 34, 36, 38, 40, 42, 44, 46, 49, 51, 53, 56, 59, 62, 65, 68, 71, 74, 77,
        81, 84, 88, 92, 96, 100, 104, 109, 113, 118, 122, 127, 132, 137, 143, 148, 154, 160, 165, 171, 178, 184, 191,
        197, 204, 211, 219, 226, 234, 241, 249, 257, 266, 274, 283, 292, 301, 310, 319, 329, 339, 349, 359, 369, 380,
        391, 402, 413, 424, 436, 448, 460, 472, 484, 497, 510, 523, 536, 549, 563, 577, 591, 605, 619, 634, 648, 663,
        679, 694,
    ],
];

/// An envelope: the level `levelcur` moves toward the target in
/// `adjust`; `e` is which (0 and 1 volume, 2 the filter). Returns the
/// volume multiplier for 0 and 1.
#[inline(always)]
fn calc_tv(nfs: bool, tv_counter: u16, e: i32, adjust: i32, levelcur: &mut u16, active: bool) -> i32 {
    *levelcur &= 0x7fff;
    let level = *levelcur as i32;
    let speed = adjust & 0xff;
    let target = (adjust >> 8) & 0xff;

    let w1 = speed & 0xf0 == 0;
    let w2 = w1 || speed & 0x10 != 0;
    let w3 = nfs && (speed & 0x80 == 0 || (speed & 0x40 == 0 && (!w2 || speed & 0x20 == 0)));

    let mut kind = w2 as i32 | ((w3 as i32) << 3);
    if speed & 0x20 != 0 {
        kind |= 2;
    }
    if speed & 0x80 == 0 || speed & 0x40 == 0 {
        kind |= 4;
    }

    let tv = tv_counter as i32;
    let mut write = !active;
    // The counter's bits from `top` down, reversed.
    let bits = |top: i32| ((tv >> top) & 1) | ((tv >> (top - 1)) & 1) << 1 | ((tv >> (top - 2)) & 1) << 2 | ((tv >> (top - 3)) & 1) << 3;
    let addlow = if kind & 4 != 0 {
        write = true;
        bits(3)
    } else {
        match kind & 3 {
            0 => {
                write |= tv & 3 == 0;
                bits(5)
            }
            1 => {
                write |= tv & 15 == 0;
                bits(7)
            }
            2 => {
                write |= tv & 63 == 0;
                bits(9)
            }
            _ => {
                write |= tv & 127 == 0;
                bits(11)
            }
        }
    };

    let mut volmul = 0;
    if kind & 8 == 0 {
        let shift = (10 - (speed & 15)) & 15;
        let mut sum1 = target << 11;
        if e != 2 || active {
            sum1 -= level << 4;
        }
        let shifted = (sum1 >> shift) - sum1;
        let sum2 = (target << 11) + addlow + shifted;
        if write && nfs {
            *levelcur = ((sum2 >> 4) & 0x7fff) as u16;
        }
        if e == 0 || e == 1 {
            volmul = (sum2 >> 4) & 0x7ffe;
        }
    } else {
        let mut shift = (speed >> 4) & 14;
        shift |= w2 as i32;
        shift = (10 - shift) & 15;

        let mut sum1 = target << 11;
        if e != 2 || active {
            sum1 -= level << 4;
        }
        let neg = sum1 & 0x80000 != 0;
        let mut preshift = (speed & 15) << 9;
        if !w1 {
            preshift |= 0x2000;
        }
        if neg {
            preshift ^= !0x3f;
        }
        let shifted = preshift >> shift;
        let mut sum2 = shifted;
        if e != 2 || active {
            sum2 += (level << 4) | addlow;
        }
        let sum2_l = sum2 >> 4;
        let sum3 = (target << 11) - (sum2_l << 4);
        let neg2 = sum3 & 0x80000 != 0;
        let xnor = !(neg2 ^ neg);
        if write && nfs {
            *levelcur = if xnor { (sum2_l & 0x7fff) as u16 } else { (target << 7) as u16 };
        }
        if e == 0 {
            volmul = sum2_l & 0x7ffe;
        } else if e == 1 {
            volmul = if xnor { sum2_l & 0x7ffe } else { target << 7 };
        }
    }
    volmul
}

impl Pcm {
    pub fn new(roms: &Loaded, oversampling: bool) -> Box<Pcm> {
        let copy = |location: Location, size: usize| -> Box<[u8]> {
            let mut buf = vec![0u8; size];
            let src = roms.get(location);
            buf[..src.len().min(size)].copy_from_slice(&src[..src.len().min(size)]);
            buf.into()
        };
        Box::new(Pcm {
            ram1: [[0; 8]; 32],
            ram2: [[0; 16]; 32],
            cycles: 0,
            voice_mask: 0,
            voice_mask_pending: 0,
            write_latch: 0,
            read_latch: 0,
            wave_read_address: 0,
            tv_counter: 0,
            wave_byte_latch: 0,
            select_channel: 0,
            config_reg_3d: 0,
            irq_channel: 0,
            irq_assert: false,
            voice_mask_updating: false,
            nfs: false,
            accum_l: 0,
            accum_r: 0,
            rcsum: [0; 2],
            config: Config { reg_slots: 1, ..Config::default() },
            eram: vec![0; 0x4000].into(),
            waverom1: copy(Location::Wave1, 0x200000),
            waverom2: copy(Location::Wave2, 0x200000),
            waverom3: copy(Location::Wave3, 0x100000),
            enable_oversampling: oversampling,
            is_mk1: roms.romset.family.is_mk1(),
        })
    }

    pub fn debug_state(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.ram1.iter().flatten().map(|&x| x as i64).collect();
        v.extend(self.ram2.iter().flatten().map(|&x| x as i64));
        v.extend([self.accum_l as i64, self.accum_r as i64, self.rcsum[0] as i64, self.rcsum[1] as i64]);
        v.extend([self.tv_counter as i64, self.voice_mask as i64, self.voice_mask_pending as i64]);
        let eram: i64 = self.eram.iter().enumerate().map(|(i, &x)| (i as i64 + 1) * x as i64).sum();
        v.push(eram);
        v
    }

    /// Frames a second it makes.
    pub fn output_frequency(&self) -> u32 {
        let freq = if self.is_mk1 { 64000 } else { 66207 };
        if self.enable_oversampling { freq } else { freq / 2 }
    }

    #[inline(always)]
    fn read_rom(&self, address: u32) -> u8 {
        let bank = if self.config_reg_3d & 0x20 != 0 { (address >> 21) & 7 } else { (address >> 19) & 7 };
        match bank {
            0 => {
                if self.is_mk1 {
                    self.waverom1[(address & 0xfffff) as usize]
                } else {
                    self.waverom1[(address & 0x1fffff) as usize]
                }
            }
            1 => self.waverom2[(address & 0xfffff) as usize],
            2 => self.waverom3[(address & 0xfffff) as usize],
            _ => 0,
        }
    }

    fn write(&mut self, address: u32, data: u8) {
        let address = address & 0x3f;
        let data32 = data as u32;
        if address < 0x4 {
            let shift = match address & 3 {
                0 => {
                    self.voice_mask_pending &= !0xf000000;
                    self.voice_mask_pending |= (data32 & 0xf) << 24;
                    None
                }
                1 => Some(16),
                2 => Some(8),
                _ => Some(0),
            };
            if let Some(shift) = shift {
                self.voice_mask_pending &= !(0xff << shift);
                self.voice_mask_pending |= data32 << shift;
            }
            self.voice_mask_updating = true;
        } else if (0x20..0x24).contains(&address) {
            match address & 3 {
                1 => {
                    self.wave_read_address &= !0xff0000;
                    self.wave_read_address |= data32 << 16;
                }
                2 => {
                    self.wave_read_address &= !0xff00;
                    self.wave_read_address |= data32 << 8;
                }
                3 => {
                    self.wave_read_address &= !0xff;
                    self.wave_read_address |= data32;
                    self.wave_byte_latch = self.read_rom(self.wave_read_address);
                }
                _ => {}
            }
        } else if address == 0x3c {
            self.config.apply(data);
        } else if address == 0x3d {
            self.config_reg_3d = data;
            self.config.reg_slots = (data & 31) + 1;
        } else if address == 0x3e {
            self.select_channel = data & 0x1f;
        } else if (0x4..0x10).contains(&address) || (0x24..0x30).contains(&address) {
            match address & 3 {
                1 => {
                    self.write_latch &= !0xf0000;
                    self.write_latch |= (data32 & 0xf) << 16;
                }
                2 => {
                    self.write_latch &= !0xff00;
                    self.write_latch |= data32 << 8;
                }
                3 => {
                    self.write_latch &= !0xff;
                    self.write_latch |= data32;
                    let mut ix = 0;
                    if address & 32 != 0 {
                        ix |= 1;
                    }
                    if address & 8 == 0 {
                        ix |= 4;
                    }
                    if address & 4 == 0 {
                        ix |= 2;
                    }
                    self.ram1[self.select_channel as usize][ix] = self.write_latch;
                }
                _ => {}
            }
        } else if (0x10..0x20).contains(&address) || (0x30..0x38).contains(&address) {
            if address & 1 == 0 {
                self.write_latch &= !0xff00;
                self.write_latch |= data32 << 8;
            } else {
                self.write_latch &= !0xff;
                self.write_latch |= data32;
                let mut ix = ((address >> 1) & 7) as usize;
                if address & 32 != 0 {
                    ix |= 8;
                }
                self.ram2[self.select_channel as usize][ix] = self.write_latch as u16;
            }
        }
    }

    fn read(&mut self, address: u32, irq: &mut u32) -> u8 {
        let address = address & 0x3f;
        if address < 0x4 {
            if self.voice_mask_updating {
                self.voice_mask = self.voice_mask_pending;
            }
            self.voice_mask_updating = false;
        } else if address == 0x3c || address == 0x3e {
            if address == 0x3e && self.irq_assert {
                self.irq_assert = false;
                set_request(irq, INT_IRQ0, false);
            }
            let mut status = self.irq_channel;
            if self.voice_mask_updating {
                status |= 32;
            }
            return status;
        } else if address == 0x3f {
            return self.wave_byte_latch;
        } else if (0x4..0x10).contains(&address) || (0x24..0x30).contains(&address) {
            if address & 3 == 1 {
                let mut ix = 0;
                if address & 32 != 0 {
                    ix |= 1;
                }
                if address & 8 == 0 {
                    ix |= 4;
                }
                if address & 4 == 0 {
                    ix |= 2;
                }
                self.read_latch = self.ram1[self.select_channel as usize][ix];
            }
        } else if (0x10..0x20).contains(&address) || (0x30..0x38).contains(&address) {
            if address & 1 == 0 {
                let mut ix = ((address >> 1) & 7) as usize;
                if address & 32 != 0 {
                    ix |= 8;
                }
                self.read_latch = self.ram2[self.select_channel as usize][ix] as u32;
            }
        } else if (0x39..=0x3b).contains(&address) {
            return match address & 3 {
                1 => ((self.read_latch >> 16) & 0xf) as u8,
                2 => (self.read_latch >> 8) as u8,
                _ => self.read_latch as u8,
            };
        }
        0
    }

    #[inline(always)]
    fn eram_unpack(&self, addr: i32, kind: i32) -> i32 {
        let data = self.eram[(addr & 0x3fff) as usize] as i32;
        let val = data & 0x3fff;
        let sh = (data >> 14) & 3;
        (val << 18) >> (18 - sh * 2 + kind)
    }

    #[inline(always)]
    fn eram_pack(&mut self, addr: i32, val: u32) {
        let mut top = ((val >> 13) & 0x7f) as i32;
        if top & 0x40 != 0 {
            top ^= 0x7f;
        }
        let sh = if top >= 16 {
            3
        } else if top >= 4 {
            2
        } else if top >= 1 {
            1
        } else {
            0
        };
        let data = ((val >> (sh * 2)) & 0x3fff) as i32 | (sh << 14);
        self.eram[(addr & 0x3fff) as usize] = data as u16;
    }

    /// Run the chip up to `cycles` of the main processor.
    fn update(&mut self, cycles: u64, irq: &mut u32, samples: &mut Vec<[i32; 2]>) {
        while self.cycles < cycles {
            self.cycle(irq, samples);
            let new_cycles = (self.config.reg_slots as u64 + 1) * 25;
            self.cycles += new_cycles;
        }
    }

    /// One pass over the voices, making a frame or two.
    fn cycle(&mut self, irq: &mut u32, samples: &mut Vec<[i32; 2]>) {
        let voice_active = self.voice_mask & self.voice_mask_pending;
        let tv_counter_add = |p: &Pcm, i: usize, j: usize| p.ram2[i][j] as i32 + p.tv_counter as i32;
        {
            // The final mix.
            let mut shifter = self.ram2[30][10] as i32;
            let mut xr = (shifter ^ (shifter >> 1) ^ (shifter >> 7) ^ (shifter >> 12)) & 1;
            shifter = (shifter >> 1) | (xr << 15);
            self.ram2[30][10] = shifter as u16;

            let write_mask = self.config.write_mask as i32;
            let noise = |p: &Pcm, shifter: i32| (p.config.orval as i32) | (shifter & p.config.noise_mask as i32);

            self.accum_l = addclip20(self.accum_l, self.ram1[30][0] as i32, 0);
            self.accum_r = addclip20(self.accum_r, self.ram1[30][1] as i32, 0);
            self.ram1[30][2] = addclip20(self.accum_l, noise(self, shifter), 0) as u32;
            self.ram1[30][4] = addclip20(self.accum_r, noise(self, shifter), 0) as u32;
            self.ram1[30][0] = (self.accum_l & write_mask) as u32;
            self.ram1[30][1] = (self.accum_r & write_mask) as u32;

            let mask = !(self.config.write_mask as u32);
            samples.push([((self.ram1[30][2] & mask) << 12) as i32, ((self.ram1[30][4] & mask) << 12) as i32]);

            xr = (shifter ^ (shifter >> 1) ^ (shifter >> 7) ^ (shifter >> 12)) & 1;
            shifter = (shifter >> 1) | (xr << 15);

            self.accum_l = addclip20(self.accum_l, self.ram1[30][0] as i32, 0);
            self.accum_r = addclip20(self.accum_r, self.ram1[30][1] as i32, 0);
            self.ram1[30][3] = addclip20(self.accum_l, noise(self, shifter), 0) as u32;
            self.ram1[30][5] = addclip20(self.accum_r, noise(self, shifter), 0) as u32;

            if self.enable_oversampling && self.config.oversampling {
                self.ram2[30][10] = shifter as u16;
                self.ram1[30][0] = (self.accum_l & write_mask) as u32;
                self.ram1[30][1] = (self.accum_r & write_mask) as u32;
                samples.push([((self.ram1[30][3] & mask) << 12) as i32, ((self.ram1[30][5] & mask) << 12) as i32]);
            }
        }

        // The counter the envelopes and the delay lines go by.
        if !self.nfs {
            self.tv_counter = self.ram2[31][8];
        }
        self.tv_counter = self.tv_counter.wrapping_sub(1) & 0x3fff;

        {
            let r8 = self.ram2[31][8] as i32;
            if r8 & 0x8000 != 0 {
                self.ram2[31][9] = (r8 & 0x7fff) as u16;
            } else {
                self.ram2[31][10] = (r8 & 0x7fff) as u16;
            }
            let neg = 0x4000 - r8;
            if neg & 0x8000 != 0 {
                self.ram2[31][10] = (neg & 0x7fff) as u16;
            } else {
                self.ram2[31][9] = (neg & 0x7fff) as u16;
            }
        }

        {
            let v1 = self.ram2[31][1] as i32;
            let m1 = multi(self.ram1[29][1] as i32, (v1 >> 8) as i8) >> 5;
            let m2 = multi(self.rcsum[1], v1 as i8) >> 5;
            self.ram1[29][1] = addclip20(m1 >> 1, m2 >> 1, (m1 | m2) & 1) as u32;
        }

        {
            let active = self.ram2[31][7] & 0x20 != 0;
            let (nfs, tv) = (self.nfs, self.tv_counter);
            let adjust = self.ram2[30][0] as i32;
            calc_tv(nfs, tv, 1, adjust, &mut self.ram2[30][9], active);
        }

        {
            let v1 = self.ram2[30][1] as i32;
            let m1 = multi(self.ram1[29][0] as i32, (v1 >> 8) as i8) >> 5;
            let m2 = multi(self.rcsum[0], v1 as i8) >> 5;
            self.ram1[29][0] = addclip20(m1 >> 1, m2 >> 1, (m1 | m2) & 1) as u32;
        }

        let mut rcadd = [0i32; 6];
        let mut rcadd2 = [0i32; 6];

        let unpack = |p: &Pcm, i: usize, j: usize, kind: i32| p.eram_unpack(tv_counter_add(p, i, j), kind);

        {
            // 1
            let v1 = self.ram2[30][4] as i32;
            let m1 = multi(self.ram1[29][0] as i32, (v1 >> 8) as i8) >> 6;
            let s1 = unpack(self, 28, 1, 1);
            let s2 = unpack(self, 28, 1, 0);
            let v2 = if v1 & 0x30 != 0 { s1 } else { 0 };
            let v3 = addclip20(m1, v2 ^ 0xfffff, 1);
            self.ram1[29][4] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[29][5] = addclip20(m2 >> 1, s2, m2 & 1) as u32;
        }
        {
            // 2
            let v1 = self.ram2[30][4] as i32;
            let s1 = unpack(self, 28, 2, 1);
            let s2 = unpack(self, 28, 2, 0);
            let v2 = if v1 & 0x30 != 0 { s1 } else { 0 };
            let v3 = addclip20(self.ram1[29][5] as i32, v2 ^ 0xfffff, 1);
            self.ram1[29][5] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[28][0] = addclip20(m2 >> 1, s2, m2 & 1) as u32;
        }
        {
            // 3
            let v1 = self.ram2[30][4] as i32;
            let s1 = unpack(self, 28, 3, 1);
            let s2 = unpack(self, 28, 3, 0);
            let v2 = if v1 & 0x30 != 0 { s1 } else { 0 };
            let v3 = addclip20(self.ram1[28][0] as i32, v2 ^ 0xfffff, 1);
            self.ram1[28][0] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[28][1] = addclip20(m2 >> 1, s2, m2 & 1) as u32;
            self.ram1[28][2] = unpack(self, 28, 5, 0) as u32;
        }
        {
            // 4
            let v1 = self.ram2[30][5] as i32;
            let s1 = unpack(self, 28, 4, 1);
            let s2 = unpack(self, 28, 4, 0);
            let v2 = if v1 & 0x30 != 0 { s1 } else { 0 };
            let v3 = addclip20(self.ram1[28][1] as i32, v2 ^ 0xfffff, 1);
            self.ram1[28][1] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[28][3] = addclip20(m2 >> 1, s2, m2 & 1) as u32;
            self.ram1[28][4] = unpack(self, 29, 1, 0) as u32;
        }
        {
            // 5
            let v1 = self.ram2[30][7] as i32;
            let m1 = multi(self.ram1[29][2] as i32, (v1 >> 8) as i8) >> 5;
            let s1 = unpack(self, 29, 0, 0);
            let m2 = multi(s1, v1 as i8) >> 5;
            self.ram1[29][2] = addclip20(m1 >> 1, m2 >> 1, (m1 | m2) & 1) as u32;
            self.eram_pack(tv_counter_add(self, 28, 0), self.ram1[29][4]);
        }
        {
            // 6
            let v1 = self.ram2[30][8] as i32;
            let m1 = multi(self.ram1[29][3] as i32, (v1 >> 8) as i8) >> 5;
            let s1 = unpack(self, 29, 8, 0);
            let m2 = multi(s1, v1 as i8) >> 5;
            self.ram1[29][3] = addclip20(m1 >> 1, m2 >> 1, (m1 | m2) & 1) as u32;
            self.eram_pack(tv_counter_add(self, 28, 1), self.ram1[29][5]);
            self.eram_pack(tv_counter_add(self, 28, 2), self.ram1[28][0]);
        }
        {
            // 7
            let v1 = self.ram2[30][9] as i32;
            let v2 = self.ram1[28][3] as i32;
            let m1 = multi(self.ram1[29][2] as i32, (v1 >> 8) as i8) >> 5;
            let m2 = multi(self.ram1[29][3] as i32, (v1 >> 8) as i8) >> 5;
            self.ram1[28][3] = addclip20(v2, m1 >> 1, m1 & 1) as u32;
            self.ram1[28][5] = addclip20(v2, m2 >> 1, m2 & 1) as u32;
            self.eram_pack(tv_counter_add(self, 28, 3), self.ram1[28][1]);
        }
        {
            // 8
            let v1 = self.ram2[30][6] as i32;
            let m1 = multi(self.ram1[28][2] as i32, (v1 >> 8) as i8) >> 5;
            let v2 = addclip20(self.ram1[28][3] as i32, m1 >> 1, m1 & 1);
            self.ram1[28][3] = v2 as u32;
            let m2 = multi(v2, v1 as i8) >> 5;
            self.ram1[28][2] = addclip20(self.ram1[28][2] as i32, m2 >> 1, m2 & 1) as u32;
            self.ram1[28][1] = unpack(self, 28, 9, 0) as u32;
        }
        {
            // 9
            let v1 = self.ram2[30][6] as i32;
            let m1 = multi(self.ram1[28][4] as i32, (v1 >> 8) as i8) >> 5;
            let v2 = addclip20(self.ram1[28][5] as i32, m1 >> 1, m1 & 1);
            self.ram1[28][5] = v2 as u32;
            let m2 = multi(v2, v1 as i8) >> 5;
            self.ram1[28][4] = addclip20(self.ram1[28][4] as i32, m2 >> 1, m2 & 1) as u32;
            self.ram1[29][4] = unpack(self, 29, 5, 0) as u32;
        }
        {
            // 10
            let v1 = self.ram2[30][6] as i32;
            let v2 = self.ram1[28][1] as i32;
            let m1 = multi(v2, (v1 >> 8) as i8) >> 5;
            let s1 = unpack(self, 28, 8, 0);
            let v3 = addclip20(m1 >> 1, s1, m1 & 1);
            self.ram1[28][1] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[29][5] = addclip20(m2 >> 1, v2, m2 & 1) as u32;
            self.eram_pack(tv_counter_add(self, 28, 4), self.ram1[28][3]);
        }
        {
            // 11
            let v1 = self.ram2[30][6] as i32;
            let v2 = self.ram1[29][4] as i32;
            let m1 = multi(v2, (v1 >> 8) as i8) >> 5;
            let s1 = unpack(self, 29, 4, 0);
            let v3 = addclip20(m1 >> 1, s1, m1 & 1);
            self.ram1[29][4] = v3 as u32;
            let m2 = multi(v3, v1 as i8) >> 5;
            self.ram1[28][0] = addclip20(m2 >> 1, v2, m2 & 1) as u32;
            self.eram_pack(tv_counter_add(self, 28, 5), self.ram1[28][2]);
            self.eram_pack(tv_counter_add(self, 29, 0), self.ram1[28][5]);
        }
        // 12
        self.ram1[28][5] = unpack(self, 28, 6, 0) as u32;
        {
            // 13
            let s1 = unpack(self, 28, 10, 0);
            self.ram1[28][5] = addclip20(self.ram1[28][5] as i32, s1, 0) as u32;
            self.ram1[28][2] = unpack(self, 29, 2, 0) as u32;
        }
        {
            // 14
            let s1 = unpack(self, 29, 6, 0);
            let t1 = addclip20(s1, self.ram1[28][2] as i32, 0);
            self.ram1[28][5] = addclip20(t1, self.ram1[28][5] as i32, 0) as u32;
            self.ram1[28][2] = unpack(self, 28, 7, 0) as u32;
        }
        {
            // 15
            let s1 = unpack(self, 28, 11, 0);
            self.ram1[28][2] = addclip20(self.ram1[28][2] as i32, s1, 0) as u32;
            self.ram1[28][3] = unpack(self, 29, 3, 0) as u32;
        }
        {
            // 16
            let s1 = unpack(self, 29, 7, 0);
            let t1 = addclip20(s1, self.ram1[28][2] as i32, 0);
            self.ram1[28][2] = addclip20(t1, self.ram1[28][3] as i32, 0) as u32;
            self.eram_pack(tv_counter_add(self, 29, 1), self.ram1[28][4]);
            self.eram_pack(tv_counter_add(self, 28, 8), self.ram1[28][1]);
        }
        {
            // 17
            let v1 = self.ram2[30][2] as i32;
            let v2 = self.ram1[28][5] as i32;
            rcadd[0] = multi(v2, (v1 >> 8) as i8) >> 5;
            rcadd2[0] = multi(v2, v1 as i8) >> 5;
            let t1 = self.eram_unpack(tv_counter_add(self, 29, 10) + 1, 0);
            self.eram_pack(tv_counter_add(self, 28, 9), self.ram1[29][5]);
            self.ram1[29][5] = t1 as u32;
        }
        {
            // 18
            let v1 = self.ram2[30][3] as i32;
            let v2 = self.ram1[28][2] as i32;
            rcadd[1] = multi(v2, (v1 >> 8) as i8) >> 5;
            rcadd2[1] = multi(v2, v1 as i8) >> 5;
            self.ram1[28][1] = self.eram_unpack(tv_counter_add(self, 29, 11) + 1, 0) as u32;
        }
        {
            // 19
            let v1 = self.ram2[31][9] as i32;
            let s1 = unpack(self, 29, 10, 0);
            self.eram_pack(tv_counter_add(self, 29, 4), self.ram1[29][4]);
            let m1 = multi(s1, (v1 >> 8) as i8) >> 5;
            let m2 = multi(self.ram1[29][5] as i32, (v1 >> 8) as i8) >> 5;
            let t2 = addclip20(s1, (m1 >> 1) ^ 0xfffff, 1);
            self.ram1[29][5] = addclip20(t2, m2 >> 1, m2 & 1) as u32;
        }
        {
            // 20
            let v1 = self.ram2[31][10] as i32;
            let s1 = unpack(self, 29, 11, 0);
            self.eram_pack(tv_counter_add(self, 29, 5), self.ram1[28][0]);
            let m1 = multi(s1, (v1 >> 8) as i8) >> 5;
            let m2 = multi(self.ram1[28][1] as i32, (v1 >> 8) as i8) >> 5;
            let t2 = addclip20(s1, (m1 >> 1) ^ 0xfffff, 1);
            self.ram1[28][1] = addclip20(t2, m2 >> 1, m2 & 1) as u32;
            self.eram_pack(tv_counter_add(self, 29, 9), self.ram1[29][1]);
        }
        for (k, (i, j, src)) in [(31usize, 2usize, (29usize, 5usize)), (31, 3, (29, 5)), (31, 4, (28, 1)), (31, 5, (28, 1))]
            .into_iter()
            .enumerate()
        {
            // 21, 22, 23, 31
            let v1 = self.ram2[i][j] as i32;
            let v2 = self.ram1[src.0][src.1] as i32;
            rcadd[k + 2] = multi(v2, (v1 >> 8) as i8) >> 5;
            rcadd2[k + 2] = multi(v2, v1 as i8) >> 5;
        }
        self.global_address_generator();

        self.ram1[31][1] = 0;
        self.ram1[31][3] = 0;
        self.rcsum = [0, 0];

        let slots = self.config.reg_slots as usize;
        for slot in 0..slots {
            self.voice(slot, slots, voice_active, &rcadd, &rcadd2, irq);
        }

        if self.nfs {
            self.ram2[31][7] |= 0x20;
        }
        self.nfs = true;
    }

    /// Slot 31's address generator, which moves the chorus's delay.
    fn global_address_generator(&mut self) {
        // Its key is always on.
        let okey = self.ram2[31][7] & 0x20 != 0;
        let active = okey;
        let kon = !okey;
        let mut b15 = self.ram2[31][8] & 0x8000 != 0;
        let b6 = self.ram2[31][7] & 0x40 != 0;
        let b7 = self.ram2[31][7] & 0x80 != 0;

        let address = self.ram1[31][4] as i32;
        let address_end = self.ram1[31][0] as i32;
        let address_loop = self.ram1[31][2] as i32;

        let mut sub_phase = (self.ram2[31][8] & 0x3fff) as i32;
        sub_phase += self.ram2[(self.ram2[31][7] & 31) as usize][0] as i32;
        let sub_phase_of = (sub_phase >> 14) & 7;
        if self.nfs {
            self.ram2[31][8] &= !0x3fff;
            self.ram2[31][8] |= (sub_phase & 0x3fff) as u16;
        }

        let mut address_cnt = address;
        let cmp1 = if b15 { address_loop } else { address_end };
        let address_cmp = (cmp1 & 0xfffff) == (address_cnt & 0xfffff);
        let mut next_b15 = b15;
        let mut next_address = address_cnt;

        let cmp1 = if !b6 && address_cmp { address_loop } else { address_cnt };
        let mut address_cnt2 = if kon || (!b6 && address_cmp) { cmp1 } else { address_cnt };
        let address_add = (!address_cmp && b6 && !b15) || (!address_cmp && !b6);
        let address_sub = !address_cmp && b6 && b15;
        let delta = address_add as i32 - address_sub as i32;
        if b7 {
            address_cnt2 -= delta;
        } else {
            address_cnt2 += delta;
        }
        address_cnt = address_cnt2 & 0xfffff;
        b15 = b6 && (b15 ^ address_cmp);

        if sub_phase_of >= 1 {
            next_address = address_cnt;
            next_b15 = b15;
        }
        if active && self.nfs {
            self.ram1[31][4] = next_address as u32;
        }
        if self.nfs {
            self.ram2[31][8] &= !0x8000;
            self.ram2[31][8] |= (next_b15 as u16) << 15;
        }

        let t2 = (self.ram1[31][4] as i32).wrapping_sub(address_loop);
        let t3 = address_end.wrapping_sub(t2);
        let t4 = self.ram1[31][4] as i32;
        self.ram2[29][10] = t3 as u16;
        self.ram2[29][11] = t4 as u16;
    }

    #[inline(always)]
    fn voice(&mut self, slot: usize, slots: usize, voice_active: u32, rcadd: &[i32; 6], rcadd2: &[i32; 6], irq: &mut u32) {
        let okey = self.ram2[slot][7] & 0x20 != 0;
        let key = (voice_active >> slot) & 1 != 0;
        let active = okey && key;
        let kon = key && !okey;

        // The address generator.
        let r7 = self.ram2[slot][7];
        let r8 = self.ram2[slot][8];
        let mut b15 = r8 & 0x8000 != 0;
        let b6 = r7 & 0x40 != 0;
        let b7 = r7 & 0x80 != 0;
        let hiaddr = ((r7 >> 8) & 15) as u32;
        let old_nibble = ((r7 >> 12) & 15) as i32;

        let address = self.ram1[slot][4] as i32;
        let address_end = self.ram1[slot][0] as i32;
        let address_loop = self.ram1[slot][2] as i32;

        let cmp1 = if b15 { address_loop } else { address_end };
        let nibble_cmp1 = (cmp1 & 0xffff0) == (address & 0xffff0);

        let mut irq_flag = if kon {
            (cmp1.wrapping_add(address_loop) & 0x100000) != 0
        } else {
            (address.wrapping_add(address_loop.wrapping_neg() & 0xfffff) & 0x100000) != 0
        };
        irq_flag ^= b7;

        let nibble_address = if !b6 && nibble_cmp1 { address_loop } else { address };
        let address_b4 = nibble_address & 0x10 != 0;
        let mut wave_address = nibble_address >> 5;
        let xor2 = address_b4 ^ b7;
        let check1 = xor2 && active;
        let xor1 = b15 ^ !nibble_cmp1;
        let nibble_add = if b6 { check1 && xor1 } else { !nibble_cmp1 && check1 };
        let nibble_subtract = b6 && !xor1 && active && !xor2;
        let delta = nibble_add as i32 - nibble_subtract as i32;
        if b7 {
            wave_address -= delta;
        } else {
            wave_address += delta;
        }
        wave_address &= 0xfffff;

        let mut newnibble = self.read_rom((hiaddr << 20) | wave_address as u32) as i32;
        let newnibble_sel = address_b4 ^ ((b6 || !nibble_cmp1) && okey);
        if newnibble_sel {
            newnibble = (newnibble >> 4) & 15;
        } else {
            newnibble &= 15;
        }

        let mut sub_phase = (r8 & 0x3fff) as i32;
        let interp_ratio = ((sub_phase >> 7) & 127) as usize;
        sub_phase += self.ram2[(r7 & 31) as usize][0] as i32;
        let sub_phase_of = (sub_phase >> 14) & 7;
        if self.nfs {
            self.ram2[slot][8] &= !0x3fff;
            self.ram2[slot][8] |= (sub_phase & 0x3fff) as u16;
        }

        // The four steps through the wave.
        let read = |p: &Pcm, a: i32| p.read_rom((hiaddr << 20) | a as u32) as i8 as i32;
        let advance = |address_cnt: i32, b15: bool| -> (i32, bool) {
            let cmp1 = if b15 { address_loop } else { address_end };
            let address_cmp = (cmp1 & 0xfffff) == (address_cnt & 0xfffff);
            let cmp1 = if !b6 && address_cmp { address_loop } else { address_cnt };
            let mut address_cnt2 = if kon || (!b6 && address_cmp) { cmp1 } else { address_cnt };
            let address_add = (!address_cmp && b6 && !b15) || (!address_cmp && !b6);
            let address_sub = !address_cmp && b6 && b15;
            let delta = address_add as i32 - address_sub as i32;
            if b7 {
                address_cnt2 -= delta;
            } else {
                address_cnt2 += delta;
            }
            (address_cnt2 & 0xfffff, b6 && (b15 ^ address_cmp))
        };
        let nibble_cmp = |a: i32| (address & 0xffff0) == (a & 0xffff0);

        let mut address_cnt = address;
        let samp0 = read(self, address_cnt);
        let nibble_cmp2 = nibble_cmp(address_cnt);
        let mut next_address = address_cnt;
        let mut usenew = !nibble_cmp2;
        let mut next_b15 = b15;

        (address_cnt, b15) = advance(address_cnt, b15);
        let samp1 = read(self, address_cnt);
        let nibble_cmp3 = nibble_cmp(address_cnt);
        if sub_phase_of >= 1 {
            next_address = address_cnt;
            usenew = !nibble_cmp3;
            next_b15 = b15;
        }

        (address_cnt, b15) = advance(address_cnt, b15);
        let samp2 = read(self, address_cnt);
        let nibble_cmp4 = nibble_cmp(address_cnt);
        if sub_phase_of >= 2 {
            next_address = address_cnt;
            usenew = !nibble_cmp4;
            next_b15 = b15;
        }

        (address_cnt, b15) = advance(address_cnt, b15);
        let samp3 = read(self, address_cnt);
        let nibble_cmp5 = nibble_cmp(address_cnt);
        if sub_phase_of >= 3 {
            next_address = address_cnt;
            usenew = !nibble_cmp5;
            next_b15 = b15;
        }

        (address_cnt, _) = advance(address_cnt, b15);
        let nibble_cmp6 = nibble_cmp(address_cnt);
        if sub_phase_of >= 4 {
            next_address = address_cnt;
            usenew = !nibble_cmp6;
        }

        if active && self.nfs {
            self.ram1[slot][4] = next_address as u32;
        }
        if self.nfs {
            self.ram2[slot][8] &= !0x8000;
            self.ram2[slot][8] |= (next_b15 as u16) << 15;
        }

        // DPCM: the reference sample moves by the steps passed.
        let shift_for = |same: bool| (10 - if same { old_nibble } else { newnibble }) & 15;
        let mut reference = self.ram1[slot][5] as i32;
        for (k, (samp, same)) in
            [(samp0, nibble_cmp2), (samp1, nibble_cmp3), (samp2, nibble_cmp4), (samp3, nibble_cmp5)].into_iter().enumerate()
        {
            let shifted = ((samp << 10) << 1) >> shift_for(same);
            if sub_phase_of > k as i32 {
                reference = addclip20(reference, shifted >> 1, shifted & 1);
            }
        }

        // Interpolation.
        let mut test = self.ram1[slot][5] as i32;
        for (k, (samp, same)) in [(samp0, nibble_cmp2), (samp1, nibble_cmp3), (samp2, nibble_cmp4)].into_iter().enumerate() {
            let step = multi(INTERP_LUT[k][interp_ratio] << 6, samp as i8) >> 8;
            let step = (step << 1) >> shift_for(same);
            test = addclip20(test, step >> 1, step & 1);
        }

        // The filter.
        let reg1 = self.ram1[slot][1] as i32;
        let reg3 = self.ram1[slot][3] as i32;
        let reg2_6 = ((self.ram2[slot][6] >> 8) & 127) as i32;
        let filter = self.ram2[slot][11] as i32;
        let v3;
        if self.is_mk1 {
            let mult1 = multi(reg1, (filter >> 8) as i8);
            let mult2 = multi(reg1, ((filter >> 1) & 127) as i8);
            let mult3 = multi(reg1, reg2_6 as i8);
            let v2 = addclip20(reg3, mult1 >> 6, (mult1 >> 5) & 1);
            let v1 = addclip20(v2, mult2 >> 13, (mult2 >> 12) & 1);
            let subvar = addclip20(v1, mult3 >> 6, (mult3 >> 5) & 1);
            self.ram1[slot][3] = v1 as u32;
            v3 = addclip20(test, subvar ^ 0xfffff, 1);
            let mult4 = multi(v3, (filter >> 8) as i8);
            let mult5 = multi(v3, ((filter >> 1) & 127) as i8);
            let v4 = addclip20(reg1, mult4 >> 6, (mult4 >> 5) & 1);
            let v5 = addclip20(v4, mult5 >> 13, (mult5 >> 12) & 1);
            self.ram1[slot][1] = v5 as u32;
        } else {
            // 32-bit arithmetic, as Nuked-SC55 does to avoid overflow.
            let mult1 = reg1.wrapping_mul((filter >> 8) as i8 as i32);
            let mult2 = reg1.wrapping_mul(((filter >> 1) & 127) as i8 as i32);
            let mult3 = reg1.wrapping_mul(reg2_6 as i8 as i32);
            let v2 = reg3.wrapping_add(mult1 >> 6).wrapping_add((mult1 >> 5) & 1);
            let v1 = v2.wrapping_add(mult2 >> 13).wrapping_add((mult2 >> 12) & 1);
            let subvar = v1.wrapping_add(mult3 >> 6).wrapping_add((mult3 >> 5) & 1);
            self.ram1[slot][3] = v1 as u32;
            let tests = (test << 12) >> 12;
            v3 = tests.wrapping_sub(subvar);
            let mult4 = v3.wrapping_mul((filter >> 8) as i8 as i32);
            let mult5 = v3.wrapping_mul(((filter >> 1) & 127) as i8 as i32);
            let v4 = reg1.wrapping_add(mult4 >> 6).wrapping_add((mult4 >> 5) & 1);
            let v5 = v4.wrapping_add(mult5 >> 13).wrapping_add((mult5 >> 12) & 1);
            self.ram1[slot][1] = v5 as u32;
        }

        self.ram1[slot][5] = reference as u32;

        if active && self.ram2[slot][6] & 1 != 0 && self.ram2[slot][8] & 0x4000 == 0 && !self.irq_assert && irq_flag {
            if self.nfs {
                self.ram2[slot][8] |= 0x4000;
            }
            self.irq_assert = true;
            self.irq_channel = slot as u8;
            set_request(irq, INT_IRQ0, true);
        }

        // Envelopes and volume.
        let (nfs, tv) = (self.nfs, self.tv_counter);
        let ram2 = &mut self.ram2[slot];
        let (a3, a4, a5) = (ram2[3] as i32, ram2[4] as i32, ram2[5] as i32);
        let volmul1 = calc_tv(nfs, tv, 0, a3, &mut ram2[9], active);
        let volmul2 = calc_tv(nfs, tv, 1, a4, &mut ram2[10], active);
        calc_tv(nfs, tv, 2, a5, &mut ram2[11], active);

        let sample = if ram2[6] & 2 == 0 { self.ram1[slot][3] as i32 } else { v3 };

        let multiv1 = multi(sample, (volmul1 >> 8) as i8);
        let multiv2 = multi(sample, ((volmul1 >> 1) & 127) as i8);
        let sample2 = addclip20(multiv1 >> 6, multiv2 >> 13, ((multiv2 >> 12) | (multiv1 >> 5)) & 1);
        let multiv3 = multi(sample2, (volmul2 >> 8) as i8);
        let multiv4 = multi(sample2, ((volmul2 >> 1) & 127) as i8);
        let sample3 = addclip20(multiv3 >> 6, multiv4 >> 13, ((multiv4 >> 12) | (multiv3 >> 5)) & 1);

        let pan = if active { ram2[1] as i32 } else { 0 };
        let rc = if active { ram2[2] as i32 } else { 0 };
        let sampl = multi(sample3, (pan >> 8) as i8);
        let sampr = multi(sample3, pan as i8);
        let rc0 = multi(sample3, (rc >> 8) as i8) >> 5; // reverb
        let rc1 = multi(sample3, rc as i8) >> 5; // chorus

        // The reverb and chorus come back into the mix at certain slots.
        let slot2 = if slot == slots - 1 { 31 } else { slot + 1 };
        let back = match slot2 {
            17 => Some((1, 0, 1)),
            18 => Some((3, 1, 1)),
            21 => Some((1, 2, 0)),
            22 => Some((3, 3, 1)),
            23 => Some((1, 4, 0)),
            31 => Some((3, 5, 1)),
            _ => None,
        };
        if let Some((side, k, _)) = back {
            self.ram1[31][side] = addclip20(self.ram1[31][side] as i32, rcadd[k] >> 1, rcadd[k] & 1) as u32;
        }
        let suml = addclip20(self.ram1[31][1] as i32, sampl >> 6, (sampl >> 5) & 1);
        let sumr = addclip20(self.ram1[31][3] as i32, sampr >> 6, (sampr >> 5) & 1);
        if let Some((_, k, sum)) = back {
            self.rcsum[sum] = addclip20(self.rcsum[sum], rcadd2[k] >> 1, rcadd2[k] & 1);
        }
        self.rcsum[0] = addclip20(self.rcsum[0], rc0 >> 1, rc0 & 1);
        self.rcsum[1] = addclip20(self.rcsum[1], rc1 >> 1, rc1 & 1);

        if slot != slots - 1 {
            self.ram1[31][1] = suml as u32;
            self.ram1[31][3] = sumr as u32;
        } else {
            self.accum_l = suml;
            self.accum_r = sumr;
        }

        let ram2 = &mut self.ram2[slot];
        if key && self.nfs {
            ram2[7] &= !0xf020;
            ram2[7] |= (((if usenew || kon { newnibble } else { old_nibble }) as u16) << 12) | (1 << 5);
        }
        if !active {
            if self.nfs {
                self.ram1[slot][1] = 0;
                self.ram1[slot][3] = 0;
                self.ram1[slot][5] = 0;
            }
            ram2[8] = 0;
            ram2[9] = 0;
            ram2[10] = 0;
        }
    }
}

impl Machine {
    pub(super) fn pcm_read(&mut self, address: u32) -> u8 {
        self.pcm.read(address, &mut self.interrupt_pending)
    }

    pub(super) fn pcm_write(&mut self, address: u32, data: u8) {
        self.pcm.write(address, data);
    }

    pub(super) fn pcm_update(&mut self) {
        self.pcm.update(self.cycles, &mut self.interrupt_pending, &mut self.samples);
    }
}
