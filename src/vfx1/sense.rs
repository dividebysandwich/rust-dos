//! VFX1.COM's "Sense" interface on INT 33h, which it shares with the mouse
//! driver: a game calls AH=60h..70h with AL=7Fh to find it, and the AH it
//! answers to (60h) is the prefix of every later call, with the function in
//! AL. The packets and answers are VFX1.COM 2.14's, as forte-vfx1-vr
//! (src/vfx1dev, docs/FINDINGS.md Q13) has them; Z.A.R. checks most of
//! them, Descent only finds the driver, reads the head and zeroes it.

use super::Vfx1;
use crate::cpu::Cpu;
use crate::savestate::{Reader, Result, State, Writer};

/// The range of prefixes games look for the driver in, and the one it
/// answers to.
const SENSE_MIN: u8 = 0x60;
const SENSE_MAX: u8 = 0x70;
const PREFIX: u8 = 0x60;

/// The functions, in AL.
const RESET_DEVICE: u8 = 0x00;
const GET_DATA_SIZES: u8 = 0x01;
const SET_DATA_SIZE: u8 = 0x02;
const GET_CONFIGURATION: u8 = 0x03;
const SET_CONFIGURATION: u8 = 0x04;
const REPORT: u8 = 0x05;
const CONTINUE: u8 = 0x06;
const RESET_ZERO: u8 = 0x07;
const SET_ZERO: u8 = 0x08;
const TEST_DEVICES: u8 = 0x09;
const GET_DEVICE_DATA: u8 = 0x70;
const SET_VIDEO_MODE: u8 = 0x71;
const DRIVER_STATUS: u8 = 0x7F;
const SET_VIDEO_PAGE: u8 = 0x80;

/// The device classes, in BH: the display, the head tracker, the puck;
/// FFh is all of them.
const CLASS_HMS: u8 = 2;
const CLASS_HTD: u8 = 3;
const CLASS_HPD: u8 = 4;
const CLASS_ALL: u8 = 0xFF;

/// The configuration packets' types.
const PKT_NULL: u8 = 0;
const PKT_SYSI: u8 = 1;
const PKT_VNDI: u8 = 2;
const PKT_HMS: u8 = 4;
const PKT_SVM: u8 = 5;
const PKT_HTD: u8 = 6;
const PKT_HPD: u8 = 7;

/// The most a configuration a game hands back is read of.
const MAX_CONFIG: usize = 256;

/// Errors in AX; the SDK only says they aren't 0.
const OK: u16 = 0;
const FAILED: u16 = 1;

/// What a game has set up in the driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Driver {
    /// The data each axis reports: head yaw, pitch, roll, the puck's yaw,
    /// pitch, roll, and its buttons. A game that never sets the
    /// configuration gets all but the puck's yaw, which it hasn't.
    axes: [bool; 7],
    /// The HMS flags' bit 7, which an SVM packet sets.
    hms_flag7: bool,
    /// 0 mono, 1 multiple-buffer, 2 line-sequential.
    pub(crate) video_mode: u8,
    /// The head where the game zeroed it.
    zero: [i16; 3],
}

impl Default for Driver {
    fn default() -> Self {
        Self {
            axes: [true, true, true, false, true, true, true],
            hms_flag7: false,
            video_mode: 0,
            zero: [0; 3],
        }
    }
}

impl State for Driver {
    fn save(&self, w: &mut Writer) {
        let Self {
            axes,
            hms_flag7,
            video_mode,
            zero,
        } = self;
        axes.map(u8::from).save(w);
        hms_flag7.save(w);
        video_mode.save(w);
        zero.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut axes = [0u8; 7];
        axes.load(r)?;
        self.axes = axes.map(|on| on != 0);
        self.hms_flag7.load(r)?;
        self.video_mode.load(r)?;
        self.zero.load(r)
    }
}

/// The registers a call takes and gives back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Regs {
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
}

/// A call to INT 33h for the VFX1's driver: true when it took it, as
/// one with AH in the Sense range does while the card is there.
pub fn handle(cpu: &mut Cpu) -> bool {
    let ah = (cpu.ax() >> 8) as u8;
    if !(SENSE_MIN..=SENSE_MAX).contains(&ah) || cpu.bus.vfx1.is_none() {
        return false;
    }
    let mut regs = Regs {
        ax: cpu.ax(),
        bx: cpu.bx(),
        cx: cpu.cx(),
        dx: cpu.dx(),
    };
    let now_ms = cpu.bus.clock.now_ns() as f64 / 1e6;
    let buffer = cpu.get_physical_addr(cpu.es(), cpu.dx()) as u32;
    let handed: Vec<u8> = if regs.ax == u16::from(PREFIX) << 8 | u16::from(SET_CONFIGURATION) {
        (0..MAX_CONFIG as u32)
            .map(|i| cpu.bus.guest_read_8(buffer + i))
            .collect()
    } else {
        Vec::new()
    };
    let Some(vfx1) = cpu.bus.vfx1.as_mut() else {
        return false;
    };
    let out = call(vfx1, &mut regs, now_ms, &handed);
    for (i, byte) in out.iter().enumerate() {
        cpu.bus.guest_write_8(buffer + i as u32, *byte);
    }
    cpu.set_ax(regs.ax);
    cpu.set_bx(regs.bx);
    cpu.set_cx(regs.cx);
    cpu.set_dx(regs.dx);
    true
}

/// A Sense call at `now_ms` of the machine's time, with `handed` what
/// ES:DX points to for SET_CONFIGURATION. Returns what goes to ES:DX.
pub(crate) fn call(vfx1: &mut Vfx1, regs: &mut Regs, now_ms: f64, handed: &[u8]) -> Vec<u8> {
    // Another prefix in the range isn't this driver's: the registers
    // unchanged tell a game looking for it to go on looking.
    if (regs.ax >> 8) as u8 != PREFIX {
        return Vec::new();
    }
    let function = regs.ax as u8;
    let class = (regs.bx >> 8) as u8;
    let mut out = Vec::new();
    match function {
        DRIVER_STATUS => {
            // VFX1.COM answers its prefix, BX=0, and CX+1 with bit 8 set, a
            // handshake Z.A.R. checks.
            regs.ax = u16::from(DRIVER_STATUS) << 8 | u16::from(PREFIX);
            regs.bx = 0;
            regs.cx = (regs.cx.wrapping_add(1) | 0x100) & 0x1FF;
        }
        GET_DEVICE_DATA => match class {
            CLASS_HTD => {
                let head = head(vfx1, now_ms);
                regs.bx = head[0] as u16;
                regs.cx = head[1] as u16;
                regs.dx = head[2] as u16;
                regs.ax = OK;
            }
            CLASS_HPD => {
                let puck = *vfx1.tracking();
                regs.bx = 0;
                regs.cx = puck.puck[0] as u16;
                regs.dx = puck.puck[1] as u16;
                regs.ax = u16::from(puck.buttons);
            }
            _ => regs.ax = FAILED << 8,
        },
        SET_ZERO | RESET_ZERO => {
            if matches!(class, CLASS_HTD | CLASS_ALL) {
                vfx1.driver.zero = if function == SET_ZERO {
                    vfx1.poll_head(now_ms)
                } else {
                    [0; 3]
                };
                regs.ax = OK;
            } else {
                regs.ax = FAILED;
            }
        }
        SET_VIDEO_MODE => {
            if class == CLASS_HMS && regs.cx <= 2 {
                vfx1.driver.video_mode = regs.cx as u8;
                regs.ax = OK;
            } else {
                regs.ax = FAILED;
            }
        }
        SET_VIDEO_PAGE => {
            vfx1.page_select(regs.cx as u8);
            regs.ax = OK;
        }
        GET_CONFIGURATION => {
            out = configuration(&vfx1.driver);
            regs.cx = out.len() as u16;
            regs.ax = OK;
        }
        SET_CONFIGURATION => {
            apply_configuration(&mut vfx1.driver, handed);
            regs.ax = OK;
        }
        REPORT => {
            out = report(vfx1, now_ms);
            regs.cx = out.len() as u16;
            regs.ax = OK;
        }
        GET_DATA_SIZES => {
            regs.cx = configuration(&vfx1.driver).len() as u16;
            regs.dx = (vfx1.driver.axes.iter().filter(|on| **on).count() * 2) as u16;
            regs.ax = OK;
        }
        RESET_DEVICE | SET_DATA_SIZE | CONTINUE | TEST_DEVICES => regs.ax = OK,
        // Callbacks (90h) aren't offered: games poll.
        _ => regs.ax = FAILED,
    }
    out
}

/// The head as the game reads it now: from where it zeroed it.
fn head(vfx1: &mut Vfx1, now_ms: f64) -> [i16; 3] {
    let head = vfx1.poll_head(now_ms);
    let zero = vfx1.driver.zero;
    std::array::from_fn(|i| head[i].wrapping_sub(zero[i]))
}

/// REPORT's data: a word for each axis reported, in their order.
fn report(vfx1: &mut Vfx1, now_ms: f64) -> Vec<u8> {
    let head = head(vfx1, now_ms);
    let tracking = *vfx1.tracking();
    let values = [
        head[0],
        head[1],
        head[2],
        0,
        tracking.puck[0],
        tracking.puck[1],
        i16::from(tracking.buttons),
    ];
    values
        .iter()
        .zip(vfx1.driver.axes)
        .filter(|(_, on)| *on)
        .flat_map(|(value, _)| value.to_le_bytes())
        .collect()
}

/// The configuration packets as VFX1.COM 2.14 builds them: SYSI, VNDI,
/// HMS, HTD, HPD and NULL.
pub(crate) fn configuration(driver: &Driver) -> Vec<u8> {
    let mut out = Vec::with_capacity(230);
    let word = |out: &mut Vec<u8>, v: i32| out.extend_from_slice(&(v as u16).to_le_bytes());
    let text = |out: &mut Vec<u8>, s: &str, field: usize| {
        out.extend(s.bytes().chain(std::iter::repeat(0)).take(field));
    };
    // A rotation's descriptor: its largest and smallest values, its steps,
    // its flags (02h wraps, 08h generated, 80h enabled).
    let rotation = |out: &mut Vec<u8>, max: i32, min: i32, steps: i32, flags: u8, on: bool| {
        word(out, max);
        word(out, min);
        word(out, steps);
        out.push(flags | if on { 0x80 } else { 0 });
        out.push(0);
    };

    // SYSI: the system "SENS".
    out.extend_from_slice(&[15, PKT_SYSI, 0, 0, 0, 1, 0xFF]);
    text(&mut out, "SENS", 4);
    word(&mut out, 0x100);
    word(&mut out, 0);

    // VNDI: the SDK's example enables the tracker for this product name.
    out.extend_from_slice(&[98, PKT_VNDI]);
    text(&mut out, "Forte Technologies, Inc.", 32);
    text(&mut out, "VFX1 CyberPuck", 32);
    text(&mut out, "0", 16);
    text(&mut out, "000B", 16);

    // HMS, the display: attached (bit 0, which Z.A.R. wants), audio,
    // colour, stereo and a tracker; its fields of view and the like.
    out.extend_from_slice(&[
        44,
        PKT_HMS,
        0,
        1,
        0x01 | if driver.hms_flag7 { 0x80 } else { 0 },
        0,
        0,
        0,
        0x3D,
        1,
    ]);
    word(&mut out, 0x2FC3);
    word(&mut out, 157);
    word(&mut out, 123);
    out.extend_from_slice(&[124, 0, 0, 0xD8, 60, driver.video_mode, 0, 2]);
    word(&mut out, 0x1300);
    word(&mut out, 0x0101);
    out.extend_from_slice(&[0; 16]);

    // HTD, the head tracker: yaw wraps, pitch and roll go about 70 degrees
    // each way.
    let htd_on = driver.axes[..3].iter().any(|on| *on);
    out.extend_from_slice(&[
        32,
        PKT_HTD,
        CLASS_HTD,
        1,
        if htd_on { 0 } else { 0x80 },
        0,
        0,
        0,
    ]);
    rotation(&mut out, 0x7FFF, -0x7FFE, 0xFFFE, 0x0A, driver.axes[0]);
    rotation(&mut out, 0x31CE, -0x31CE, 0x639D, 0x08, driver.axes[1]);
    rotation(&mut out, 0x31CE, -0x31CE, 0x639D, 0x08, driver.axes[2]);

    // HPD, the CyberPuck: no yaw, pitch and roll, three buttons.
    let hpd_on = driver.axes[3..].iter().any(|on| *on);
    out.extend_from_slice(&[
        34,
        PKT_HPD,
        CLASS_HPD,
        1,
        if hpd_on { 0 } else { 0x40 },
        0,
        0,
        0,
    ]);
    rotation(&mut out, 0, 0, 0, 0, driver.axes[3]);
    rotation(&mut out, 0x31CE, -0x31CE, 0x1FF, 0x08, driver.axes[4]);
    rotation(&mut out, 0x31CE, -0x31CE, 0x1FF, 0x08, driver.axes[5]);
    out.extend_from_slice(&[3, if driver.axes[6] { 0x80 } else { 0 }]);

    out.extend_from_slice(&[2, PKT_NULL]);
    out
}

/// SET_CONFIGURATION: the packets handed back, of which the axes' enable
/// bits and a set-video-mode packet count.
fn apply_configuration(driver: &mut Driver, config: &[u8]) {
    let mut i = 0;
    while i + 2 <= config.len() {
        let (size, kind) = (config[i] as usize, config[i + 1]);
        if kind == PKT_NULL || size < 2 || i + size > config.len() {
            break;
        }
        let packet = &config[i..i + size];
        let on = |offset: usize| packet[offset] & 0x80 != 0;
        match kind {
            PKT_HTD if size >= 32 => {
                let off = packet[4] & 0x80 != 0;
                for (axis, offset) in [(0, 14), (1, 22), (2, 30)] {
                    driver.axes[axis] = !off && on(offset);
                }
            }
            PKT_HPD if size >= 34 => {
                let off = packet[4] & 0x40 != 0;
                for (axis, offset) in [(3, 14), (4, 22), (5, 30), (6, 33)] {
                    driver.axes[axis] = !off && on(offset);
                }
            }
            PKT_SVM if size >= 6 => {
                driver.hms_flag7 = on(4);
                if packet[5] <= 2 {
                    driver.video_mode = packet[5];
                }
            }
            _ => {}
        }
        i += size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets() {
        let config = configuration(&Driver::default());
        // Each packet is as long as it says, and NULL ends them.
        let mut sizes = Vec::new();
        let mut i = 0;
        while config[i + 1] != PKT_NULL {
            sizes.push((config[i + 1], config[i]));
            i += config[i] as usize;
        }
        assert_eq!(
            sizes,
            [
                (PKT_SYSI, 15),
                (PKT_VNDI, 98),
                (PKT_HMS, 44),
                (PKT_HTD, 32),
                (PKT_HPD, 34)
            ]
        );
        assert_eq!(config.len(), 15 + 98 + 44 + 32 + 34 + 2);
        assert_eq!(&config[7..11], b"SENS");
        assert_eq!(&config[15 + 34..15 + 48], b"VFX1 CyberPuck");
    }

    #[test]
    fn configuration_handed_back() {
        let mut driver = Driver::default();
        let mut config = configuration(&driver);
        // Head roll and the puck's buttons off, and line-sequential stereo.
        let htd = 15 + 98 + 44;
        let hpd = htd + 32;
        config[htd + 30] &= !0x80;
        config[hpd + 33] = 0;
        let null = config.len() - 2;
        config.splice(null..null, [6, PKT_SVM, 0, 0, 0x80, 2]);
        apply_configuration(&mut driver, &config);
        assert_eq!(driver.axes, [true, true, false, false, true, true, false]);
        assert_eq!(driver.video_mode, 2);
        assert!(driver.hms_flag7);
    }

    #[test]
    fn detection_and_tracking() {
        let mut vfx1 = Vfx1::new(0x260);
        // Another prefix is left alone; 60h answers with the handshake.
        let mut regs = Regs {
            ax: 0x657F,
            bx: 0x1234,
            cx: 0,
            dx: 0,
        };
        call(&mut vfx1, &mut regs, 0.0, &[]);
        assert_eq!(
            regs,
            Regs {
                ax: 0x657F,
                bx: 0x1234,
                cx: 0,
                dx: 0
            }
        );
        let mut regs = Regs {
            ax: 0x607F,
            bx: 0x1234,
            cx: 0,
            dx: 0,
        };
        call(&mut vfx1, &mut regs, 0.0, &[]);
        assert_eq!(
            regs,
            Regs {
                ax: 0x7F60,
                bx: 0,
                cx: 0x101,
                dx: 0
            }
        );

        vfx1.set_tracking(
            super::super::Tracking {
                head: [1000, -500, 20],
                puck: [7, -7],
                buttons: 5,
            },
            false,
        );
        let mut regs = Regs {
            ax: 0x6070,
            bx: 0x0301,
            ..Default::default()
        };
        call(&mut vfx1, &mut regs, 0.0, &[]);
        assert_eq!(
            regs,
            Regs {
                ax: 0,
                bx: 1000,
                cx: -500i16 as u16,
                dx: 20
            }
        );
        // Zeroed there, the head reads 0 until it moves.
        call(
            &mut vfx1,
            &mut Regs {
                ax: 0x6008,
                bx: 0x0301,
                ..Default::default()
            },
            0.0,
            &[],
        );
        let mut regs = Regs {
            ax: 0x6070,
            bx: 0x0301,
            ..Default::default()
        };
        call(&mut vfx1, &mut regs, 0.0, &[]);
        assert_eq!((regs.bx, regs.cx, regs.dx), (0, 0, 0));
        let mut regs = Regs {
            ax: 0x6070,
            bx: 0x0401,
            ..Default::default()
        };
        call(&mut vfx1, &mut regs, 0.0, &[]);
        assert_eq!(
            regs,
            Regs {
                ax: 5,
                bx: 0,
                cx: 7,
                dx: -7i16 as u16
            }
        );
        // The page and the video mode reach the host.
        call(
            &mut vfx1,
            &mut Regs {
                ax: 0x6071,
                bx: 0x0201,
                cx: 1,
                dx: 0,
            },
            0.0,
            &[],
        );
        call(
            &mut vfx1,
            &mut Regs {
                ax: 0x6080,
                bx: 0x0201,
                cx: 0x23,
                dx: 0,
            },
            0.0,
            &[],
        );
        let activity = vfx1.activity();
        assert_eq!(
            (
                activity.video_mode,
                activity.page_select,
                activity.page_writes
            ),
            (1, 0x23, 1)
        );
    }

    #[test]
    fn report_follows_the_axes() {
        let mut vfx1 = Vfx1::new(0x260);
        vfx1.set_tracking(
            super::super::Tracking {
                head: [100, -200, 300],
                puck: [5, 6],
                buttons: 3,
            },
            false,
        );
        vfx1.driver.zero = [50, 0, 0];
        assert_eq!(
            report(&mut vfx1, 0.0),
            [50, 0, 0x38, 0xFF, 0x2C, 1, 5, 0, 6, 0, 3, 0]
        );
        vfx1.driver.axes = [true, false, false, false, false, false, true];
        assert_eq!(report(&mut vfx1, 0.0), [50, 0, 3, 0]);
    }
}
