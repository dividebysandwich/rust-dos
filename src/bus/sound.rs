//! Installing, configuring and resetting the sound cards.

use super::*;

impl Bus {
    /// Install (or with None remove) the Gravis Ultrasound.
    pub fn configure_gus(&mut self, config: Option<crate::gus::GusConfig>) {
        let now = self.clock.now_ticks();
        self.gus = config.filter(|c| c.enabled).map(|c| crate::gus::Gus::new(c, now));
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
    }

    /// Replace the sound hardware: the Sound Blaster (None removes it) and
    /// whether the FM chip is an OPL3.
    pub fn configure_sound(&mut self, sb: Option<crate::sb::SbConfig>, opl3: bool) {
        self.sb = sb.map(crate::sb::SoundBlaster::new);
        self.isapnp = sb.filter(|sb| sb.model == crate::sb::SbModel::Sb16Vibra).map(|sb| crate::isapnp::IsaPnp::vibra16c(&sb));
        self.opl = crate::opl::Opl::new(opl3);
        self.configure_awe();
        self.sync_sb_irq();
        self.clock.schedule(self.next_event());
    }

    /// Power-on state of the sound hardware, when a program ends. The
    /// Ultrasound keeps its DRAM, and when a resident program has hooked
    /// its interrupt (a MIDI driver like ULTRAMID), its settings too: only
    /// its voices stop. A ViBRA16C keeps its Plug and Play configuration,
    /// as the card does until RESET DRV, so a DOS configuration tool's
    /// resources outlast it: a reboot (`power_on_sound`) gives the BIOS's.
    pub fn reset_sound(&mut self) {
        let config = self.sb.as_ref().map(|sb| sb.config);
        let opl3 = self.opl.is_opl3();
        let pnp = self.isapnp.take();
        self.configure_sound(config, opl3);
        if self.isapnp.is_some() && pnp.is_some() {
            self.isapnp = pnp;
            self.route_isapnp_audio();
        }
        self.dma = crate::dma::Dma::new();
        self.mpu.reset();
        self.tandy_sound = crate::sn76489::Sn76489::new(self.tandy_sound.variant());
        self.cdaudio.reset();
        self.sb_frame = (0, 0);
        let hooked = self.gus.as_ref().and_then(|gus| gus.irq()).is_some_and(|irq| {
            let vector = if irq < 8 { 0x08 + irq as usize } else { 0x70 + irq as usize - 8 };
            let entry = (self.read_16(vector * 4 + 2) as u32) << 16 | self.read_16(vector * 4) as u32;
            entry != crate::bios::default_ivt()[vector]
        });
        if let Some(gus) = &mut self.gus {
            if hooked { gus.silence() } else { gus.power_on() }
        }
        // The AWE32 keeps its sample RAM and settings, which drivers
        // loaded; its voices stop.
        if let Some(awe) = &mut self.awe {
            awe.silence();
        }
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
    }

    /// Power-on state of the sound hardware, at a reboot: as at a
    /// program's end, with a ViBRA16C back at the resources the BIOS gives
    /// it.
    pub fn power_on_sound(&mut self) {
        self.isapnp = None;
        self.reset_sound();
    }

    /// The AWE32's ROM and RAM size (KB), for the card from now on. A card
    /// there is takes the ROM at once; a new RAM size makes a new card.
    pub fn set_awe_setup(&mut self, rom: std::sync::Arc<[i16]>, ram_kb: u32) {
        if let Some(awe) = &mut self.awe {
            awe.set_rom(rom.clone());
            if awe.ram_kb() != ram_kb.min(crate::awe32::MAX_RAM_KB) {
                self.awe = None;
            }
        }
        self.awe_rom = rom;
        self.awe_ram_kb = ram_kb;
        self.configure_awe();
    }

    /// Whether the AWE32 has its ROM.
    pub fn awe_has_rom(&self) -> bool {
        !self.awe_rom.is_empty()
    }

    /// The EMU8000 the Sound Blaster has: kept while the card stays an
    /// AWE32 at the same base, made when it becomes one.
    fn configure_awe(&mut self) {
        let base = self.sb.as_ref().filter(|sb| sb.config.model == crate::sb::SbModel::Awe32).map(|sb| sb.config.base);
        match base {
            None => self.awe = None,
            Some(base) if self.awe.as_ref().is_some_and(|awe| awe.base == base.wrapping_add(0x400)) => {}
            Some(base) => {
                let now = self.awe_frames();
                self.awe = Some(Box::new(crate::awe32::Emu8000::new(base, self.awe_rom.clone(), self.awe_ram_kb, now)));
            }
        }
    }

    /// Initialise the EMU8000 as `AWEUTIL /S` does.
    pub fn awe_init(&mut self) {
        self.audio_catch_up();
        let now = self.awe_frames();
        if let Some(awe) = &mut self.awe {
            awe.power_on(now);
        }
    }

    /// Apply the code fixes (`game_fixes`) whose code is in conventional
    /// memory, as a program resets the DSP.
    pub(super) fn apply_code_fixes(&mut self) {
        let end = self.ram.len().min(0xA0000);
        let found =
            crate::game_fixes::find_code(&self.ram[..end], |fix| self.game_fix_enabled(fix));
        for (fix, code, base) in found {
            for (addr, byte) in crate::game_fixes::writes(code, base) {
                self.write_8(addr, byte);
            }
            self.log_string(&format!(
                "[game fix] {}: {} ({}): code at {:05X} patched",
                fix.name, fix.game, fix.reason, base
            ));
        }
    }

    /// Whether the `game_fixes` settings let `fix` apply.
    pub fn game_fix_enabled(&self, fix: &crate::game_fixes::Fix) -> bool {
        crate::game_fixes::enabled(fix, self.game_fixes, &self.game_fixes_off)
    }

    /// The Sound Blaster's resources now, as a BLASTER string: the base,
    /// IRQ and DMA channels a Plug and Play configuration or the SB16's
    /// mixer (80h and 81h) moved it to, with a line it has none of left
    /// out. None without a card, or while a ViBRA16C's audio device is
    /// inactive.
    pub fn sb_blaster(&self) -> Option<String> {
        let sb = self.sb.as_ref()?;
        let config = sb.config;
        let base = self.sb_base()?;
        let (dma8, dma16) = sb.dma();
        let mut s = format!("A{:X}", base);
        if let Some(irq) = sb.irq {
            s.push_str(&format!(" I{}", irq));
        }
        if let Some(dma) = dma8 {
            s.push_str(&format!(" D{}", dma));
        }
        if config.model.is_sb16() {
            if let Some(dma) = dma16 {
                s.push_str(&format!(" H{}", dma));
            }
            s.push_str(" P330");
        }
        if config.model == crate::sb::SbModel::Awe32 {
            s.push_str(&format!(" E{:X}", base + 0x400));
        }
        s.push_str(&format!(" T{}", config.model.blaster_type()));
        Some(s)
    }

    /// Give the Sound Blaster the IRQ and DMA channels the ViBRA16C's audio
    /// device has: none while it is inactive.
    pub(super) fn route_isapnp_audio(&mut self) {
        let Some(pnp) = &self.isapnp else { return };
        let audio = pnp.audio();
        if let Some(sb) = &mut self.sb {
            match audio {
                Some(audio) => sb.set_lines(audio.irq, audio.dma8, audio.dma16),
                None => sb.set_lines(None, None, None),
            }
        }
        self.sync_sb_irq();
    }
}
