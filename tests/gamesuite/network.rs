//! Machines linked as two PCs on one network, or by a cable or a phone
//! line: a relay on this machine with a LAN room both join, as players
//! do with LAN HOST and LAN JOIN.

use super::machine::Rig;
use rust_dos::net::hub::LanState;
use rust_dos::net::tunnel::relay::{RelayConfig, RelayServer};
use std::time::{Duration, Instant};

/// Put the rig's machines in one LAN room on a relay of their own, which
/// lasts as long as the value returned.
pub fn link(rig: &mut Rig) -> Result<RelayServer, String> {
    let relay = RelayServer::start("127.0.0.1:0".parse().unwrap(), RelayConfig::default(), Box::new(|_| {}))
        .map_err(|e| format!("relay: {}", e))?;
    let at = relay.local_addr().to_string();
    for m in &mut rig.machines {
        m.cpu.bus.net.join(Some(&at), "game-suite", "").map_err(|e| format!("joining the room: {}", e))?;
    }
    let joined = |rig: &Rig| {
        rig.machines
            .iter()
            .all(|m| matches!(m.cpu.bus.net.status().hub.map(|h| h.lan), Some(LanState::Joined { .. })))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !joined(rig) {
        if Instant::now() > deadline {
            return Err("the machines didn't get into the LAN room".into());
        }
        rig.run(5);
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(relay)
}
