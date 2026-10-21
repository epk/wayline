use netlink_packet_core::{NLM_F_REQUEST, NetlinkBuffer, NlaBuffer, NlasIterator, parse_u32};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io::{self, Read},
    net::Ipv4Addr,
};

const SIZE: usize = 60;
const REPLY_READS: usize = 16;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const RTA_DST: u16 = 1;
const RTA_SRC: u16 = 2;
const RTA_OIF: u16 = 4;
const RTA_MARK: u16 = 16;
const RTA_IP_PROTO: u16 = 27;
const AF_INET: u8 = 2;
const IPPROTO_ICMP: u8 = 1;
const RTN_UNICAST: u8 = 1;

pub struct RouteLookup {
    socket: Socket,
}
struct Reply {
    sequence: u32,
    interface: Option<u32>,
}

impl RouteLookup {
    pub fn new() -> io::Result<Self> {
        let socket = Socket::new(Domain::from(16), Type::RAW, Some(Protocol::from(0)))?;
        socket.set_nonblocking(true)?;
        socket.set_recv_buffer_size(8192)?;
        Ok(Self { socket })
    }

    pub fn lookup(
        &self,
        source: Ipv4Addr,
        target: Ipv4Addr,
        mark: u32,
        sequence: u32,
    ) -> io::Result<Option<u32>> {
        self.request(source, target, mark, sequence)?;
        let mut data = [0_u8; 4096];
        for _ in 0..REPLY_READS {
            let size = (&self.socket).read(&mut data)?;
            if let Some(reply) = decode(&data[..size])
                && reply.sequence == sequence
            {
                return Ok(reply.interface);
            }
        }
        Err(io::Error::other("no matching route reply"))
    }

    fn request(
        &self,
        source: Ipv4Addr,
        target: Ipv4Addr,
        mark: u32,
        sequence: u32,
    ) -> io::Result<()> {
        let mut data = [0_u8; SIZE];
        let mut header = NetlinkBuffer::new(&mut data[..]);
        header.set_length(u32::try_from(SIZE).map_err(io::Error::other)?);
        header.set_message_type(RTM_GETROUTE);
        header.set_flags(NLM_F_REQUEST);
        header.set_sequence_number(sequence);
        data[16] = AF_INET;
        data[17] = 32;
        data[18] = 32;
        attribute(&mut data[28..36], RTA_DST, &target.octets())?;
        attribute(&mut data[36..44], RTA_SRC, &source.octets())?;
        attribute(&mut data[44..52], RTA_MARK, &mark.to_ne_bytes())?;
        attribute(&mut data[52..60], RTA_IP_PROTO, &[IPPROTO_ICMP])?;
        if self.socket.send(&data)? == SIZE {
            Ok(())
        } else {
            Err(io::Error::other("incomplete route query"))
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
    Some(Reply {
        sequence: message.sequence_number(),
        interface: route_interface(&message),
    })
}

fn route_interface(message: &NetlinkBuffer<&[u8]>) -> Option<u32> {
    if message.message_type() != RTM_NEWROUTE {
        return None;
    }
    let (head, attributes) = message.payload().split_at_checked(12)?;
    let [AF_INET, _, _, _, _, _, _, RTN_UNICAST, ..] = head else {
        return None;
    };
    let mut interface = None;
    for attribute in NlasIterator::new(attributes) {
        let attribute = attribute.ok()?;
        if attribute.kind() == RTA_OIF {
            if interface.is_some() || attribute.nested_flag() || attribute.network_byte_order_flag()
            {
                return None;
            }
            interface = Some(parse_u32(attribute.value()).ok()?);
        }
    }
    interface
}
