//! The LAN relay on its own, for a server without a display: rust-dos
//! instances anywhere join its rooms (`LAN JOIN host`) and play over it as
//! over one Ethernet. It needs neither SDL nor a sound library.

use clap::Parser;
use rust_dos::net::tunnel::relay::{self, RelayConfig};
use rust_dos::net::tunnel::wire::DEFAULT_PORT;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Parser, Debug)]
#[command(author, version, about = "Relays rust-dos LAN rooms over UDP", long_about = None)]
struct Args {
    /// The UDP port to relay on
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// The address to relay on [default: all IPv4 addresses]
    #[arg(short, long, value_name = "ADDR")]
    bind: Option<IpAddr>,

    /// The password members need to join a room
    #[arg(long)]
    password: Option<String>,

    /// The relay's name, as LAN JOIN lists it
    #[arg(long, default_value = "rust-dos relay")]
    name: String,
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let bind = SocketAddr::new(args.bind.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), args.port);
    relay::serve(bind, RelayConfig { name: args.name, password: args.password, port: args.port })
}
