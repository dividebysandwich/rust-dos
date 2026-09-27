//! The ATAPI CD-ROM drive of a booted system on the bus: put in when the
//! system boots with a CD image in a CD-ROM drive, its ports, IRQ 15, its
//! timed commands among the timer events, and the disc changing.

use super::Bus;
use crate::disk::DriveKind;
use crate::ide::{ALT, BASE, Env, IRQ, Ide};

/// A data port access takes a PIO mode 4 cycle.
const DATA_PORT_NS: u64 = 120;

impl Bus {
    /// At a boot: the secondary IDE channel with the CD-ROM drive, when a
    /// CD-ROM drive has an image and IRQ 15 isn't a sound card's.
    pub fn attach_ide(&mut self) {
        self.detach_ide();
        let Some(drive) =
            self.disk.drives_of_kind(DriveKind::CdRom).into_iter().find(|&d| self.disk.cd_image(d).is_some())
        else {
            return;
        };
        let letter = crate::disk::drive_letter(drive);
        let owner = if self.sb.as_ref().is_some_and(|sb| sb.config.irq == IRQ) {
            Some("Sound Blaster")
        } else if self.gus.as_ref().and_then(|gus| gus.irq()) == Some(IRQ) {
            Some("Ultrasound")
        } else {
            None
        };
        if let Some(owner) = owner {
            self.log_string(&format!("[IDE] IRQ 15 is the {}'s: no IDE channel for the CD-ROM drive {}:", owner, letter));
            return;
        }
        self.ide = Some(Ide::new(drive, true));
        self.log_string(&format!("[IDE] {}: is the CD-ROM drive on the secondary IDE channel", letter));
    }

    /// Take the channel out, as the built-in DOS has none.
    pub fn detach_ide(&mut self) {
        if let Some(ide) = self.ide.take()
            && ide.pic_line
        {
            self.pic.lower(IRQ);
            self.refresh_irq();
        }
    }

    /// Whether `port` is the channel's.
    #[inline]
    pub(crate) fn ide_claims(&self, port: u16) -> bool {
        self.ide.is_some() && ((BASE..BASE + 8).contains(&port) || port == ALT || port == ALT + 1)
    }

    /// Run `f` on the channel with what its commands need, then follow its
    /// interrupt line and timer.
    fn with_ide<T>(&mut self, f: impl FnOnce(&mut Ide, &mut Env) -> T) -> Option<T> {
        let now = self.clock.now_ticks();
        let ide = self.ide.as_mut()?;
        let image = self.disk.cd_image(ide.drive);
        let mut env = Env { image, player: &mut self.cdaudio, now };
        let result = f(ide, &mut env);
        self.sync_ide();
        Some(result)
    }

    fn sync_ide(&mut self) {
        let Some(ide) = &mut self.ide else { return };
        let line = ide.irq();
        let log = std::mem::take(&mut ide.log);
        if line != ide.pic_line {
            ide.pic_line = line;
            if line {
                self.pic.raise(IRQ);
            } else {
                self.pic.lower(IRQ);
            }
            self.refresh_irq();
        }
        for line in log {
            self.log_string(&line);
        }
        self.clock.schedule(self.next_event());
    }

    pub(crate) fn ide_read(&mut self, port: u16) -> u8 {
        self.with_ide(|ide, env| ide.read(port, env)).unwrap_or(0xFF)
    }

    pub(crate) fn ide_write(&mut self, port: u16, value: u8) {
        self.with_ide(|ide, env| ide.write(port, value, env));
    }

    /// The data port, 2 or 4 bytes at a time.
    pub(crate) fn ide_read_wide(&mut self, len: u8) -> u32 {
        self.clock.stall(DATA_PORT_NS);
        let value = self.with_ide(|ide, env| ide.read_wide(len, env)).unwrap_or(0xFFFF_FFFF);
        self.log_port(BASE, value, len, false);
        value
    }

    pub(crate) fn ide_write_wide(&mut self, value: u32, len: u8) {
        self.clock.stall(DATA_PORT_NS);
        self.log_port(BASE, value, len, true);
        self.with_ide(|ide, env| ide.write_wide(value, len, env));
    }

    pub(crate) fn ide_next_event(&self) -> Option<u64> {
        self.ide.as_ref().and_then(|ide| ide.next_event())
    }

    /// Carry out the channel's commands that came due. CD audio is
    /// brought up to now first, as a command may play or report it.
    pub(crate) fn ide_service(&mut self) {
        self.audio_catch_up();
        self.with_ide(|ide, env| ide.service(env));
    }

    /// The disc of `drive` changed.
    pub(crate) fn ide_media_changed(&mut self, drive: u8) {
        let now = self.clock.now_ticks();
        let has_disc = self.disk.cd_image(drive).is_some();
        if let Some(ide) = &mut self.ide
            && ide.drive == drive
        {
            ide.media_changed(has_disc, now);
            self.clock.schedule(self.next_event());
        }
    }
}
