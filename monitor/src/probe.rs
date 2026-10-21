use crate::readiness::RegisteredSocket;
use crate::route::RouteLookup;
use pnet_packet::{
    Packet,
    icmp::{self, IcmpCode, IcmpPacket, IcmpTypes, echo_request::MutableEchoRequestPacket},
    ipv4::Ipv4Packet,
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroU32,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wayline_protocol::{HISTORY, Sample, State, Tunnel};

const LATENCY_WINDOW: usize = 5;
const PACKET_SIZE: usize = 32;

pub struct Pending {
    pub started: Instant,
    pub deadline: Instant,
    packet: [u8; PACKET_SIZE],
    remaining_packets: usize,
}

pub struct RoutePending {
    pub sequence: u32,
    pub deadline: Instant,
    counter: u64,
}

pub struct Probe {
    pub socket: Option<RegisteredSocket>,
    pub pending: Option<Pending>,
    pub route_pending: Option<RoutePending>,
    pub next_due: Instant,
    pub state: State,
    pub generation: u32,
    pub last_success: Option<Instant>,
    pub sent: u64,
    pub received: u64,
    history: VecDeque<Sample>,
}

impl Probe {
    pub const fn new(next_due: Instant) -> Self {
        Self {
            socket: None,
            pending: None,
            route_pending: None,
            next_due,
            state: State::Waiting,
            generation: 0,
            last_success: None,
            sent: 0,
            received: 0,
            history: VecDeque::new(),
        }
    }

    pub fn history(&self) -> Vec<Sample> {
        self.history.iter().copied().collect()
    }

    pub fn median_us(&self) -> Option<u32> {
        let mut successful: Vec<u32> = self
            .history
            .iter()
            .rev()
            .take(LATENCY_WINDOW)
            .filter_map(|sample| sample.us)
            .collect();
        successful.sort_unstable();
        successful
            .get(successful.len().checked_sub(1)? / 2)
            .copied()
    }

    fn reset_path(&mut self, generation: u32) {
        self.socket = None;
        self.pending = None;
        self.route_pending = None;
        self.generation = generation;
        self.history.clear();
        self.last_success = None;
        self.sent = 0;
        self.received = 0;
    }

    pub fn query_route(
        &mut self,
        tunnel: &Tunnel,
        target: Ipv4Addr,
        now: Instant,
        interval: Duration,
        routes: &RouteLookup,
        sequence: u64,
    ) {
        self.next_due = now + interval;

        let identity = fs::read_to_string(format!("/sys/class/net/{}/ifindex", tunnel.interface))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let Some(index) = identity else {
            self.reset_path(0);
            self.state = State::InterfaceUnavailable;
            return;
        };
        if index != self.generation {
            self.reset_path(index);
        }
        let up = fs::read_to_string(format!("/sys/class/net/{}/flags", tunnel.interface))
            .ok()
            .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
            .is_some_and(|flags| flags & 1 != 0);
        if !up {
            self.reset_path(index);
            self.state = State::InterfaceDown;
            return;
        }
        if routes
            .request(
                tunnel.source,
                target,
                tunnel.mark,
                u32::try_from(sequence & u64::from(u32::MAX)).unwrap_or_default(),
            )
            .is_err()
        {
            self.socket = None;
            self.state = State::RouteQueryFailed;
            return;
        }
        self.route_pending = Some(RoutePending {
            sequence: u32::try_from(sequence & u64::from(u32::MAX)).unwrap_or_default(),
            counter: sequence,
            deadline: now + Duration::from_millis(200),
        });
        self.state = State::CheckingRoute;
    }

    pub fn route_reply(
        &mut self,
        interface: Option<u32>,
        tunnel: &Tunnel,
        target: Ipv4Addr,
        timeout: Duration,
        nonce: &[u8; 16],
    ) {
        let Some(route) = self.route_pending.take() else {
            return;
        };
        if Instant::now() >= route.deadline {
            self.socket = None;
            self.state = State::RouteQueryTimeout;
            return;
        }
        if interface != Some(self.generation) {
            self.socket = None;
            self.state = if interface.is_none() {
                State::RouteUnavailable
            } else {
                State::RouteMismatch
            };
            return;
        }

        let current = fs::read_to_string(format!("/sys/class/net/{}/ifindex", tunnel.interface))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        if current != Some(self.generation) {
            self.reset_path(current.unwrap_or(0));
            self.state = State::InterfaceChanged;
            return;
        }
        if self.socket.is_none() {
            if let Ok(socket) = open_socket(tunnel, self.generation) {
                self.socket = Some(socket);
            } else {
                self.state = State::SocketSetupFailed;
                return;
            }
        }

        let Some(socket) = self.socket.as_ref() else {
            self.state = State::SocketSetupFailed;
            return;
        };
        if socket
            .connect(&SockAddr::from(SocketAddrV4::new(target, 0)))
            .is_err()
        {
            self.socket = None;
            self.state = State::RouteUnavailable;
            return;
        }
        let Some(packet) = echo_request(route.counter, nonce) else {
            return;
        };
        let sent_at = Instant::now();
        if matches!(socket.send(&packet), Ok(PACKET_SIZE)) {
            self.sent = self.sent.saturating_add(1);
            self.pending = Some(Pending {
                started: sent_at,
                deadline: sent_at + timeout,
                packet,
                remaining_packets: 16,
            });
            self.state = State::Probing;
        } else {
            self.socket = None;
            self.state = State::SendFailed;
        }
    }

    pub fn drain(&mut self, target: Ipv4Addr, now: Instant) {
        let mut bytes = [0_u8; 256];

        for _ in 0..16 {
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.remaining_packets == 0)
            {
                break;
            }
            let Some(socket) = self.socket.as_ref() else {
                return;
            };
            let size = match (&**socket).read(&mut bytes) {
                Ok(size) => size,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.finish(None, now);
                    self.socket = None;
                    self.state = State::ReceiveFailed;
                    return;
                }
            };
            let Some(pending) = self.pending.as_mut() else {
                continue;
            };
            pending.remaining_packets -= 1;
            if now > pending.deadline || !echo_reply(target, &bytes[..size], &pending.packet) {
                continue;
            }
            let micros =
                u32::try_from(now.duration_since(pending.started).as_micros()).unwrap_or(u32::MAX);
            self.finish(Some(micros), now);
            return;
        }

        self.finish(None, now);
        self.socket = None;
        self.state = State::ResponseLimitExceeded;
    }

    pub fn expire(&mut self, now: Instant) {
        if self
            .route_pending
            .as_ref()
            .is_some_and(|p| now >= p.deadline)
        {
            self.route_pending = None;
            self.socket = None;
            self.state = State::RouteQueryTimeout;
        }
        if self.pending.as_ref().is_some_and(|p| now >= p.deadline) {
            self.finish(None, now);
        }
    }

    pub const fn busy(&self) -> bool {
        self.pending.is_some() || self.route_pending.is_some()
    }

    fn finish(&mut self, micros: Option<u32>, now: Instant) {
        if self.pending.take().is_none() {
            return;
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Sample {
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            us: micros,
        });
        if micros.is_some() {
            self.received = self.received.saturating_add(1);
            self.last_success = Some(now);
            self.state = State::Reachable;
        } else {
            self.state = State::ProbeTimeout;
        }
    }
}

fn echo_request(sequence: u64, nonce: &[u8; 16]) -> Option<[u8; PACKET_SIZE]> {
    let mut packet = [0_u8; PACKET_SIZE];
    let mut payload = [0_u8; 24];
    payload[..16].copy_from_slice(nonce);
    payload[16..].copy_from_slice(&sequence.to_be_bytes());
    let mut echo = MutableEchoRequestPacket::new(&mut packet)?;
    echo.set_icmp_type(IcmpTypes::EchoRequest);
    echo.set_icmp_code(IcmpCode(0));
    echo.set_identifier(u16::from_be_bytes([nonce[0], nonce[1]]));
    echo.set_sequence_number(u16::try_from(sequence & u64::from(u16::MAX)).unwrap_or_default());
    echo.set_payload(&payload);
    let sum = icmp::checksum(&IcmpPacket::new(echo.packet())?);
    echo.set_checksum(sum);
    Some(packet)
}

fn echo_reply(target: Ipv4Addr, bytes: &[u8], sent: &[u8; PACKET_SIZE]) -> bool {
    let Some(ip) = Ipv4Packet::new(bytes) else {
        return false;
    };
    let header = usize::from(ip.get_header_length()) * 4;
    if ip.get_version() != 4
        || header < 20
        || bytes.len() != header + PACKET_SIZE
        || ip.get_source().octets() != target.octets()
    {
        return false;
    }
    IcmpPacket::new(ip.payload()).is_some_and(|echo| {
        echo.get_icmp_type() == IcmpTypes::EchoReply
            && echo.get_icmp_code() == IcmpCode(0)
            && icmp::checksum(&echo) == echo.get_checksum()
            && echo.packet()[4..] == sent[4..]
    })
}

fn open_socket(tunnel: &Tunnel, index: u32) -> io::Result<RegisteredSocket> {
    let index =
        NonZeroU32::new(index).ok_or_else(|| io::Error::other("invalid interface index"))?;
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))?;
    socket.bind_device_by_index_v4(Some(index))?;
    if tunnel.mark != 0 {
        socket.set_mark(tunnel.mark)?;
    }
    socket.bind(&SockAddr::from(SocketAddrV4::new(tunnel.source, 0)))?;
    socket.set_recv_buffer_size(4096)?;
    socket.set_send_buffer_size(4096)?;
    socket.set_nonblocking(true)?;
    RegisteredSocket::new(socket)
}
