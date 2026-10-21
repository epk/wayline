use crate::readiness::RegisteredSocket;
use crate::route::RouteLookup;
use pnet_packet::{
    Packet,
    icmp::{self, IcmpCode, IcmpPacket, IcmpTypes, echo_request::MutableEchoRequestPacket},
    ipv4::Ipv4Packet,
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroU32,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wayline_protocol::{BUCKET_SECONDS, Bucket, HISTORY, Sample, State, Tunnel};

const LATENCY_WINDOW: usize = 5;
const PACKET_SIZE: usize = 32;
const BUCKET_SAMPLES: usize = 64;
const OPEN_BUCKETS: usize = 8;
const UNMATCHED_LIMIT: u16 = 16;
pub const MAX_PENDING: usize = 4;
pub const RETAINED_BUCKETS: usize = 80;
pub const SNAPSHOT_BUCKETS: usize = 8;

pub struct Pending {
    pub started: Instant,
    pub deadline: Instant,
    pub bucket: u64,
    packet: [u8; PACKET_SIZE],
}

pub struct RoutePending {
    pub sequence: u32,
    pub deadline: Instant,
    counter: u64,
}

struct OpenBucket {
    at: u64,
    sent: u16,
    lost: u16,
    samples: Vec<u32>,
}

impl OpenBucket {
    fn close(mut self) -> Bucket {
        self.samples.sort_unstable();
        let last = self.samples.len().checked_sub(1);
        Bucket {
            at: self.at,
            sent: self.sent,
            lost: self.lost,
            min_us: self.samples.first().copied(),
            median_us: last.and_then(|last| self.samples.get(last / 2)).copied(),
            max_us: self.samples.last().copied(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Link {
    index: Option<u32>,
    up: bool,
}

#[derive(Default)]
pub struct Links {
    entries: BTreeMap<String, Link>,
}

impl Links {
    pub fn get(&mut self, interface: &str) -> Link {
        *self
            .entries
            .entry(interface.to_owned())
            .or_insert_with(|| read_link(interface))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn read_link(interface: &str) -> Link {
    let index = fs::read_to_string(format!("/sys/class/net/{interface}/ifindex"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    let up = index.is_some()
        && fs::read_to_string(format!("/sys/class/net/{interface}/flags"))
            .ok()
            .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
            .is_some_and(|flags| flags & 1 != 0);
    Link { index, up }
}

pub struct Probe {
    pub socket: Option<RegisteredSocket>,
    pub pending: Vec<Pending>,
    pub route_pending: Option<RoutePending>,
    pub next_due: Instant,
    pub state: State,
    pub generation: u32,
    pub last_success: Option<Instant>,
    pub sent: u64,
    pub received: u64,
    interval: Duration,
    unmatched: u16,
    history: VecDeque<Sample>,
    open: Vec<OpenBucket>,
    closed: VecDeque<Bucket>,
}

impl Probe {
    pub const fn new(next_due: Instant, interval: Duration) -> Self {
        Self {
            socket: None,
            pending: Vec::new(),
            route_pending: None,
            next_due,
            state: State::Waiting,
            generation: 0,
            last_success: None,
            sent: 0,
            received: 0,
            interval,
            unmatched: 0,
            history: VecDeque::new(),
            open: Vec::new(),
            closed: VecDeque::new(),
        }
    }

    pub fn history(&self) -> Vec<Sample> {
        self.history.iter().copied().collect()
    }

    pub fn buckets(&self, since: Option<u64>) -> Vec<Bucket> {
        let recent = self.closed.len().saturating_sub(SNAPSHOT_BUCKETS);
        self.closed
            .iter()
            .enumerate()
            .filter(|(index, bucket)| since.map_or(*index >= recent, |cursor| bucket.at > cursor))
            .map(|(_, bucket)| *bucket)
            .collect()
    }

    pub fn attempts(&self) -> usize {
        self.pending.len() + usize::from(self.route_pending.is_some())
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
        self.pending.clear();
        self.route_pending = None;
        self.generation = generation;
        self.history.clear();
        self.open.clear();
        self.closed.clear();
        self.unmatched = 0;
        self.last_success = None;
        self.sent = 0;
        self.received = 0;
    }

    fn advance_slot(&mut self, now: Instant) {
        let next = self.next_due + self.interval;
        self.next_due = if next > now {
            next
        } else {
            now + self.interval
        };
    }

    fn open_bucket(&mut self, at: u64) -> &mut OpenBucket {
        if let Some(index) = self.open.iter().position(|bucket| bucket.at == at) {
            return &mut self.open[index];
        }
        if self.open.len() >= OPEN_BUCKETS {
            self.open.remove(0);
        }
        self.open.push(OpenBucket {
            at,
            sent: 0,
            lost: 0,
            samples: Vec::with_capacity(BUCKET_SAMPLES),
        });
        let index = self.open.len() - 1;
        &mut self.open[index]
    }

    fn push_history(&mut self, micros: Option<u32>) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Sample {
            at_ms: unix_millis(),
            us: micros,
        });
    }

    pub fn skip_slot(&mut self, now: Instant) {
        self.advance_slot(now);
        self.sent = self.sent.saturating_add(1);
        let bucket = self.open_bucket(bucket_start(unix_time().as_secs()));
        bucket.sent = bucket.sent.saturating_add(1);
        bucket.lost = bucket.lost.saturating_add(1);
        self.push_history(None);
    }

    pub fn query_route(
        &mut self,
        tunnel: &Tunnel,
        target: Ipv4Addr,
        now: Instant,
        routes: &RouteLookup,
        sequence: u64,
        links: &mut Links,
    ) {
        self.advance_slot(now);

        let link = links.get(&tunnel.interface);
        let Some(index) = link.index else {
            self.reset_path(0);
            self.state = State::InterfaceUnavailable;
            return;
        };
        if index != self.generation {
            self.reset_path(index);
        }
        if !link.up {
            self.reset_path(index);
            self.state = State::InterfaceDown;
            return;
        }
        let route_sequence = u32::try_from(sequence & u64::from(u32::MAX)).unwrap_or_default();
        if routes
            .request(tunnel.source, target, tunnel.mark, route_sequence)
            .is_err()
        {
            self.socket = None;
            self.state = State::RouteQueryFailed;
            return;
        }
        self.route_pending = Some(RoutePending {
            sequence: route_sequence,
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
        links: &mut Links,
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

        let current = links.get(&tunnel.interface).index;
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
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        let Some(packet) = echo_request(route.counter, nonce) else {
            return;
        };
        let sent_at = Instant::now();
        let bucket = bucket_start(unix_time().as_secs());
        if matches!(socket.send(&packet), Ok(PACKET_SIZE)) {
            self.sent = self.sent.saturating_add(1);
            let open = self.open_bucket(bucket);
            open.sent = open.sent.saturating_add(1);
            self.pending.push(Pending {
                started: sent_at,
                deadline: sent_at + timeout,
                bucket,
                packet,
            });
            self.unmatched = UNMATCHED_LIMIT;
            self.state = State::Probing;
        } else {
            self.socket = None;
            self.state = State::SendFailed;
        }
    }

    pub fn drain(&mut self, target: Ipv4Addr, now: Instant) {
        let mut bytes = [0_u8; 256];

        for _ in 0..16 {
            let Some(socket) = self.socket.as_ref() else {
                return;
            };
            let size = match (&**socket).read(&mut bytes) {
                Ok(size) => size,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.socket = None;
                    self.state = State::ReceiveFailed;
                    return;
                }
            };
            if self.pending.is_empty() {
                continue;
            }
            let reply = &bytes[..size];
            if let Some(index) = self.pending.iter().position(|pending| {
                now <= pending.deadline && echo_reply(target, reply, &pending.packet)
            }) {
                let micros =
                    u32::try_from(now.duration_since(self.pending[index].started).as_micros())
                        .unwrap_or(u32::MAX);
                self.finish(index, Some(micros), now);
                continue;
            }
            self.unmatched = self.unmatched.saturating_sub(1);
            if self.unmatched == 0 {
                self.socket = None;
                self.state = State::ResponseLimitExceeded;
                return;
            }
        }
    }

    pub fn expire(&mut self, now: Instant, timeout: Duration) {
        if self
            .route_pending
            .as_ref()
            .is_some_and(|p| now >= p.deadline)
        {
            self.route_pending = None;
            self.socket = None;
            self.state = State::RouteQueryTimeout;
        }
        while let Some(index) = self
            .pending
            .iter()
            .position(|pending| now >= pending.deadline)
        {
            self.finish(index, None, now);
        }
        self.close_buckets(unix_millis(), timeout);
    }

    fn close_buckets(&mut self, wall_ms: u64, timeout: Duration) {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut index = 0;
        while index < self.open.len() {
            let at = self.open[index].at;
            let due = (at + BUCKET_SECONDS) * 1000 + timeout_ms;
            let waiting = self.pending.iter().any(|pending| pending.bucket == at);
            if wall_ms < due || waiting {
                index += 1;
                continue;
            }
            let bucket = self.open.remove(index);
            if bucket.sent > 0 {
                if self.closed.len() == RETAINED_BUCKETS {
                    self.closed.pop_front();
                }
                self.closed.push_back(bucket.close());
            }
        }
    }

    fn finish(&mut self, index: usize, micros: Option<u32>, now: Instant) {
        let pending = self.pending.remove(index);
        self.push_history(micros);
        let bucket = self.open_bucket(pending.bucket);
        if let Some(us) = micros {
            if bucket.samples.len() < BUCKET_SAMPLES {
                bucket.samples.push(us);
            }
            self.received = self.received.saturating_add(1);
            self.last_success = Some(now);
            self.state = State::Reachable;
        } else {
            bucket.lost = bucket.lost.saturating_add(1);
            self.state = State::ProbeTimeout;
        }
    }
}

fn unix_time() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn unix_millis() -> u64 {
    u64::try_from(unix_time().as_millis()).unwrap_or(u64::MAX)
}

const fn bucket_start(seconds: u64) -> u64 {
    seconds - seconds % BUCKET_SECONDS
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
