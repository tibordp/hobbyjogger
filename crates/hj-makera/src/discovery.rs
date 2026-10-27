//! WiFi discovery. Machines broadcast `name,ip,port,busy[,state]` on UDP 3333 (see
//! `WifiProvider.cpp` in the firmware). The sender address is authoritative; the IP in the
//! payload is informational only.

use std::net::{Ipv4Addr, SocketAddr};

use serde::Serialize;
use tokio::net::UdpSocket;

pub const DISCOVERY_PORT: u16 = 3333;
pub const DEFAULT_TCP_PORT: u16 = 2222;

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Announcement {
    pub name: String,
    /// Where to connect: the sender's IP with the advertised TCP port.
    pub addr: SocketAddr,
    /// The IP the machine claims in its payload.
    pub advertised_ip: String,
    /// Another client is connected.
    pub busy: bool,
    /// Optional fifth field: the machine state (`Idle`, `Home`, ...), as sent by the Z1.
    pub state: Option<String>,
}

impl Announcement {
    pub fn parse(data: &[u8], sender: SocketAddr) -> Option<Self> {
        let text = std::str::from_utf8(data).ok()?;
        let fields: Vec<&str> = text.trim().split(',').map(str::trim).collect();
        if !(4..=5).contains(&fields.len()) {
            return None;
        }
        let name = fields[0];
        if name.is_empty() || name.chars().any(char::is_control) {
            return None;
        }
        let port: u16 = fields[2].parse().ok().filter(|p| *p != 0)?;
        let busy = match fields[3] {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        Some(Self {
            name: name.to_owned(),
            addr: SocketAddr::new(sender.ip(), port),
            advertised_ip: fields[1].to_owned(),
            busy,
            state: fields.get(4).map(|v| (*v).to_owned()),
        })
    }
}

/// Bind the discovery port with address reuse, so we can listen alongside other software (e.g.
/// Makera Studio) doing the same.
pub fn listen() -> std::io::Result<Listener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).into())?;
    Ok(Listener {
        socket: UdpSocket::from_std(socket.into())?,
    })
}

pub struct Listener {
    socket: UdpSocket,
}

impl Listener {
    /// Wait for the next datagram. Returns the raw bytes and sender alongside the parsed
    /// announcement, so callers can log announcements we failed to parse.
    pub async fn recv(&self) -> std::io::Result<(Option<Announcement>, Vec<u8>, SocketAddr)> {
        let mut buf = [0u8; 512];
        let (n, sender) = self.socket.recv_from(&mut buf).await?;
        let data = buf[..n].to_vec();
        Ok((Announcement::parse(&data, sender), data, sender))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> SocketAddr {
        "192.168.1.50:40000".parse().unwrap()
    }

    #[test]
    fn parses_four_and_five_fields() {
        let a = Announcement::parse(b"Z1-ABCD,192.168.1.50,2222,0", sender()).unwrap();
        assert_eq!(a.addr, "192.168.1.50:2222".parse().unwrap());
        assert!(!a.busy);
        assert_eq!(a.state, None);

        // Captured from a Z1 on firmware 1.1.2.
        let a = Announcement::parse(b"Makera_Z1_010967,10.0.0.1,2222,1,Idle\n", sender()).unwrap();
        assert!(a.busy);
        assert_eq!(a.state.as_deref(), Some("Idle"));
        // Sender wins over the advertised IP.
        assert_eq!(a.addr.ip(), sender().ip());
    }

    #[test]
    fn rejects_malformed() {
        assert!(Announcement::parse(b"hello", sender()).is_none());
        assert!(Announcement::parse(b"Z1,1.2.3.4,notaport,0", sender()).is_none());
        assert!(Announcement::parse(b"Z1,1.2.3.4,2222,2", sender()).is_none());
    }
}
