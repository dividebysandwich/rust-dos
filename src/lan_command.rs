//! The LAN command: joins this instance to a LAN of rust-dos instances
//! through a relay, which it can host itself, so games play over IPX as
//! on one network:
//!
//! ```text
//! LAN HOST [port] [/ROOM:name] [/PASSWORD:text]
//! LAN JOIN [host[:port] | /LOCAL] [/ROOM:name] [/PASSWORD:text]
//! LAN LIST [host[:port] | /LOCAL] [/ROOM:text]
//! LAN LEAVE | DISBAND | STOP | STATUS
//! ```
//!
//! JOIN and LIST go to the relay `relay` in `[network]` names (the public
//! one unless set) without an address, and to the first relay that
//! answers on this network with /LOCAL. HOST and JOIN install the IPX
//! driver (with `ipx=auto`), and wait a few seconds for the room to be
//! joined, as LIST does for the rooms, which a key cuts short.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::{print_cp437, print_string};

const HELP: &str = "Joins rust-dos instances into a LAN, for games that play over IPX.\r\n\
\r\n\
LAN HOST [port] [/ROOM:name] [/PASSWORD:text]\r\n\
LAN JOIN [host[:port] | /LOCAL] [/ROOM:name] [/PASSWORD:text]\r\n\
LAN LIST [host[:port] | /LOCAL] [/ROOM:text]\r\n\
LAN LEAVE | DISBAND | STOP | STATUS\r\n\
\r\n\
  HOST     Relays rooms for others on a UDP port (21213 unless given), and\r\n\
           joins one.\r\n\
  JOIN     Joins a room at a relay, making it if it isn't there.\r\n\
  LIST     Lists a relay's rooms, those with text in their names with /ROOM.\r\n\
  LEAVE    Leaves the room; the one there longest hosts it next.\r\n\
  DISBAND  Ends the room for everyone in it, as its host (who made it).\r\n\
  STOP     Stops relaying.\r\n\
  STATUS   Shows the IPX driver and the LAN (LAN alone does too).\r\n\
  host     The relay: relay in [network] unless given (relay.rust-dos.com).\r\n\
  /LOCAL   The first relay that answers on this network.\r\n\
  /ROOM    The room, \"lobby\" unless given or set in [network]; in quotes\r\n\
           for a name with spaces.\r\n\
  /PASSWORD  The room's password, which the one who makes it gives it.\r\n\
\r\n\
Over the internet, the host's UDP port has to reach it, or everyone joins\r\n\
a relay on a server (rust-dos-relay). Nothing crossing a relay is\r\n\
encrypted.\r\n";

/// How long HOST and JOIN wait for the room, and LIST for the rooms, in
/// PIT ticks.
const WAIT_TICKS: u64 = 6 * crate::timer::PIT_HZ;

pub struct LanCommand;

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    verb: String,
    target: Option<String>,
    room: Option<String>,
    password: Option<String>,
    local: bool,
}

/// The words of `args`, split at spaces outside double quotes, which go.
fn words(args: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut word, mut quoted, mut any) = (String::new(), false, false);
    for c in args.chars() {
        match c {
            '"' => (quoted, any) = (!quoted, true),
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => (word, any) = (word + c.encode_utf8(&mut [0; 4]), true),
        }
    }
    if any {
        words.push(word);
    }
    words
}

fn parse(args: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    for word in words(args) {
        if let Some(switch) = word.strip_prefix('/') {
            let (name, value) = switch.split_once(':').unwrap_or((switch, ""));
            match name.to_ascii_uppercase().as_str() {
                "ROOM" if !value.is_empty() => parsed.room = Some(value.to_string()),
                "PASSWORD" | "PW" => parsed.password = Some(value.to_string()),
                "LOCAL" if value.is_empty() => parsed.local = true,
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
    if parsed.local && parsed.target.is_some() {
        return Err("/LOCAL looks for a relay: it takes no address".into());
    }
    Ok(parsed)
}

/// `text`, which may have a room's name in it, in the characters of the
/// screen, and a new line.
fn print_line(cpu: &mut Cpu, text: &str) {
    let bytes: Vec<u8> = text.chars().map(crate::config_ui::cp437).collect();
    print_cp437(cpu, &bytes, 0x07);
    print_string(cpu, "\r\n");
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
        // The relay: the one given, the first on this network, or the
        // one the settings name.
        let relay = match (&parsed.target, parsed.local) {
            (Some(target), _) => Some(target.clone()),
            (None, true) => None,
            (None, false) => settings.relay.clone(),
        };
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
                if !crate::net::tunnel::wire::valid_room(&room) {
                    return error(cpu, "a room's name is 1 to 32 printable characters");
                }
                cpu.bus.install_ipx();
                cpu.bus.net.join(relay.as_deref(), &room, &password)
            }
            "LIST" => {
                let filter = parsed.room.clone().unwrap_or_default();
                match cpu.bus.net.browse(relay.as_deref(), &filter) {
                    Ok(()) => {
                        let at = relay.as_deref().unwrap_or("the first relay on this network");
                        print_string(cpu, &format!("Asking {} for its rooms...\r\n", at));
                        let until = cpu.bus.clock.now_ticks() + WAIT_TICKS;
                        crate::shell::enter_wait(cpu, crate::shell::ShellWait::LanList { until });
                    }
                    Err(e) => error(cpu, &e),
                }
                return;
            }
            "LEAVE" => {
                cpu.bus.net.leave();
                return;
            }
            "DISBAND" => {
                let lan = cpu.bus.net.status();
                match (lan.roster(), lan.joined()) {
                    (Some((index, roster)), Some((_, room))) if roster.host == index => {
                        cpu.bus.net.disband();
                        print_line(cpu, &format!("Ended room \"{}\" for everyone in it", room));
                    }
                    (Some(_), _) => error(cpu, "only the room's host can end it (LAN LEAVE leaves it)"),
                    _ => error(cpu, "not in a room"),
                }
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
                if parsed.verb == "JOIN" && relay.is_none() {
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

/// Whether LIST is done waiting at PIT tick `until`: the rooms came, or
/// can't.
pub fn list_over(cpu: &Cpu, until: u64) -> bool {
    !cpu.bus.net.listing().asking || cpu.bus.clock.now_ticks() >= until
}

/// LIST stopped waiting: the rooms, or why there are none.
pub fn list_ended(cpu: &mut Cpu) {
    let listing = cpu.bus.net.listing();
    let list = match listing.result {
        _ if listing.asking => return print_string(cpu, "No answer yet from the relay\r\n"),
        None => return,
        Some(Err(e)) => return error(cpu, &e),
        Some(Ok(list)) => list,
    };
    let locked = if list.password { ", every room with its password" } else { "" };
    let name = if list.name.is_empty() { String::new() } else { format!(" (\"{}\"{})", list.name, locked) };
    print_line(cpu, &format!("Rooms at {}{}:", list.relay, name));
    if !list.rooms.is_empty() {
        print_line(cpu, &format!("  {:<32} {:>7}", "Room", "Players"));
    }
    for room in &list.rooms {
        let lock = if room.password && !list.password { "  password" } else { "" };
        print_line(cpu, &format!("  {:<32} {:>7}{}", room.name, room.members, lock));
    }
    let shown = match list.rooms.len() {
        0 => "No rooms yet".to_string(),
        n if n < list.total => format!("{} rooms of {} (/ROOM:text finds the others)", n, list.total),
        1 => "1 room".to_string(),
        n => format!("{} rooms", n),
    };
    print_string(cpu, &format!("{}; LAN JOIN /ROOM:name joins one, or makes it\r\n", shown));
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
    let lan = cpu.bus.net.status();
    print_line(cpu, &format!("LAN: {}", lan.describe()));
    if let Some((index, roster)) = lan.roster() {
        print_string(cpu, "Players:\r\n");
        for member in &roster.members {
            let role = match (member.index == roster.host, member.index == index) {
                (true, true) => "host, you",
                (true, false) => "host",
                (false, true) => "you",
                (false, false) => "",
            };
            print_line(cpu, &format!("  {:>3}  {:<32} {}", member.index, member.shown(), role));
        }
        if (roster.total as usize) > roster.members.len() {
            print_line(cpu, &format!("       and {} more", roster.total as usize - roster.members.len()));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let relay = cpu.bus.net.settings.relay.as_deref().unwrap_or("the first on this network").to_string();
        print_string(cpu, &format!("Relay: {} (without an address)\r\n", relay));
        let Some(status) = lan.hub else { return };
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
                local: false,
            }
        );
        assert_eq!(parse("HOST 2000").unwrap().target.as_deref(), Some("2000"));
        assert_eq!(parse("/?").unwrap().verb, "HELP");
        assert!(parse("join a b").is_err());
        assert!(parse("join /speed:fast").is_err());
        // Names with spaces in quotes, and looking on this network.
        let parsed = parse(r#"list /local /room:"doom 2  dm" /pw:"#).unwrap();
        assert_eq!((parsed.verb.as_str(), parsed.room.as_deref(), parsed.local), ("LIST", Some("doom 2  dm"), true));
        assert_eq!(parsed.password.as_deref(), Some(""));
        assert_eq!(parse(r#"join "/ROOM:a b""#).unwrap().room.as_deref(), Some("a b"));
        assert!(parse("join 192.0.2.1 /local").is_err());
    }
}
