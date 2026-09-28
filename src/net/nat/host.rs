//! The router's actions carried out on the host, on the network thread:
//! TCP connections (a task each, which reads only as far ahead of the
//! guest as its credit goes), UDP sockets, and name lookups. What happens
//! goes back to the router as events through the network thread's channel.

use super::{Action, Event, TCP_CREDIT};
use crate::net::hub::Command;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

/// How long a connection may take to be made.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

enum ToConnection {
    Send(Vec<u8>),
    Shutdown,
    Credit(usize),
}

pub struct NatHost {
    events: WeakUnboundedSender<Command>,
    /// Each connection's task, which ends when its sender goes.
    tcp: HashMap<u64, UnboundedSender<ToConnection>>,
    udp: HashMap<u64, (Arc<UdpSocket>, JoinHandle<()>)>,
}

impl NatHost {
    pub fn new(events: WeakUnboundedSender<Command>) -> Self {
        Self { events, tcp: HashMap::new(), udp: HashMap::new() }
    }

    /// Carry out `action` (anything but a frame for the guest).
    pub fn run(&mut self, action: Action) {
        match action {
            Action::ToGuest(_) => {}
            Action::Connect { flow, to } => {
                let (tx, rx) = unbounded_channel();
                self.tcp.insert(flow, tx);
                tokio::spawn(connection(flow, to, rx, self.events.clone()));
            }
            Action::Send { flow, data } => self.tell_connection(flow, ToConnection::Send(data)),
            Action::Shutdown { flow } => self.tell_connection(flow, ToConnection::Shutdown),
            Action::Credit { flow, bytes } => self.tell_connection(flow, ToConnection::Credit(bytes)),
            Action::Close { flow } => {
                self.tcp.remove(&flow);
            }
            Action::UdpOpen { id } => {
                let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                    .and_then(|s| s.set_nonblocking(true).map(|_| s))
                    .and_then(UdpSocket::from_std);
                if let Ok(socket) = socket {
                    let socket = Arc::new(socket);
                    let task = tokio::spawn(datagrams(id, socket.clone(), self.events.clone()));
                    self.udp.insert(id, (socket, task));
                }
            }
            Action::UdpSend { id, to, data } => {
                if let Some((socket, _)) = self.udp.get(&id) {
                    let _ = socket.try_send_to(&data, to);
                }
            }
            Action::UdpClose { id } => {
                if let Some((_, task)) = self.udp.remove(&id) {
                    task.abort();
                }
            }
            Action::Resolve { token, name } => {
                let events = self.events.clone();
                tokio::spawn(async move {
                    let (addresses, found) = match tokio::net::lookup_host((name.as_str(), 0)).await {
                        Ok(addrs) => {
                            let mut v4: Vec<Ipv4Addr> = addrs
                                .filter_map(|a| match a {
                                    SocketAddr::V4(a) => Some(*a.ip()),
                                    SocketAddr::V6(_) => None,
                                })
                                .collect();
                            v4.dedup();
                            (v4, true)
                        }
                        Err(_) => (Vec::new(), false),
                    };
                    tell(&events, Event::Resolved { token, addresses, found });
                });
            }
        }
    }

    fn tell_connection(&mut self, flow: u64, message: ToConnection) {
        if let Some(tx) = self.tcp.get(&flow) {
            let _ = tx.send(message);
        }
    }

    /// Close every connection and socket: the router went.
    pub fn clear(&mut self) {
        self.tcp.clear();
        for (_, (_, task)) in self.udp.drain() {
            task.abort();
        }
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.tcp.len(), self.udp.len())
    }
}

fn tell(events: &WeakUnboundedSender<Command>, event: Event) {
    if let Some(tx) = events.upgrade() {
        let _ = tx.send(Command::Nat(event));
    }
}

/// A guest's TCP connection on the host.
async fn connection(
    flow: u64,
    to: SocketAddr,
    mut commands: UnboundedReceiver<ToConnection>,
    events: WeakUnboundedSender<Command>,
) {
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(to)).await {
        Ok(Ok(stream)) => stream,
        _ => {
            tell(&events, Event::ConnectFailed { flow });
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    tell(&events, Event::Connected { flow });
    let (mut reader, mut writer) = stream.into_split();
    let mut credit = TCP_CREDIT;
    let mut reading = true;
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let room = credit.min(buf.len());
        tokio::select! {
            command = commands.recv() => match command {
                None => return,
                Some(ToConnection::Send(data)) => {
                    if writer.write_all(&data).await.is_err() {
                        tell(&events, Event::Failed { flow });
                        return;
                    }
                }
                Some(ToConnection::Shutdown) => {
                    let _ = writer.shutdown().await;
                }
                Some(ToConnection::Credit(bytes)) => credit += bytes,
            },
            read = reader.read(&mut buf[..room]), if reading && room > 0 => match read {
                Ok(0) => {
                    reading = false;
                    tell(&events, Event::Eof { flow });
                }
                Ok(n) => {
                    credit -= n;
                    tell(&events, Event::Data { flow, data: buf[..n].to_vec() });
                }
                Err(_) => {
                    tell(&events, Event::Failed { flow });
                    return;
                }
            },
        }
    }
}

/// What comes to a guest's UDP flow's socket.
async fn datagrams(id: u64, socket: Arc<UdpSocket>, events: WeakUnboundedSender<Command>) {
    let mut buf = vec![0u8; 2048];
    loop {
        // An error is (on Windows) an earlier datagram's port being closed.
        if let Ok((len, from)) = socket.recv_from(&mut buf).await {
            tell(&events, Event::UdpData { id, from, data: buf[..len].to_vec() });
        }
    }
}
