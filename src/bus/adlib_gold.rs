//! The AdLib Gold on the bus: its ports, its MMA's time, DMA and
//! interrupt line (`crates/adlibgold`).

use super::*;

impl Bus {
    /// Install (or with None remove) the AdLib Gold. A card already there
    /// with the same resources keeps its EEPROM, as the real one keeps it
    /// with the power off.
    pub fn configure_adlib_gold(&mut self, config: Option<crate::adlibgold::AdlibGoldConfig>) {
        self.audio_catch_up();
        let now = self.clock.now_ticks();
        let eeprom = self
            .adlib_gold
            .as_ref()
            .filter(|g| Some(g.config()) == config)
            .map(|g| g.eeprom());
        self.adlib_gold =
            config.map(|c| Box::new(crate::adlibgold::AdlibGold::new(c, eeprom, now)));
        self.sync_adlib_gold_irq();
        self.clock.schedule(self.next_event());
    }

    /// The card at power-on, its EEPROM kept.
    pub(super) fn reset_adlib_gold(&mut self) {
        let config = self.adlib_gold.as_ref().map(|g| g.config());
        self.configure_adlib_gold(config);
    }

    /// Whether `port` is one of the AdLib Gold's.
    #[inline]
    pub(super) fn adlib_gold_claims(&self, port: u16) -> bool {
        self.adlib_gold.as_ref().is_some_and(|g| g.claims(port))
    }

    /// Run the AdLib Gold's MMA up to the present and update its
    /// interrupt line.
    pub(super) fn adlib_gold_advance(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(gold) = &mut self.adlib_gold {
            gold.advance(now, &mut self.dma, &self.ram);
            self.sync_adlib_gold_irq();
        }
    }

    pub(super) fn adlib_gold_next_event(&self) -> Option<u64> {
        self.adlib_gold
            .as_ref()
            .and_then(|g| g.next_event(&self.dma))
    }

    /// Drive the card's interrupt line: raise the IRQ as a request comes
    /// up, withdraw it as it goes.
    pub(super) fn sync_adlib_gold_irq(&mut self) {
        let line = self.adlib_gold.as_ref().and_then(|g| g.irq());
        if let Some(old) = self.adlib_gold_line
            && line != Some(old)
        {
            self.withdraw_irq(old);
        }
        if let Some(irq) = line
            && self.adlib_gold_line != line
        {
            self.pic.raise(irq);
        }
        self.adlib_gold_line = line;
        self.refresh_irq();
    }

    pub(super) fn adlib_gold_write(&mut self, port: u16, value: u8) {
        let Some(gold) = &mut self.adlib_gold else {
            return;
        };
        match port.wrapping_sub(gold.base()) {
            0 => self.opl.write_address(0, value),
            1 => self.opl_data(0, value),
            2 => {
                if !gold.write_control_address(value) {
                    self.opl.write_address(1, value);
                }
            }
            3 => {
                if !gold.control_selected() {
                    self.opl_data(1, value);
                    return;
                }
                // The mixer and output stage change from now on.
                self.audio_catch_up();
                if let Some(gold) = &mut self.adlib_gold {
                    gold.write_control_data(value);
                }
                // Register 13h or 14h may have moved the interrupt or the
                // DMA.
                self.adlib_gold_advance();
                self.clock.schedule(self.next_event());
            }
            offset => {
                self.audio_catch_up();
                let now = self.clock.now_ticks();
                if let Some(gold) = &mut self.adlib_gold {
                    gold.write_mma(offset, value, now, &mut self.dma, &self.ram);
                }
                self.sync_adlib_gold_irq();
                self.clock.schedule(self.next_event());
            }
        }
    }

    pub(super) fn adlib_gold_read(&mut self, port: u16) -> u8 {
        let Some(gold) = &mut self.adlib_gold else {
            return 0xFF;
        };
        match port.wrapping_sub(gold.base()) {
            0 => self.opl.read_status(self.clock.now_micros()),
            1 => 0xFF,
            2 => {
                if gold.control_selected() {
                    // The FM's interrupt: the OPL3's status bit 7.
                    let fm = self.opl.read_status(self.clock.now_micros()) & 0x80 != 0;
                    self.adlib_gold_advance();
                    self.adlib_gold
                        .as_ref()
                        .and_then(|g| g.read_control_status(fm))
                        .unwrap_or(0xFF)
                } else {
                    self.opl.read_status(self.clock.now_micros())
                }
            }
            3 => gold.read_control_data().unwrap_or(0xFF),
            offset => {
                let now = self.clock.now_ticks();
                let value = gold.read_mma(offset, now, &mut self.dma, &self.ram);
                self.sync_adlib_gold_irq();
                self.clock.schedule(self.next_event());
                value
            }
        }
    }
}
