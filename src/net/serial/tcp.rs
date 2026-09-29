//! A modem's calls over TCP, on the network thread: a call dialed to
//! `host:port`, and calls taken on a port, a task each, which report what
//! happens through the network thread's channel. What goes through is the
//! characters as they are, as DOSBox's modem has them, or with telnet's
//! negotiation answered and taken out (`Telnet`).

use crate::net::hub::Command;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel};

/// How long a call may take to go through.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// What happened to a call.
#[derive(Debug)]
pub enum TcpEvent {
    Connected { id: u64 },
    Failed { id: u64, reason: String },
    Data { id: u64, data: Vec<u8> },
    Closed { id: u64 },
    /// A call came in on the port taken calls on; `tx` sends to it.
    Incoming { id: u64, from: SocketAddr, tx: UnboundedSender<Vec<u8>> },
}

fn tell(events: &WeakUnboundedSender<Command>, event: TcpEvent) {
    if let Some(tx) = events.upgrade() {
        let _ = tx.send(Command::Serial(super::SerialCommand::Tcp(event)));
    }
}

/// Call `address` (`host:port`); what to send comes through the channel
/// returned, and the call ends when it goes.
pub fn dial(id: u64, address: String, events: WeakUnboundedSender<Command>) -> UnboundedSender<Vec<u8>> {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(async move {
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address.as_str())).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return tell(&events, TcpEvent::Failed { id, reason: e.to_string() }),
            Err(_) => return tell(&events, TcpEvent::Failed { id, reason: "no answer".into() }),
        };
        tell(&events, TcpEvent::Connected { id });
        pump(id, stream, rx, events).await;
    });
    tx
}

/// Take calls on TCP port `port`, numbering them from `first_id`. Ends
/// with the task (`JoinHandle::abort`).
pub async fn listen(port: u16, first_id: u64, events: WeakUnboundedSender<Command>) -> std::io::Result<()> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
    let mut id = first_id;
    loop {
        let (stream, from) = listener.accept().await?;
        id += 1;
        let (tx, rx) = unbounded_channel();
        tell(&events, TcpEvent::Incoming { id, from, tx });
        tokio::spawn(pump(id, stream, rx, events.clone()));
    }
}

/// Carry a call's characters both ways until either end closes it.
async fn pump(id: u64, stream: TcpStream, mut rx: UnboundedReceiver<Vec<u8>>, events: WeakUnboundedSender<Command>) {
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            data = rx.recv() => match data {
                Some(data) => {
                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = writer.shutdown().await;
                    return;
                }
            },
            read = reader.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => tell(&events, TcpEvent::Data { id, data: buf[..n].to_vec() }),
            },
        }
    }
    tell(&events, TcpEvent::Closed { id });
}

/// Telnet on a call: IAC (FFh) sequences taken out of what comes, options
/// answered (binary, suppress go-ahead and the other end's echo are
/// fine, nothing else), and FFh doubled in what goes.
#[derive(Default)]
pub struct Telnet {
    state: TelnetState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TelnetState {
    #[default]
    Data,
    Iac,
    Option(u8),
    Sub,
    SubIac,
}

const IAC: u8 = 0xFF;
const WILL: u8 = 0xFB;
const WONT: u8 = 0xFC;
const DO: u8 = 0xFD;
const DONT: u8 = 0xFE;
const SB: u8 = 0xFA;
const SE: u8 = 0xF0;
const BINARY: u8 = 0;
const ECHO: u8 = 1;
const SGA: u8 = 3;

impl Telnet {
    /// What came: the characters in it, and the answers to send back.
    pub fn incoming(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut replies) = (Vec::new(), Vec::new());
        for &b in bytes {
            self.state = match (self.state, b) {
                (TelnetState::Data, IAC) => TelnetState::Iac,
                (TelnetState::Data, _) => {
                    data.push(b);
                    TelnetState::Data
                }
                (TelnetState::Iac, IAC) => {
                    data.push(IAC);
                    TelnetState::Data
                }
                (TelnetState::Iac, WILL | WONT | DO | DONT) => TelnetState::Option(b),
                (TelnetState::Iac, SB) => TelnetState::Sub,
                (TelnetState::Iac, _) => TelnetState::Data,
                (TelnetState::Option(command), option) => {
                    let answer = match command {
                        DO if matches!(option, BINARY | SGA) => Some(WILL),
                        DO => Some(WONT),
                        WILL if matches!(option, BINARY | SGA | ECHO) => Some(DO),
                        WILL => Some(DONT),
                        _ => None,
                    };
                    if let Some(answer) = answer {
                        replies.extend([IAC, answer, option]);
                    }
                    TelnetState::Data
                }
                (TelnetState::Sub, IAC) => TelnetState::SubIac,
                (TelnetState::Sub, _) => TelnetState::Sub,
                (TelnetState::SubIac, SE) => TelnetState::Data,
                (TelnetState::SubIac, _) => TelnetState::Sub,
            };
        }
        (data, replies)
    }

    /// What goes, with FFh doubled.
    pub fn outgoing(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            out.push(b);
            if b == IAC {
                out.push(IAC);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telnet_negotiation_is_answered_and_taken_out() {
        let mut telnet = Telnet::default();
        let (data, replies) = telnet.incoming(&[b'h', IAC, DO, SGA, IAC, WILL, ECHO, b'i', IAC, IAC, IAC, DO, 24]);
        assert_eq!(data, [b'h', b'i', IAC]);
        assert_eq!(replies, [IAC, WILL, SGA, IAC, DO, ECHO, IAC, WONT, 24]);
        // Split across reads, and a subnegotiation.
        let (data, _) = telnet.incoming(&[IAC]);
        assert!(data.is_empty());
        let (data, _) = telnet.incoming(&[SB, 24, 1, IAC, SE, b'!']);
        assert_eq!(data, b"!");
        assert_eq!(Telnet::outgoing(&[1, IAC, 2]), [1, IAC, IAC, 2]);
    }
}
