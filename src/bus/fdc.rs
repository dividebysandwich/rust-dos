//! The floppy disk controller of a booted system on the bus: put in when
//! the system boots (with `floppy_controller`), its ports and IRQ 6, DMA
//! channel 2, its timed commands among the timer events, and disks
//! changing. Its drives 0 and 1 have the disks of floppy units 00h and 01h
//! (`boot::unit_drive`), the images INT 13h reads and writes.

use super::Bus;
use crate::fdc::{DRIVES, Env, Fdc, IRQ};

impl Bus {
    /// At a boot: the controller, unless `floppy_controller` is off or a
    /// sound or network card has IRQ 6. The built-in DOS has none.
    pub fn attach_fdc(&mut self) {
        self.detach_fdc();
        if !self.floppy_controller {
            return;
        }
        if let Some(owner) = self.irq_owner(IRQ) {
            self.log_string(&format!(
                "[FDC] IRQ {} is the {}'s: no floppy disk controller",
                IRQ, owner
            ));
            return;
        }
        self.fdc = Some(Fdc::new());
    }

    /// Take the controller out.
    pub fn detach_fdc(&mut self) {
        if self.fdc.take().is_some_and(|fdc| fdc.pic_line) {
            self.withdraw_irq(IRQ);
            self.refresh_irq();
        }
    }

    /// Whether `port` is the controller's, if it is there.
    #[inline]
    pub(crate) fn fdc_claims(&self, port: u16) -> bool {
        self.fdc.is_some() && Fdc::claims(port)
    }

    /// The drive (0 = A:) of floppy unit `unit`, if it has one.
    fn fdc_drive(&self, unit: usize) -> Option<u8> {
        crate::boot::unit_drive(self, unit as u8)
    }

    /// Run `f` on the controller with the disks, DMA and memory its
    /// commands work with, then follow its interrupt line and timer.
    fn with_fdc<T>(&mut self, f: impl FnOnce(&mut Fdc, &mut Env) -> T) -> Option<T> {
        self.fdc.as_ref()?;
        let disks = std::array::from_fn(|unit| {
            self.fdc_drive(unit)
                .and_then(|drive| self.disk.bios_image(drive))
        });
        let now = self.clock.now_ticks();
        let ns_per_sector = self.disk_io.cost_ns(
            crate::diskio::DiskClass::Floppy,
            crate::diskimage::SECTOR_SIZE as u32,
        );
        let fdc = self.fdc.as_mut()?;
        let mut env = Env {
            disks,
            dma: &mut self.dma,
            ram: &mut self.ram,
            now,
            ns_per_sector,
            written: None,
            faked: self.faked_io,
        };
        let result = f(fdc, &mut env);
        let written = env.written.take();
        if let Some(span) = written {
            self.bump_page_gens(span.start, span.end);
        }
        self.sync_fdc();
        Some(result)
    }

    fn sync_fdc(&mut self) {
        let Some(fdc) = &mut self.fdc else { return };
        let line = fdc.irq();
        let log = std::mem::take(&mut fdc.log);
        let activity = std::mem::take(&mut fdc.activity);
        if line != fdc.pic_line {
            fdc.pic_line = line;
            if line {
                self.pic.raise(IRQ);
            } else {
                self.withdraw_irq(IRQ);
            }
            self.refresh_irq();
        }
        for line in log {
            self.log_string(&line);
        }
        // The drive's light and noise, for the time its sectors took. The
        // command's execution phase already took that time, so nothing is
        // charged to `disk_io` for an HLE service to wait out.
        for (unit, lba, sectors) in activity {
            let Some(drive) = self.fdc_drive(unit) else { continue };
            self.drives_active |= 1 << drive;
            let bytes = sectors.saturating_mul(crate::diskimage::SECTOR_SIZE as u32);
            self.activity.io_bytes += bytes as u64;
            let per_track = self.disk.bios_image(drive).map_or(1, |disk| disk.geometry().sectors.max(1) as u64);
            let class = crate::diskio::DiskClass::Floppy;
            let ns = self.disk_io.cost_ns(class, bytes);
            self.disk_noise(class, crate::disknoise::Access::Track(lba / per_track), ns);
        }
        self.clock.schedule(self.next_event());
    }

    /// A port read needs none of the disks or DMA: only reading a result
    /// byte changes the controller, and `sync_fdc` follows that. Status
    /// polls between the bytes leave the timers alone.
    pub(crate) fn fdc_read(&mut self, port: u16) -> u8 {
        let Some(fdc) = &mut self.fdc else { return 0xFF };
        let value = fdc.read(port);
        if port == crate::fdc::DATA {
            self.sync_fdc();
        }
        value
    }

    pub(crate) fn fdc_write(&mut self, port: u16, value: u8) {
        self.with_fdc(|fdc, env| fdc.write(port, value, env));
    }

    pub(crate) fn fdc_next_event(&self) -> Option<u64> {
        self.fdc.as_ref().and_then(Fdc::next_event)
    }

    /// Carry out the commands and seeks that came due.
    pub(crate) fn fdc_service(&mut self) {
        self.with_fdc(|fdc, env| fdc.service(env));
    }

    /// Another disk is in `drive`, or none: if it is the disk mounted as 0
    /// or 1, or A: or B: where no disk is mounted as that number, the
    /// floppy unit's disk change line goes on.
    pub(crate) fn fdc_media_changed(&mut self, drive: u8) {
        if let Some(fdc) = &mut self.fdc {
            for unit in 0..DRIVES {
                let numbered = crate::disk::numbered_drive(unit as u8);
                if drive == numbered || (drive == unit as u8 && !self.disk.is_mounted(numbered)) {
                    fdc.media_changed(unit);
                }
            }
        }
    }
}
