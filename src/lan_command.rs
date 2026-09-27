//! The LAN command: joins this instance to a LAN of rust-dos instances
//! through a relay, which it can host itself, so games play over IPX as
//! on one network:
//!
//! ```text
//! LAN HOST [port] [/ROOM:name] [/PASSWORD:text]
//! LAN JOIN [host[:port]] [/ROOM:name] [/PASSWORD:text]
//! LAN LEAVE | STOP | STATUS
//! ```
//!
//! HOST and JOIN install the IPX driver (with `ipx=auto`), and wait a few
//! seconds for the room to be joined, which a key cuts short.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::{print_cp437, print_string};

const HELP: &str = "Joins rust-dos instances into a LAN, for games that play over IPX.\r\n\
\r\n\
LAN HOST [port] [/ROOM:name] [/PASSWORD:text]\r\n\
LAN JOIN [host[:port]] [/ROOM:name] [/PASSWORD:text]\r\n\
LAN LEAVE | STOP | STATUS\r\n\
\r\n\
  HOST     Relays rooms for others on a UDP port (21213 unless given), and\r\n\
           joins one.\r\n\
  JOIN     Joins a room at a relay: the one at host, or the first that\r\n\
           answers on this network.\r\n\
  LEAVE    Leaves the room.\r\n\
  STOP     Stops relaying.\r\n\
  STATUS   Shows the IPX driver and the LAN (LAN alone does too).\r\n\
  /ROOM    The room, \"lobby\" unless given or set in [network].\r\n\
  /PASSWORD  The password the relay's rooms need.\r\n\
\r\n\
Over the internet, the host's UDP port has to reach it, or everyone joins\r\n\
a relay on a server (rust-dos-relay). Nothing crossing a relay is\r\n\
encrypted.\r\n";

/// How long HOST and JOIN wait for the room, in PIT ticks.
const WAIT_TICKS: u64 = 6 * crate::timer::PIT_HZ;

pub struct LanCommand;

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    verb: String,
    target: Option<String>,
    room: Option<String>,
    password: Option<String>,
}

fn parse(args: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    for word in args.split_whitespace() {
        if let Some(switch) = word.strip_prefix('/') {
            let (name, value) = switch.split_once(':').unwrap_or((switch, ""));
            match name.to_ascii_uppercase().as_str() {
                "ROOM" if !value.is_empty() => parsed.room = Some(value.to_string()),
                "PASSWORD" | "PW" => parsed.password = Some(value.to_string()),
                "?" => parsed.verb = "HELP".into(),
                _ => return Err(format!("Invalid switch - {}", word)),
            }
        } else if parsed.verb.is_empty() {
            parsed.verb = word.to_ascii_uppercase();
        } else if parsed.target.is_none() {
            parsed.target = Some(word.to_string());
        } else {
            return Err(format!("Too many parameters - {}", word));
        }
    }
    Ok(parsed)
}

fn error(cpu: &mut Cpu, text: &str) {
    print_cp437(cpu, format!("LAN: {}", text).as_bytes(), 0x0C);
    print_string(cpu, "\r\n");
}

impl ShellCommand for LanCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let parsed = match parse(args) {
            Ok(parsed) => parsed,
            Err(e) => return error(cpu, &e),
        };
        let settings = cpu.bus.net.settings.clone();
        let room = parsed.room.clone().unwrap_or(settings.room.clone());
        let password = parsed.password.clone().unwrap_or(settings.password.clone());
        if room.len() > crate::net::tunnel::wire::MAX_NAME {
            return error(cpu, "the room's name is too long");
        }
        let started = match parsed.verb.as_str() {
            "" | "STATUS" => return show(cpu),
            "HELP" => return print_string(cpu, HELP),
            "HOST" => {
                let port = match parsed.target.as_deref().map(str::parse::<u16>) {
                    None => crate::net::tunnel::wire::DEFAULT_PORT,
                    Some(Ok(port)) => port,
                    Some(Err(_)) => return error(cpu, "the port is a number from 0 to 65535"),
                };
                cpu.bus.install_ipx();
                cpu.bus.net.host(port, &room, &password)
            }
            "JOIN" => {
                cpu.bus.install_ipx();
                cpu.bus.net.join(parsed.target.as_deref(), &room, &password)
            }
            "LEAVE" => {
                cpu.bus.net.leave();
                return;
            }
            "STOP" => {
                cpu.bus.net.stop_hosting();
                return;
            }
            verb => return error(cpu, &format!("unknown command {} (LAN /? shows them)", verb)),
        };
        match started {
            Err(e) => error(cpu, &e),
            Ok(()) => {
                if parsed.verb == "JOIN" && parsed.target.is_none() {
                    print_string(cpu, "Looking for a relay on this network...\r\n");
                }
                let until = cpu.bus.clock.now_ticks() + WAIT_TICKS;
                crate::shell::enter_wait(cpu, crate::shell::ShellWait::Lan { until });
            }
        }
    }
}

/// Whether HOST or JOIN is done waiting at PIT tick `until`: the room is
/// joined, or can't be.
pub fn wait_over(cpu: &Cpu, until: u64) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::net::hub::LanState;
        let lan = cpu.bus.net.status().hub.map(|h| h.lan);
        if matches!(lan, None | Some(LanState::Joined { .. } | LanState::Failed(_) | LanState::Off)) {
            return true;
        }
    }
    cpu.bus.clock.now_ticks() >= until
}

/// HOST or JOIN stopped waiting, for `key` (0: the room, or the time).
pub fn wait_ended(cpu: &mut Cpu, key: u8) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::net::hub::LanState;
        match cpu.bus.net.status().hub.map(|h| (h.lan, h.room)) {
            Some((LanState::Joined { relay, index, members, .. }, room)) => {
                let others = match members.saturating_sub(1) {
                    0 => "no one else there yet".to_string(),
                    1 => "1 other there".to_string(),
                    n => format!("{} others there", n),
                };
                print_string(
                    cpu,
                    &format!("Joined room \"{}\" at {} as member {}, {}\r\n", room, relay, index, others),
                );
                return;
            }
            Some((LanState::Failed(e), _)) => return error(cpu, &e),
            _ => {}
        }
    }
    if key == 0 {
        print_string(cpu, "No answer yet: joining goes on (LAN STATUS tells how it went)\r\n");
    } else {
        print_string(cpu, "Joining goes on (LAN STATUS tells how it went)\r\n");
    }
}

/// LAN STATUS: the IPX driver, the room and the relay hosted.
fn show(cpu: &mut Cpu) {
    let ipx = match &cpu.bus.net.ipx {
        Some(ipx) => format!(
            "IPX driver: node {}, IRQ {}, {} frames, {} socket{} open\r\n",
            ipx.node,
            ipx.irq,
            ipx.frame_type.name(),
            ipx.sockets.len(),
            if ipx.sockets.len() == 1 { "" } else { "s" }
        ),
        None => format!(
            "IPX driver: not installed ({})\r\n",
            match cpu.bus.net.settings.ipx {
                crate::net::IpxMode::Off => "ipx=false",
                _ => "LAN HOST or LAN JOIN installs it",
            }
        ),
    };
    print_string(cpu, &ipx);
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::net::hub::LanState;
        let Some(status) = cpu.bus.net.status().hub else {
            return print_string(cpu, "LAN: not joined\r\n");
        };
        let lan = match &status.lan {
            LanState::Off => "not joined".to_string(),
            LanState::Looking => format!("looking for the relay of room \"{}\"", status.room),
            LanState::Joining { relay } => format!("joining room \"{}\" at {}", status.room, relay),
            LanState::Rejoining { relay } => format!("joining room \"{}\" at {} again", status.room, relay),
            LanState::Failed(e) => format!("not joined: {}", e),
            LanState::Joined { relay, index, members, rtt_ms } => format!(
                "room \"{}\" at {}, member {} of {}{}",
                status.room,
                relay,
                index,
                members,
                rtt_ms.map_or(String::new(), |ms| format!(", {} ms to the relay", ms))
            ),
        };
        print_string(cpu, &format!("LAN: {}\r\n", lan));
        if status.frames_out + status.frames_in > 0 {
            print_string(cpu, &format!("     {} frames sent, {} received\r\n", status.frames_out, status.frames_in));
        }
        if let Some(hosting) = status.hosting {
            let rooms: Vec<String> =
                status.hosted_rooms.iter().map(|r| format!("{} ({})", r.name, r.members)).collect();
            let rooms = if rooms.is_empty() { "none yet".to_string() } else { rooms.join(", ") };
            print_string(cpu, &format!("Relaying on UDP port {}: rooms {}\r\n", hosting.port(), rooms));
        }
    }
    #[cfg(target_arch = "wasm32")]
    print_string(cpu, "LAN: the browser version of rust-dos has no network\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_lines() {
        assert_eq!(parse("").unwrap(), Parsed::default());
        assert_eq!(
            parse("join relay.example.com:4000 /room:doom /password:x").unwrap(),
            Parsed {
                verb: "JOIN".into(),
                target: Some("relay.example.com:4000".into()),
                room: Some("doom".into()),
                password: Some("x".into()),
            }
        );
        assert_eq!(parse("HOST 2000").unwrap().target.as_deref(), Some("2000"));
        assert_eq!(parse("/?").unwrap().verb, "HELP");
        assert!(parse("join a b").is_err());
        assert!(parse("join /speed:fast").is_err());
    }
}
