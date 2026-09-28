//! The IDE channels of a booted system on the bus: put in when the system
//! boots, their ports and IRQs 14 and 15, their devices' timed commands
//! among the timer events, and discs changing.

use super::Bus;
use crate::disk::DriveKind;
use crate::ide::{Ata, Atapi, Channel, ChannelId, Device, Env, IdeSlot};

/// A data port access takes a PIO mode 4 cycle.
const DATA_PORT_NS: u64 = 120;

impl Bus {
    /// At a boot: the IDE channels with the machine's hard disks (with
    /// `ide_hard_disks`), BIOS units 80h and 81h the primary master and
    /// slave, and its CD-ROM drive with a CD image, the secondary master,
    /// or where their MOUNT's `-ide` puts them. A channel whose IRQ is a
    /// sound or network card's isn't there.
    pub fn attach_ide(&mut self) {
        self.detach_ide();
        // The devices, with the slots they ask for.
        let mut wanted: Vec<(Option<IdeSlot>, &[IdeSlot; 4], Device, String)> = Vec::new();
        if let Some(drive) =
            self.disk.drives_of_kind(DriveKind::CdRom).into_iter().find(|&d| self.disk.cd_image(d).is_some())
        {
            let what = format!("{}: is the CD-ROM drive", crate::disk::drive_letter(drive));
            let device = Device::Atapi(Atapi::new(drive, true));
            wanted.push((self.disk.ide_slot(drive), &IdeSlot::CD_ORDER, device, what));
        }
        if self.ide_hard_disks {
            for (i, drive) in crate::boot::hard_disk_drives(self).into_iter().enumerate() {
                let Some(image) = self.disk.bios_image(drive) else { continue };
                let what = format!("Hard disk {:02X}h is the ATA disk", 0x80 + i);
                let device = Device::Ata(Ata::new(drive, image.geometry(), image.sectors()));
                wanted.push((self.disk.ide_slot(drive), &IdeSlot::HARD_DISK_ORDER, device, what));
            }
        }
        if wanted.is_empty() {
            return;
        }
        // The slots asked for first, then the others in order.
        let mut slots: [[Option<(Device, String)>; 2]; 2] = Default::default();
        let (asked, rest): (Vec<_>, Vec<_>) = wanted.into_iter().partition(|(slot, ..)| slot.is_some());
        for (slot, order, device, what) in asked.into_iter().chain(rest) {
            let free = |slot: &IdeSlot| slots[slot.channel.index()][slot.slave as usize].is_none();
            match slot.filter(free).or_else(|| order.iter().copied().find(free)) {
                Some(s) => slots[s.channel.index()][s.slave as usize] = Some((device, format!("{} on {}", what, s.describe()))),
                None => self.log_string(&format!("[IDE] No IDE slot is left: {} of none", what)),
            }
        }
        for id in ChannelId::ALL {
            let [master, slave] = std::mem::take(&mut slots[id.index()]);
            if master.is_none() && slave.is_none() {
                continue;
            }
            if let Some(owner) = self.irq_owner(id.irq()) {
                for (_, what) in [master, slave].into_iter().flatten() {
                    self.log_string(&format!("[IDE] IRQ {} is the {}'s: {} of no {} channel", id.irq(), owner, what, id.name()));
                }
                continue;
            }
            let mut channel = Channel::new(id);
            for (i, placed) in [master, slave].into_iter().enumerate() {
                if let Some((device, what)) = placed {
                    self.log_string(&format!("[IDE] {}", what));
                    channel.devices[i] = Some(device);
                }
            }
            self.ide[id.index()] = Some(channel);
        }
    }

    /// The device configured on IRQ `irq`, which an IDE channel can't
    /// have.
    fn irq_owner(&self, irq: u8) -> Option<&'static str> {
        if self.sb.as_ref().is_some_and(|sb| sb.config.irq == irq) {
            Some("Sound Blaster")
        } else if self.gus.as_ref().and_then(|gus| gus.irq()) == Some(irq) {
            Some("Ultrasound")
        } else if self.net.nic.as_ref().is_some_and(|nic| nic.irq == irq) {
            Some("NE2000")
        } else {
            None
        }
    }

    /// Take the channels out, as the built-in DOS has none.
    pub fn detach_ide(&mut self) {
        for id in ChannelId::ALL {
            if let Some(channel) = self.ide[id.index()].take()
                && channel.pic_line
            {
                self.pic.lower(id.irq());
                self.refresh_irq();
            }
        }
    }

    /// Whether a channel is there.
    pub fn has_ide(&self) -> bool {
        self.ide.iter().any(Option::is_some)
    }

    /// The channel whose port `port` is, if it is there.
    #[inline]
    pub(crate) fn ide_channel(&self, port: u16) -> Option<ChannelId> {
        if !self.has_ide() {
            return None;
        }
        ChannelId::ALL.into_iter().find(|id| self.ide[id.index()].is_some() && id.claims(port))
    }

    /// Whether `port` is a channel's.
    #[inline]
    pub(crate) fn ide_claims(&self, port: u16) -> bool {
        self.ide_channel(port).is_some()
    }

    /// The channel whose data port `port` is.
    pub(crate) fn ide_data_port(&self, port: u16) -> Option<ChannelId> {
        self.ide_channel(port).filter(|id| id.base() == port)
    }

    /// Run `f` on channel `id` with what its devices' commands need, then
    /// follow its interrupt line and timer.
    fn with_ide<T>(&mut self, id: ChannelId, f: impl FnOnce(&mut Channel, &mut Env) -> T) -> Option<T> {
        let now = self.clock.now_ticks();
        let channel = self.ide[id.index()].as_mut()?;
        let class = crate::diskio::DiskClass::HardDisk;
        let hard_disk_ns_per_sector = self.disk_io.cost_ns(class, crate::diskimage::SECTOR_SIZE as u32);
        let faked = self.ide_faked;
        let mut env = Env { disks: &self.disk, player: &mut self.cdaudio, now, hard_disk_ns_per_sector, faked };
        let result = f(channel, &mut env);
        self.sync_ide(id);
        Some(result)
    }

    fn sync_ide(&mut self, id: ChannelId) {
        let Some(channel) = &mut self.ide[id.index()] else { return };
        let line = channel.irq();
        let log = channel.take_log();
        let activity = channel.take_activity();
        if line != channel.pic_line {
            channel.pic_line = line;
            if line {
                self.pic.raise(id.irq());
            } else {
                self.pic.lower(id.irq());
            }
            self.refresh_irq();
        }
        for line in log {
            self.log_string(&line);
        }
        // A hard disk's noise and light, for the time its sectors took.
        for (drive, transfers) in activity {
            self.drives_active |= 1 << drive;
            let per_track = self.disk.bios_image(drive).map_or(1, |disk| disk.geometry().sectors.max(1) as u64);
            for (lba, sectors) in transfers {
                let class = crate::diskio::DiskClass::HardDisk;
                let ns = self.disk_io.cost_ns(class, sectors * crate::diskimage::SECTOR_SIZE as u32);
                self.disk_noise(class, crate::disknoise::Access::Track(lba / per_track), ns);
            }
        }
        self.clock.schedule(self.next_event());
    }

    pub(crate) fn ide_read(&mut self, port: u16) -> u8 {
        let Some(id) = self.ide_channel(port) else { return 0xFF };
        self.with_ide(id, |channel, env| channel.read(port, env)).unwrap_or(0xFF)
    }

    pub(crate) fn ide_write(&mut self, port: u16, value: u8) {
        let Some(id) = self.ide_channel(port) else { return };
        self.with_ide(id, |channel, env| channel.write(port, value, env));
    }

    /// The data port, 2 or 4 bytes at a time.
    pub(crate) fn ide_read_wide(&mut self, id: ChannelId, len: u8) -> u32 {
        self.clock.stall(DATA_PORT_NS * (len as u64 / 2));
        let value = self.with_ide(id, |channel, env| channel.read_wide(len, env)).unwrap_or(0xFFFF_FFFF);
        self.log_port(id.base(), value, len, false);
        value
    }

    pub(crate) fn ide_write_wide(&mut self, id: ChannelId, value: u32, len: u8) {
        self.clock.stall(DATA_PORT_NS * (len as u64 / 2));
        self.log_port(id.base(), value, len, true);
        self.with_ide(id, |channel, env| channel.write_wide(value, len, env));
    }

    /// What the BIOS's INT 13h did to the disk in `slot` of channel `id`,
    /// done to it at once (`ide::int13`).
    pub(crate) fn ide_bios_access(&mut self, id: ChannelId, slot: usize, access: crate::ide::int13::BiosAccess) {
        self.with_ide(id, |channel, env| channel.bios_access(slot, access, env));
    }

    pub(crate) fn ide_next_event(&self) -> Option<u64> {
        self.ide.iter().flatten().filter_map(Channel::next_event).min()
    }

    /// Carry out the devices' commands that came due. CD audio is brought
    /// up to now first, as a command may play or report it.
    pub(crate) fn ide_service(&mut self) {
        self.audio_catch_up();
        for id in ChannelId::ALL {
            self.with_ide(id, |channel, env| channel.service(env));
        }
    }

    /// The disc of `drive` changed.
    pub(crate) fn ide_media_changed(&mut self, drive: u8) {
        let now = self.clock.now_ticks();
        let has_disc = self.disk.cd_image(drive).is_some();
        let mut changed = false;
        for channel in self.ide.iter_mut().flatten() {
            if let Some(cd) = channel.cd_drive(drive) {
                cd.media_changed(has_disc, now);
                changed = true;
            }
        }
        if changed {
            self.clock.schedule(self.next_event());
        }
    }
}
