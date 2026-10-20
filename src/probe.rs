use crate::config::Tunnel;
use crate::readiness::RegisteredSocket;
use crate::route::RouteLookup;
use pnet_packet::{
    Packet,
    icmp::{self, IcmpCode, IcmpPacket, IcmpTypes, echo_request::MutableEchoRequestPacket},
    ipv4::Ipv4Packet,
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::{
    fs,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroU32,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HISTORY: usize = 64;
pub const LATENCY_WINDOW: usize = 5;
const LOST: u32 = u32::MAX;
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

#[derive(Clone, Copy, serde::Serialize)]
pub struct Sample {
    pub at_ms: u64,
    pub serial: u64,
    pub us: Option<u32>,
}

pub struct Probe {
    pub socket: Option<RegisteredSocket>,
    pub pending: Option<Pending>,
    pub route_pending: Option<RoutePending>,
    pub next_due: Instant,
    pub state: &'static str,
    pub generation: u32,
    pub last_success: Option<Instant>,
    pub last_completed: Option<Instant>,
    pub sent: u64,
    pub received: u64,
    samples: [u32; HISTORY],
    timed: [Sample; HISTORY],
    serial: u64,
    count: usize,
    cursor: usize,
}

impl Probe {
    pub const fn new(next_due: Instant) -> Self {
        Self {
            socket: None,
            pending: None,
            route_pending: None,
            next_due,
            state: "waiting",
            generation: 0,
            last_success: None,
            last_completed: None,
            sent: 0,
            received: 0,
            samples: [LOST; HISTORY],
            timed: [Sample {
                at_ms: 0,
                serial: 0,
                us: None,
            }; HISTORY],
            serial: 0,
            count: 0,
            cursor: 0,
        }
    }

    pub fn timed_history(&self) -> Vec<Sample> {
        (0..self.count)
            .map(|offset| self.timed[(self.cursor + HISTORY - self.count + offset) % HISTORY])
            .collect()
    }

    pub const fn sample_count(&self) -> usize {
        self.count
    }
    pub fn history(&self) -> Vec<Option<u32>> {
        (0..self.count)
            .map(|offset| {
                let value = self.samples[(self.cursor + HISTORY - self.count + offset) % HISTORY];
                if value == LOST { None } else { Some(value) }
            })
            .collect()
    }
    pub fn lost(&self) -> usize {
        self.samples[..self.count]
            .iter()
            .filter(|&&x| x == LOST)
            .count()
    }
    pub fn median_us(&self) -> Option<u32> {
        let mut successful = [0_u32; LATENCY_WINDOW];
        let mut len = 0;
        for offset in 0..self.count.min(LATENCY_WINDOW) {
            let sample = self.samples[(self.cursor + HISTORY - 1 - offset) % HISTORY];
            if sample != LOST {
                successful[len] = sample;
                len += 1;
            }
        }
        if len == 0 {
            return None;
        }
        successful[..len].sort_unstable();
        Some(successful[(len - 1) / 2])
    }

    fn reset_path(&mut self, generation: u32) {
        self.socket = None;
        self.pending = None;
        self.route_pending = None;
        self.generation = generation;
        self.count = 0;
        self.cursor = 0;
        self.last_success = None;
        self.last_completed = None;
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
            self.state = "interface_unavailable";
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
            self.state = "interface_down";
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
            self.state = "route_query_failed";
            return;
        }
        self.route_pending = Some(RoutePending {
            sequence: u32::try_from(sequence & u64::from(u32::MAX)).unwrap_or_default(),
            counter: sequence,
            deadline: now + Duration::from_millis(200),
        });
        self.state = "checking_route";
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
            self.state = "route_query_timeout";
            return;
        }
        if interface != Some(self.generation) {
            self.socket = None;
            self.state = if interface.is_none() {
                "route_unavailable"
            } else {
                "route_mismatch"
            };
            return;
        }

        let current = fs::read_to_string(format!("/sys/class/net/{}/ifindex", tunnel.interface))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        if current != Some(self.generation) {
            self.reset_path(current.unwrap_or(0));
            self.state = "interface_changed";
            return;
        }
        if self.socket.is_none() {
            if let Ok(socket) = open_socket(tunnel, self.generation) {
                self.socket = Some(socket);
            } else {
                self.state = "socket_setup_failed";
                return;
            }
        }

        let Some(socket) = self.socket.as_ref() else {
            self.state = "socket_setup_failed";
            return;
        };
        if socket
            .connect(&SockAddr::from(SocketAddrV4::new(target, 0)))
            .is_err()
        {
            self.socket = None;
            self.state = "route_unavailable";
            return;
        }
        let mut packet = [0_u8; PACKET_SIZE];
        let sequence = route.counter;
        {
            let Some(mut echo) = MutableEchoRequestPacket::new(&mut packet) else {
                return;
            };
            echo.set_icmp_type(IcmpTypes::EchoRequest);
            echo.set_icmp_code(IcmpCode(0));
            echo.set_identifier(u16::from_be_bytes([nonce[0], nonce[1]]));
            echo.set_sequence_number(
                u16::try_from(sequence & u64::from(u16::MAX)).unwrap_or_default(),
            );
            let mut payload = [0_u8; 24];
            payload[..16].copy_from_slice(nonce);
            payload[16..].copy_from_slice(&sequence.to_be_bytes());
            echo.set_payload(&payload);
            let Some(view) = IcmpPacket::new(echo.packet()) else {
                return;
            };
            let sum = icmp::checksum(&view);
            echo.set_checksum(sum);
        }
        let sent_at = Instant::now();
        if matches!(socket.send(&packet), Ok(PACKET_SIZE)) {
            self.sent = self.sent.saturating_add(1);
            self.pending = Some(Pending {
                started: sent_at,
                deadline: sent_at + timeout,
                packet,
                remaining_packets: 16,
            });
            self.state = "probing";
        } else {
            self.socket = None;
            self.state = "send_failed";
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
                    self.state = "receive_failed";
                    return;
                }
            };
            let Some(pending) = self.pending.as_mut() else {
                continue;
            };
            pending.remaining_packets -= 1;
            if now > pending.deadline || size < 20 + PACKET_SIZE {
                continue;
            }
            let Some(ip) = Ipv4Packet::new(&bytes[..size]) else {
                continue;
            };
            let header = usize::from(ip.get_header_length()) * 4;
            if ip.get_version() != 4
                || header < 20
                || size != header + PACKET_SIZE
                || ip.get_source().octets() != target.octets()
            {
                continue;
            }
            if ip.payload().len() != PACKET_SIZE {
                continue;
            }
            let Some(echo) = IcmpPacket::new(ip.payload()) else {
                continue;
            };
            if echo.get_icmp_type() != IcmpTypes::EchoReply
                || echo.get_icmp_code() != IcmpCode(0)
                || icmp::checksum(&echo) != echo.get_checksum()
                || echo.packet()[4..] != pending.packet[4..]
            {
                continue;
            }
            let micros = u32::try_from(now.duration_since(pending.started).as_micros())
                .unwrap_or(LOST - 1)
                .min(LOST - 1);
            self.finish(Some(micros), now);
            return;
        }

        self.finish(None, now);
        self.socket = None;
        self.state = "response_limit_exceeded";
    }

    pub fn expire(&mut self, now: Instant) {
        if self
            .route_pending
            .as_ref()
            .is_some_and(|p| now >= p.deadline)
        {
            self.route_pending = None;
            self.socket = None;
            self.state = "route_query_timeout";
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
        self.serial = self.serial.saturating_add(1);
        self.timed[self.cursor] = Sample {
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            serial: self.serial,
            us: micros,
        };
        self.samples[self.cursor] = micros.unwrap_or(LOST);
        self.cursor = (self.cursor + 1) % HISTORY;
        self.count = (self.count + 1).min(HISTORY);
        self.last_completed = Some(now);
        if micros.is_some() {
            self.received = self.received.saturating_add(1);
            self.last_success = Some(now);
            self.state = "reachable";
        } else {
            self.state = "probe_timeout";
        }
    }
}

fn open_socket(tunnel: &Tunnel, index: u32) -> io::Result<RegisteredSocket> {
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))?;

    let index =
        NonZeroU32::new(index).ok_or_else(|| io::Error::other("invalid interface index"))?;
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
