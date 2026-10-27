//! Byte-stream transports to the machine. Both carry the same framed protocol.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// An open byte stream to the machine.
pub struct Link {
    pub reader: BoxRead,
    pub writer: BoxWrite,
    /// Human-readable description, e.g. `tcp 192.168.1.50:2222`.
    pub description: String,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn connect_tcp(addr: SocketAddr) -> io::Result<Link> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connection timed out"))??;
    stream.set_nodelay(true)?;
    let (reader, writer) = stream.into_split();
    Ok(Link {
        reader: Box::new(reader),
        writer: Box::new(writer),
        description: format!("tcp {addr}"),
    })
}

/// USB on the Z1: a vendor-class device with a bulk IN/OUT endpoint pair, served by the ESP32
/// (not an FTDI serial port like the Carvera). Needs a udev rule on Linux.
pub mod usb {
    use nusb::descriptors::TransferType;
    use nusb::transfer::{Bulk, Direction, In, Out};
    use nusb::{DeviceInfo, MaybeFuture};

    use super::*;

    pub const VENDOR_ID: u16 = 0x303A;
    pub const PRODUCT_ID: u16 = 0x4002;

    const READ_TRANSFER_SIZE: usize = 4096;
    const WRITE_BUFFER_SIZE: usize = 16384;

    pub fn is_z1(info: &DeviceInfo) -> bool {
        info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID
    }

    /// All attached devices with the Z1's VID/PID.
    pub async fn list() -> io::Result<Vec<DeviceInfo>> {
        Ok(nusb::list_devices().await?.filter(is_z1).collect())
    }

    /// Open the first attached Z1 (or the one with the given serial number).
    pub async fn open(serial: Option<&str>) -> io::Result<Link> {
        let info = list()
            .await?
            .into_iter()
            .find(|d| serial.is_none() || d.serial_number() == serial)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no USB device {VENDOR_ID:04x}:{PRODUCT_ID:04x} found"),
                )
            })?;
        let device = info.open().await?;
        let config = device.active_configuration()?;

        let (interface, ep_in, ep_out) = config
            .interface_alt_settings()
            .find_map(|alt| {
                let bulk = |dir| {
                    alt.endpoints()
                        .find(|ep| {
                            ep.transfer_type() == TransferType::Bulk && ep.direction() == dir
                        })
                        .map(|ep| ep.address())
                };
                Some((
                    alt.interface_number(),
                    bulk(Direction::In)?,
                    bulk(Direction::Out)?,
                ))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no interface with a bulk IN/OUT pair",
                )
            })?;

        let iface = device.detach_and_claim_interface(interface).wait()?;
        let reader = iface
            .endpoint::<Bulk, In>(ep_in)?
            .reader(READ_TRANSFER_SIZE);
        let writer = iface
            .endpoint::<Bulk, Out>(ep_out)?
            .writer(WRITE_BUFFER_SIZE);

        Ok(Link {
            reader: Box::new(reader),
            writer: Box::new(writer),
            description: format!(
                "usb {:04x}:{:04x} bus {} addr {} (interface {interface}, in {ep_in:#04x}, out {ep_out:#04x})",
                info.vendor_id(),
                info.product_id(),
                info.bus_id(),
                info.device_address()
            ),
        })
    }
}
