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
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroU32,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wayline_protocol::{BUCKET_SECONDS, Bucket, HISTORY, Sample, Series, State, Tunnel};

const LATENCY_WINDOW: usize = 5;
const PACKET_SIZE: usize = 32;
const BUCKET_SAMPLES: usize = 64;
const OPEN_BUCKETS: usize = 8;
const RETAINED_BUCKETS: usize = 80;
const SNAPSHOT_BUCKETS: usize = 8;
const UNMATCHED_LIMIT: u16 = 16;
const ROUTE_WINDOW: Duration = Duration::from_millis(200);
pub const MAX_PENDING: usize = 4;

struct Pending {
    started: Instant,
    deadline: Instant,
    bucket: u64,
    packet: [u8; PACKET_SIZE],
}

struct RoutePending {
    sequence: u32,
    deadline: Instant,
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
        Bucket {
            at: self.at,
            sent: self.sent,
            lost: self.lost,
            min_us: self.samples.first().copied(),
            median_us: self
                .samples
                .get(self.samples.len().saturating_sub(1) / 2)
                .copied(),
            max_us: self.samples.last().copied(),
        }
    }
}

pub fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub fn unix_millis() -> u64 {
    millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

const fn bucket_start(wall_ms: u64) -> u64 {
    let seconds = wall_ms / 1000;
    seconds - seconds % BUCKET_SECONDS
}

fn sysfs_number(interface: &str, attribute: &str, radix: u32) -> Option<u32> {
    let mut path = [0_u8; 64];
    let free = {
        let mut rest = &mut path[..];
        write!(rest, "/sys/class/net/{interface}/{attribute}").ok()?;
        rest.len()
    };
    let length = path.len() - free;
    let mut text = [0_u8; 16];
    let size = File::open(std::str::from_utf8(&path[..length]).ok()?)
        .ok()?
        .read(&mut text)
        .ok()?;
    let value = std::str::from_utf8(&text[..size]).ok()?.trim();
    u32::from_str_radix(value.trim_start_matches("0x"), radix).ok()
}

fn ifindex(interface: &str) -> Option<u32> {
    sysfs_number(interface, "ifindex", 10)
}

fn is_up(interface: &str) -> bool {
    sysfs_number(interface, "flags", 16).is_some_and(|flags| flags & 1 != 0)
}

pub struct Probe<'a> {
    tunnel: &'a Tunnel,
    target: Ipv4Addr,
    interval: Duration,
    timeout: Duration,
    sample_hint: usize,
    socket: Option<RegisteredSocket>,
    pending: Vec<Pending>,
    route_pending: Option<RoutePending>,
    next_due: Instant,
    state: State,
    generation: u32,
    last_success: Option<Instant>,
    sent: u64,
    received: u64,
    unmatched: u16,
    history: VecDeque<Sample>,
    open: Vec<OpenBucket>,
    closed: VecDeque<Bucket>,
}

impl<'a> Probe<'a> {
    pub fn new(
        tunnel: &'a Tunnel,
        default_target: Ipv4Addr,
        next_due: Instant,
        interval: Duration,
        timeout: Duration,
    ) -> Self {
        let per_bucket = BUCKET_SECONDS * 1000 / millis(interval).max(1) + 2;
        Self {
            tunnel,
            target: tunnel.target.unwrap_or(default_target),
            interval,
            timeout,
            sample_hint: usize::try_from(per_bucket)
                .map_or(BUCKET_SAMPLES, |n| n.min(BUCKET_SAMPLES)),
            socket: None,
            pending: Vec::new(),
            route_pending: None,
            next_due,
            state: State::Waiting,
            generation: 0,
            last_success: None,
            sent: 0,
            received: 0,
            unmatched: 0,
            history: VecDeque::new(),
            open: Vec::new(),
            closed: VecDeque::new(),
        }
    }

    pub fn series(&self, now: Instant, since: Option<u64>) -> Series {
        let skip = if since.is_some() {
            0
        } else {
            self.closed.len().saturating_sub(SNAPSHOT_BUCKETS)
        };
        let path_id = if self.tunnel.path_id.is_empty() {
            &self.tunnel.id
        } else {
            &self.tunnel.path_id
        };
        Series {
            id: self.tunnel.id.clone(),
            name: self.tunnel.name.clone(),
            interface: self.tunnel.interface.clone(),
            path_id: path_id.clone(),
            target: self.target,
            state: self.state,
            median_us: self.median_us(),
            sent: self.sent,
            received: self.received,
            success_age_ms: self.last_success.map(|at| millis(now.duration_since(at))),
            history: self.history.iter().copied().collect(),
            buckets: self
                .closed
                .iter()
                .skip(skip)
                .filter(|bucket| since.is_none_or(|cursor| bucket.at > cursor))
                .copied()
                .collect(),
        }
    }

    pub fn attempts(&self) -> usize {
        self.pending.len() + usize::from(self.route_pending.is_some())
    }

    pub const fn checking(&self) -> bool {
        self.route_pending.is_some()
    }

    pub const fn saturated(&self) -> bool {
        self.pending.len() >= MAX_PENDING
    }

    pub fn due(&self, now: Instant) -> bool {
        now >= self.next_due && self.route_pending.is_none()
    }

    pub fn waiting_for(&self, sequence: u32) -> bool {
        self.route_pending
            .as_ref()
            .is_some_and(|route| route.sequence == sequence)
    }

    pub fn listening(&self) -> Option<&RegisteredSocket> {
        self.socket.as_ref().filter(|_| !self.pending.is_empty())
    }

    pub fn deadline(&self, can_start: bool) -> Option<Instant> {
        let slot = match &self.route_pending {
            Some(route) => Some(route.deadline),
            None if can_start => Some(self.next_due),
            None => None,
        };
        self.pending
            .iter()
            .map(|pending| pending.deadline)
            .chain(slot)
            .min()
    }

    fn median_us(&self) -> Option<u32> {
        let mut window = [0_u32; LATENCY_WINDOW];
        let latest = self
            .history
            .iter()
            .rev()
            .take(LATENCY_WINDOW)
            .filter_map(|sample| sample.us);
        let mut count = 0;
        for (slot, us) in window.iter_mut().zip(latest) {
            *slot = us;
            count += 1;
        }
        let successful = window.get_mut(..count)?;
        successful.sort_unstable();
        successful.get(count.checked_sub(1)? / 2).copied()
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

    fn fail(&mut self, state: State) {
        self.socket = None;
        self.state = state;
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
        let index = self
            .open
            .iter()
            .position(|bucket| bucket.at == at)
            .unwrap_or_else(|| {
                if self.open.len() >= OPEN_BUCKETS {
                    self.open.remove(0);
                }
                self.open.push(OpenBucket {
                    at,
                    sent: 0,
                    lost: 0,
                    samples: Vec::with_capacity(self.sample_hint),
                });
                self.open.len() - 1
            });
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
        let bucket = self.open_bucket(bucket_start(unix_millis()));
        bucket.sent = bucket.sent.saturating_add(1);
        bucket.lost = bucket.lost.saturating_add(1);
        self.push_history(None);
    }

    pub fn query_route(&mut self, now: Instant, routes: &RouteLookup, sequence: u64) {
        self.advance_slot(now);

        let interface = self.tunnel.interface.as_str();
        let Some(index) = ifindex(interface) else {
            self.reset_path(0);
            self.state = State::InterfaceUnavailable;
            return;
        };
        if index != self.generation {
            self.reset_path(index);
        }
        if !is_up(interface) {
            self.reset_path(index);
            self.state = State::InterfaceDown;
            return;
        }
        let route_sequence = u32::try_from(sequence & u64::from(u32::MAX)).unwrap_or_default();
        if routes
            .request(
                self.tunnel.source,
                self.target,
                self.tunnel.mark,
                route_sequence,
            )
            .is_err()
        {
            self.fail(State::RouteQueryFailed);
            return;
        }
        self.route_pending = Some(RoutePending {
            sequence: route_sequence,
            counter: sequence,
            deadline: now + ROUTE_WINDOW,
        });
        self.state = State::CheckingRoute;
    }

    pub fn route_reply(&mut self, interface: Option<u32>, nonce: &[u8; 16]) {
        let Some(route) = self.route_pending.take() else {
            return;
        };
        if Instant::now() >= route.deadline {
            self.fail(State::RouteQueryTimeout);
            return;
        }
        if interface != Some(self.generation) {
            self.fail(if interface.is_none() {
                State::RouteUnavailable
            } else {
                State::RouteMismatch
            });
            return;
        }

        let current = ifindex(&self.tunnel.interface);
        if current != Some(self.generation) {
            self.reset_path(current.unwrap_or(0));
            self.state = State::InterfaceChanged;
            return;
        }
        if self.socket.is_none() {
            let Ok(socket) = open_socket(self.tunnel, self.target, self.generation) else {
                self.state = State::SocketSetupFailed;
                return;
            };
            self.socket = Some(socket);
        }
        let Some(socket) = &self.socket else {
            return;
        };
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        let Some(packet) = echo_request(route.counter, nonce) else {
            return;
        };
        let sent_at = Instant::now();
        let bucket = bucket_start(unix_millis());
        if matches!(socket.send(&packet), Ok(PACKET_SIZE)) {
            self.sent = self.sent.saturating_add(1);
            let open = self.open_bucket(bucket);
            open.sent = open.sent.saturating_add(1);
            self.pending.push(Pending {
                started: sent_at,
                deadline: sent_at + self.timeout,
                bucket,
                packet,
            });
            self.unmatched = UNMATCHED_LIMIT;
            self.state = State::Probing;
        } else {
            self.fail(State::SendFailed);
        }
    }

    pub fn drain(&mut self, now: Instant) {
        let mut bytes = [0_u8; 256];

        for _ in 0..16 {
            let Some(socket) = &self.socket else {
                return;
            };
            if self.pending.is_empty() {
                return;
            }
            let size = match (&**socket).read(&mut bytes) {
                Ok(size) => size,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.fail(State::ReceiveFailed);
                    return;
                }
            };
            let reply = &bytes[..size];
            if let Some(index) = self.pending.iter().position(|pending| {
                now <= pending.deadline && echo_reply(self.target, reply, &pending.packet)
            }) {
                let micros =
                    u32::try_from(now.duration_since(self.pending[index].started).as_micros())
                        .unwrap_or(u32::MAX);
                self.finish(index, Some(micros), now);
                continue;
            }
            self.unmatched = self.unmatched.saturating_sub(1);
            if self.unmatched == 0 {
                self.fail(State::ResponseLimitExceeded);
                return;
            }
        }
    }

    pub fn expire(&mut self, now: Instant, wall_ms: u64) {
        if self
            .route_pending
            .as_ref()
            .is_some_and(|route| now >= route.deadline)
        {
            self.route_pending = None;
            self.fail(State::RouteQueryTimeout);
        }
        while let Some(index) = self
            .pending
            .iter()
            .position(|pending| now >= pending.deadline)
        {
            self.finish(index, None, now);
        }
        self.close_buckets(wall_ms);
    }

    fn close_buckets(&mut self, wall_ms: u64) {
        let timeout_ms = millis(self.timeout);
        let pending = &self.pending;
        let closable = |bucket: &mut OpenBucket| {
            wall_ms >= (bucket.at + BUCKET_SECONDS) * 1000 + timeout_ms
                && !pending.iter().any(|waiting| waiting.bucket == bucket.at)
        };
        for bucket in self.open.extract_if(.., closable) {
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

fn open_socket(tunnel: &Tunnel, target: Ipv4Addr, index: u32) -> io::Result<RegisteredSocket> {
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
    socket.connect(&SockAddr::from(SocketAddrV4::new(target, 0)))?;
    RegisteredSocket::new(socket)
}
