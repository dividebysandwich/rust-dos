//! The IDE channels of a booted system: the primary (ports 1F0h-1F7h and
//! 3F6h/3F7h, IRQ 14) and the secondary (170h-177h and 376h/377h, IRQ 15),
//! each with a master and a slave device. A device is an ATAPI CD-ROM
//! drive (atapi.rs) with a CD image of the machine's, or an ATA hard disk
//! (ata.rs) with one of its hard disk images, the same image the BIOS's
//! INT 13h reads and writes.
//!
//! The controller's part is DOSBox-X's (ide.cpp): the task file registers
//! written reach the selected device, whose interrupt request is the
//! channel's while nIEN is clear; a device that isn't there reads 0, and
//! the soft reset (SRST) resets both.

pub mod ata;
pub mod atapi;
pub mod int13;

use crate::cdrom::audio::CdPlayer;
use crate::cdrom::image::CdImage;
use crate::disk::DiskController;
use crate::savestate::{Reader, State, Writer};
use crate::timer::PIT_HZ;
use std::rc::Rc;

pub use ata::Ata;
pub use atapi::Atapi;

pub(crate) const BSY: u8 = 0x80;
pub(crate) const DRDY: u8 = 0x40;
pub(crate) const DSC: u8 = 0x10;
pub(crate) const DRQ: u8 = 0x08;
pub(crate) const ERR: u8 = 0x01;

pub(crate) fn ticks(ms: f64) -> u64 {
    (ms * PIT_HZ as f64 / 1000.0).ceil() as u64
}

/// What a device's commands work with: the drives' images, the CD audio
/// player, the time.
pub struct Env<'a> {
    pub disks: &'a DiskController,
    pub player: &'a mut CdPlayer,
    /// PIT ticks.
    pub now: u64,
    /// How long a hard disk takes for a sector at `hard_disk_speed`, in
    /// nanoseconds; 0 at the maximum.
    pub hard_disk_ns_per_sector: u64,
    /// The commands are the BIOS's show for a V86 monitor, which the disks
    /// carry out at once (DOSBox-X's `faked_command`).
    pub faked: bool,
}

impl Env<'_> {
    /// The CD image in the CD-ROM drive `drive`.
    pub fn cd(&self, drive: u8) -> Option<Rc<CdImage>> {
        self.disks.cd_image(drive)
    }
}

/// One of the two channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelId {
    Primary,
    Secondary,
}

crate::state_enum!(ChannelId { ChannelId::Primary, ChannelId::Secondary });

impl ChannelId {
    pub const ALL: [ChannelId; 2] = [ChannelId::Primary, ChannelId::Secondary];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The task file's ports.
    pub fn base(self) -> u16 {
        match self {
            ChannelId::Primary => 0x1F0,
            ChannelId::Secondary => 0x170,
        }
    }

    /// The alternate status and device control register, and the drive
    /// address register after it.
    pub fn alt(self) -> u16 {
        self.base() + 0x206
    }

    pub fn irq(self) -> u8 {
        match self {
            ChannelId::Primary => 14,
            ChannelId::Secondary => 15,
        }
    }

    /// The handle of its Plug and Play BIOS node: where DOSBox-X has it,
    /// so a Windows 95 installed there knows it.
    pub fn pnp_handle(self) -> u8 {
        match self {
            ChannelId::Primary => 0x0F,
            ChannelId::Secondary => 0x10,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ChannelId::Primary => "primary",
            ChannelId::Secondary => "secondary",
        }
    }

    /// Whether `port` is one of the channel's.
    pub fn claims(self, port: u16) -> bool {
        (self.base()..self.base() + 8).contains(&port) || port == self.alt() || port == self.alt() + 1
    }
}

/// Where on the channels a device goes: MOUNT's `-ide 1m` (primary
/// master), `1s`, `2m` or `2s`, as DOSBox-X has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdeSlot {
    pub channel: ChannelId,
    pub slave: bool,
}

impl IdeSlot {
    /// The slots in the order a hard disk takes the free ones; a CD-ROM
    /// drive takes them from the secondary master on.
    pub const HARD_DISK_ORDER: [IdeSlot; 4] =
        [Self::new(ChannelId::Primary, false), Self::new(ChannelId::Primary, true), Self::new(ChannelId::Secondary, true), Self::new(ChannelId::Secondary, false)];
    pub const CD_ORDER: [IdeSlot; 4] =
        [Self::new(ChannelId::Secondary, false), Self::new(ChannelId::Secondary, true), Self::new(ChannelId::Primary, true), Self::new(ChannelId::Primary, false)];

    pub const fn new(channel: ChannelId, slave: bool) -> Self {
        Self { channel, slave }
    }

    /// "1m", "1s", "2m" or "2s", in any case.
    pub fn parse(s: &str) -> Option<Self> {
        let channel = match s.as_bytes().first()? {
            b'1' => ChannelId::Primary,
            b'2' => ChannelId::Secondary,
            _ => return None,
        };
        match s.get(1..)?.to_ascii_lowercase().as_str() {
            "m" => Some(Self::new(channel, false)),
            "s" => Some(Self::new(channel, true)),
            _ => None,
        }
    }

    pub fn name(self) -> String {
        format!("{}{}", self.channel.index() + 1, if self.slave { 's' } else { 'm' })
    }

    /// "the primary master".
    pub fn describe(self) -> String {
        format!("the {} {}", self.channel.name(), if self.slave { "slave" } else { "master" })
    }
}

/// A device on a channel.
#[derive(Clone, Debug)]
pub enum Device {
    Atapi(Atapi),
    Ata(Ata),
}

/// The same for either kind of device.
macro_rules! each {
    ($device:expr, $d:ident => $e:expr) => {
        match $device {
            Device::Atapi($d) => $e,
            Device::Ata($d) => $e,
        }
    };
}

impl Device {
    fn irq_signal(&self) -> bool {
        each!(self, d => d.irq_signal())
    }

    fn status(&self) -> u8 {
        each!(self, d => d.status())
    }

    fn busy(&self) -> bool {
        self.status() & BSY != 0
    }

    fn drivehead(&self) -> u8 {
        each!(self, d => d.drivehead())
    }

    fn next_event(&self) -> Option<u64> {
        each!(self, d => d.next_event())
    }

    fn service(&mut self, env: &mut Env) {
        each!(self, d => d.service(env))
    }

    fn take_log(&mut self) -> Vec<String> {
        each!(self, d => std::mem::take(&mut d.log))
    }

    /// The hard disk's drive slot and the sectors it read or wrote since
    /// last asked.
    fn take_activity(&mut self) -> Option<(u8, Vec<(u64, u32)>)> {
        match self {
            Device::Ata(d) if !d.activity.is_empty() => Some((d.drive, std::mem::take(&mut d.activity))),
            _ => None,
        }
    }

    /// The CD-ROM drive of DOS drive `drive`, if this is it.
    pub fn cd_drive(&mut self, drive: u8) -> Option<&mut Atapi> {
        match self {
            Device::Atapi(d) if d.drive == drive => Some(d),
            _ => None,
        }
    }
}

impl Default for Device {
    fn default() -> Self {
        Device::Atapi(Atapi::new(0, false))
    }
}

impl State for Device {
    fn save(&self, w: &mut Writer) {
        match self {
            Device::Atapi(d) => {
                0u8.save(w);
                d.save(w);
            }
            Device::Ata(d) => {
                1u8.save(w);
                d.save(w);
            }
        }
    }

    fn load(&mut self, r: &mut Reader) -> crate::savestate::Result<()> {
        let mut kind = 0u8;
        kind.load(r)?;
        *self = match kind {
            0 => Device::Atapi(Atapi::new(0, false)),
            1 => Device::Ata(Ata::new(0, crate::diskimage::Chs { cylinders: 1, heads: 1, sectors: 1 }, 1)),
            _ => return Err(crate::savestate::StateError::Invalid("an IDE device it doesn't know".into())),
        };
        each!(self, d => d.load(r))
    }
}

/// A channel and its master and slave devices.
#[derive(Clone, Debug)]
pub struct Channel {
    pub id: ChannelId,
    pub devices: [Option<Device>; 2],
    /// The selected device (the drive/head register's bit 4), interrupts
    /// disabled (nIEN), and the soft reset going on.
    select: u8,
    nien: bool,
    host_reset: bool,
    /// The level of the channel's IRQ the bus last passed to the interrupt
    /// controller.
    pub pic_line: bool,
}

crate::state_fields!(Channel { id, devices, select, nien, host_reset, pic_line });

impl Default for Channel {
    fn default() -> Self {
        Self::new(ChannelId::Primary)
    }
}

impl Channel {
    /// The channel `id` without devices.
    pub fn new(id: ChannelId) -> Self {
        Self { id, devices: [None, None], select: 0, nien: false, host_reset: false, pic_line: false }
    }

    fn selected(&mut self) -> Option<&mut Device> {
        self.devices[self.select as usize].as_mut()
    }

    /// The interrupt line: the selected device's request, unless nIEN.
    pub fn irq(&self) -> bool {
        !self.nien && self.devices[self.select as usize].as_ref().is_some_and(Device::irq_signal)
    }

    /// When a device next needs attention (PIT ticks).
    pub fn next_event(&self) -> Option<u64> {
        self.devices.iter().flatten().filter_map(Device::next_event).min()
    }

    /// Carry out what came due.
    pub fn service(&mut self, env: &mut Env) {
        for device in self.devices.iter_mut().flatten() {
            device.service(env);
        }
    }

    /// The devices' messages for the log.
    pub fn take_log(&mut self) -> Vec<String> {
        self.devices.iter_mut().flatten().flat_map(Device::take_log).collect()
    }

    /// What the hard disks read and wrote since last asked: their drive
    /// slots and (first sector, sectors).
    pub fn take_activity(&mut self) -> Vec<(u8, Vec<(u64, u32)>)> {
        self.devices.iter_mut().flatten().filter_map(Device::take_activity).collect()
    }

    /// The CD-ROM drive of DOS drive `drive` on the channel, if there is
    /// one.
    pub fn cd_drive(&mut self, drive: u8) -> Option<&mut Atapi> {
        self.devices.iter_mut().flatten().find_map(|d| d.cd_drive(drive))
    }

    /// What a BIOS's INT 13h did to disk `slot` (0 master, 1 slave), done
    /// to the channel at once (DOSBox-X's `int13fakeio`): the devices
    /// selected in turn up to it, then the disk reset, or its task file
    /// left as after reading a sector.
    pub fn bios_access(&mut self, slot: usize, access: int13::BiosAccess, env: &mut Env) {
        // Not in the middle of a command of the system's own driver.
        if self.devices.iter().flatten().any(|d| d.status() & (BSY | DRQ) != 0) {
            return;
        }
        let base = self.id.base();
        for s in 0..=slot {
            if self.devices[s].is_some() {
                self.read(base + 7, env);
                self.write(base + 6, (s as u8) << 4, env);
            }
        }
        match access {
            int13::BiosAccess::Reset => {
                self.write(base + 7, 0x08, env);
                if let Some(Device::Ata(d)) = &mut self.devices[slot] {
                    d.clear_irq();
                }
            }
            int13::BiosAccess::Read { lba, drivehead } => {
                if let Some(Device::Ata(d)) = &mut self.devices[slot] {
                    d.bios_read(lba, drivehead);
                }
            }
        }
    }

    /// The hard disk of drive slot `drive` on the channel: its slot and
    /// the geometry its CHS addresses go by.
    pub fn disk_of(&self, drive: u8) -> Option<(usize, ata::Geometry)> {
        self.devices.iter().enumerate().find_map(|(slot, device)| match device {
            Some(Device::Ata(d)) if d.drive == drive => Some((slot, d.logical_geometry())),
            _ => None,
        })
    }

    // --- Ports ---

    /// A byte read at `port`, one of the channel's.
    pub fn read(&mut self, port: u16, env: &mut Env) -> u8 {
        let alt = self.id.alt();
        if port == alt || port == alt + 1 {
            return self.read_alt(port);
        }
        let reg = port - self.id.base();
        let Some(device) = self.selected() else {
            // No device: its registers read 0.
            return if reg == 0 { 0xFF } else { 0 };
        };
        each!(device, d => d.read_register(reg, env))
    }

    fn read_alt(&self, port: u16) -> u8 {
        let device = self.devices[self.select as usize].as_ref();
        if port == self.id.alt() {
            // The status, leaving the interrupt alone.
            device.map_or(0, Device::status)
        } else {
            // The drive address register: the selected drive (active low)
            // and head.
            let heads = device.map_or(0x3C, |d| ((d.drivehead() & 0xF) ^ 0xF) << 2);
            0x80 | (self.select != 0) as u8 | ((self.select != 1) as u8) << 1 | heads
        }
    }

    /// The data port read 2 or 4 bytes wide: a doubleword is two words.
    pub fn read_wide(&mut self, len: u8, env: &mut Env) -> u32 {
        if len == 4 {
            let low = self.read_wide(2, env);
            return low | self.read_wide(2, env) << 16;
        }
        match self.selected() {
            Some(device) => each!(device, d => d.read_data(env)),
            None => 0xFFFF,
        }
    }

    /// A byte written at `port`, one of the channel's.
    pub fn write(&mut self, port: u16, value: u8, env: &mut Env) {
        let alt = self.id.alt();
        if port == alt {
            self.write_control(value);
            return;
        }
        if port == alt + 1 {
            return;
        }
        let reg = port - self.id.base();
        // A busy device takes nothing: drivers that select the same drive
        // again are let be.
        if self.selected().is_some_and(|d| d.busy()) {
            return;
        }
        if reg == 6 {
            self.select = (value >> 4) & 1;
            if let Some(device) = self.selected() {
                each!(device, d => d.select(value));
            }
            return;
        }
        if let Some(device) = self.selected() {
            each!(device, d => d.write_register(reg, value, env));
        }
    }

    /// The data port written 2 or 4 bytes wide: a doubleword is two words.
    pub fn write_wide(&mut self, value: u32, len: u8, env: &mut Env) {
        if len == 4 {
            self.write_wide(value & 0xFFFF, 2, env);
            self.write_wide(value >> 16, 2, env);
            return;
        }
        if let Some(device) = self.selected() {
            each!(device, d => d.write_data(value & 0xFFFF, env));
        }
    }

    /// The device control register: nIEN, and SRST, the soft reset of
    /// both devices, which puts their signatures in their task files.
    fn write_control(&mut self, value: u8) {
        self.nien = value & 2 != 0;
        let reset = value & 4 != 0;
        if reset && !self.host_reset {
            for device in self.devices.iter_mut().flatten() {
                each!(device, d => d.reset_begin());
            }
            self.host_reset = true;
        } else if !reset && self.host_reset {
            for device in self.devices.iter_mut().flatten() {
                each!(device, d => d.reset_complete());
            }
            self.host_reset = false;
        }
    }
}
