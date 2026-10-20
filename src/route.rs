use crate::readiness::RegisteredSocket;
use netlink_packet_core::{NLM_F_REQUEST, NetlinkBuffer, NlaBuffer, NlasIterator, parse_u32};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io::{self, Read},
    net::Ipv4Addr,
};

pub struct RouteLookup {
    pub socket: RegisteredSocket,
}
pub struct Reply {
    pub sequence: u32,
    pub interface: Option<u32>,
}

impl RouteLookup {
    pub fn new() -> io::Result<Self> {
        let socket = Socket::new(Domain::from(16), Type::RAW, Some(Protocol::from(0)))?;
        socket.set_nonblocking(true)?;
        socket.set_recv_buffer_size(8192)?;
        Ok(Self {
            socket: RegisteredSocket::new(socket)?,
        })
    }

    pub fn request(
        &self,
        source: Ipv4Addr,
        target: Ipv4Addr,
        mark: u32,
        sequence: u32,
    ) -> io::Result<()> {
        let mut data = [0_u8; 60];
        let mut header = NetlinkBuffer::new(&mut data[..]);
        header.set_length(60);
        header.set_message_type(26);
        header.set_flags(NLM_F_REQUEST);
        header.set_sequence_number(sequence);
        data[16] = 2;
        data[17] = 32;
        data[18] = 32;
        attribute(&mut data[28..36], 1, &target.octets())?;
        attribute(&mut data[36..44], 2, &source.octets())?;
        attribute(&mut data[44..52], 16, &mark.to_ne_bytes())?;

        attribute(&mut data[52..60], 27, &[1])?;
        match self.socket.send(&data)? {
            60 => Ok(()),
            _ => Err(io::Error::other("incomplete route query")),
        }
    }

    pub fn receive(&self) -> io::Result<Option<Reply>> {
        let mut data = [0_u8; 4096];
        match (&*self.socket).read(&mut data) {
            Ok(size) => Ok(decode(&data[..size])),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn attribute(data: &mut [u8], kind: u16, value: &[u8]) -> io::Result<()> {
    let mut attr = NlaBuffer::new(data);
    attr.set_length(u16::try_from(4 + value.len()).map_err(io::Error::other)?);
    attr.set_kind(kind);
    attr.value_mut().copy_from_slice(value);
    Ok(())
}

fn decode(data: &[u8]) -> Option<Reply> {
    let message = NetlinkBuffer::new_checked(data).ok()?;
    let sequence = message.sequence_number();
    let mut reply = Reply {
        sequence,
        interface: None,
    };
    let body = message.payload();

    if message.message_type() != 24 || body.len() < 12 || body[0] != 2 || body[7] != 1 {
        return Some(reply);
    }
    for attr in NlasIterator::new(&body[12..]) {
        let Ok(attr) = attr else {
            return Some(Reply {
                sequence,
                interface: None,
            });
        };
        if attr.kind() == 4 {
            if reply.interface.is_some() || attr.nested_flag() || attr.network_byte_order_flag() {
                return Some(Reply {
                    sequence,
                    interface: None,
                });
            }
            reply.interface = parse_u32(attr.value()).ok();
            if reply.interface.is_none() {
                return Some(reply);
            }
        }
    }
    Some(reply)
}
