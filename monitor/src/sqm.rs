use crate::probe::{bucket_start, millis, unix_millis};
use crate::wifi::valid_device;
use netlink_packet_core::{NLM_F_REQUEST, NetlinkBuffer, NlaBuffer, NlasIterator};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use wayline_protocol::{
    BUCKET_SECONDS, SqmAction, SqmBucket, SqmConfig, SqmHistory, SqmInterface, SqmLinkStatus,
    SqmStatus,
};

const RETAINED: usize = 80;
const SNAPSHOT: usize = 8;
const COOLDOWN: Duration = Duration::from_secs(1);

const NLMSG_HEADER_LEN: usize = 16;
const TCMSG_LEN: usize = 20;
const NLA_HEADER_LEN: usize = 4;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWQDISC: u16 = 36;
const RTM_GETQDISC: u16 = 38;
const NLM_F_ACK: u16 = 4;
const NLM_F_DUMP: u16 = 0x300;
const TC_H_ROOT: u32 = 0xffff_ffff;
const TCA_KIND: u16 = 1;
const TCA_OPTIONS: u16 = 2;
const TCA_CAKE_BASE_RATE64: u16 = 2;
const NLA_F_NESTED: u16 = 0x8000;
const MAX_DUMP_READS: usize = 128;

pub const ORIGINALS: &str = "sqm-original.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cake {
    pub handle: u32,
    pub kbps: u32,
}

pub type Originals = BTreeMap<String, Cake>;

pub trait Shaper {
    fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)>;
    fn cake(&mut self, device: &str) -> Option<Cake>;
    fn set_cake_bandwidth(&mut self, device: &str, handle: u32, kbps: u32) -> io::Result<()>;
    fn recall(&mut self) -> Originals;
    fn remember(&mut self, originals: &Originals);
}

pub struct LinuxShaper {
    originals: PathBuf,
}

impl LinuxShaper {
    #[must_use]
    pub const fn new(originals: PathBuf) -> Self {
        Self { originals }
    }
}

impl Shaper for LinuxShaper {
    fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)> {
        valid_device(device).then_some(())?;
        let rx = read_sysfs_u64(device, "rx_bytes")?;
        let tx = read_sysfs_u64(device, "tx_bytes")?;
        Some((rx, tx))
    }

    fn cake(&mut self, device: &str) -> Option<Cake> {
        read_ifindex(device).and_then(root_cake)
    }

    fn set_cake_bandwidth(&mut self, device: &str, handle: u32, kbps: u32) -> io::Result<()> {
        let ifindex = read_ifindex(device)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown network device"))?;
        let request = encode_set_cake_bandwidth(ifindex, handle, kbps, 2)?;
        let socket = open_netlink()?;
        if socket.send(&request)? != request.len() {
            return Err(io::Error::other("incomplete netlink qdisc request"));
        }
        let mut buffer = [0_u8; 4096];
        for _ in 0..16 {
            let size = (&socket).read(&mut buffer)?;
            if let Some(result) = decode_netlink_ack(&buffer[..size], 2) {
                return result;
            }
        }
        Err(io::Error::other("missing netlink qdisc acknowledgement"))
    }

    fn recall(&mut self) -> Originals {
        let entries: Vec<(String, u32, u32)> = fs::read(&self.originals)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        entries
            .into_iter()
            .map(|(device, handle, kbps)| (device, Cake { handle, kbps }))
            .collect()
    }

    fn remember(&mut self, originals: &Originals) {
        if originals.is_empty() {
            let _ = fs::remove_file(&self.originals);
            return;
        }
        let entries: Vec<(&str, u32, u32)> = originals
            .iter()
            .map(|(device, cake)| (device.as_str(), cake.handle, cake.kbps))
            .collect();
        if let Ok(bytes) = serde_json::to_vec(&entries)
            && let Err(error) = wayline_support::atomic(&self.originals, &bytes)
        {
            eprintln!("waylined: cannot keep the original SQM rates: {error}");
        }
    }
}

fn read_sysfs_u64(device: &str, counter: &str) -> Option<u64> {
    fs::read_to_string(format!("/sys/class/net/{device}/statistics/{counter}"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn read_ifindex(device: &str) -> Option<i32> {
    valid_device(device).then_some(())?;
    fs::read_to_string(format!("/sys/class/net/{device}/ifindex"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn open_netlink() -> io::Result<Socket> {
    let socket = Socket::new(Domain::from(16), Type::RAW, Some(Protocol::from(0)))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.set_recv_buffer_size(32 * 1024)?;
    Ok(socket)
}

fn root_cake(ifindex: i32) -> Option<Cake> {
    let socket = open_netlink().ok()?;
    let request = encode_get_qdisc(ifindex, 1);
    if socket.send(&request).ok()? != request.len() {
        return None;
    }
    let mut buffer = [0_u8; 8192];
    for _ in 0..MAX_DUMP_READS {
        let size = (&socket).read(&mut buffer).ok()?;
        match decode_qdisc_dump(&buffer[..size], ifindex, 1) {
            DumpOutcome::Found(cake) => return Some(cake),
            DumpOutcome::Done | DumpOutcome::Error => return None,
            DumpOutcome::Continue => (),
        }
    }
    None
}

const fn align4(len: usize) -> usize {
    (len + 3) & !3
}

fn write_tcmsg(buf: &mut [u8], ifindex: i32, handle: u32, parent: u32) {
    buf[..TCMSG_LEN].fill(0);
    buf[4..8].copy_from_slice(&ifindex.to_ne_bytes());
    buf[8..12].copy_from_slice(&handle.to_ne_bytes());
    buf[12..16].copy_from_slice(&parent.to_ne_bytes());
}

fn write_nla(buf: &mut [u8], kind: u16, value: &[u8]) -> io::Result<usize> {
    let total = NLA_HEADER_LEN + value.len();
    let aligned = align4(total);
    let slot = buf
        .get_mut(..aligned)
        .ok_or_else(|| io::Error::other("netlink attribute buffer overflow"))?;
    slot.fill(0);
    let mut attr = NlaBuffer::new(slot);
    attr.set_length(u16::try_from(total).map_err(io::Error::other)?);
    attr.set_kind(kind);
    if (kind & NLA_F_NESTED) != 0 {
        attr.set_nested_flag();
    }
    attr.value_mut().copy_from_slice(value);
    Ok(aligned)
}

fn encode_get_qdisc(ifindex: i32, sequence: u32) -> [u8; NLMSG_HEADER_LEN + TCMSG_LEN] {
    let mut data = [0_u8; NLMSG_HEADER_LEN + TCMSG_LEN];
    let mut header = NetlinkBuffer::new(&mut data[..]);
    header.set_length(u32::try_from(NLMSG_HEADER_LEN + TCMSG_LEN).unwrap_or(u32::MAX));
    header.set_message_type(RTM_GETQDISC);
    header.set_flags(NLM_F_REQUEST | NLM_F_DUMP);
    header.set_sequence_number(sequence);
    write_tcmsg(&mut data[NLMSG_HEADER_LEN..], ifindex, 0, TC_H_ROOT);
    data
}

fn encode_set_cake_bandwidth(
    ifindex: i32,
    handle: u32,
    kbps: u32,
    sequence: u32,
) -> io::Result<Vec<u8>> {
    let bps = u64::from(kbps) * 125;
    let mut nested = [0_u8; 12];
    let nested_len = write_nla(&mut nested, TCA_CAKE_BASE_RATE64, &bps.to_ne_bytes())?;
    let kind_len = align4(NLA_HEADER_LEN + 5);
    let options_len = align4(NLA_HEADER_LEN + nested_len);
    let total = NLMSG_HEADER_LEN + TCMSG_LEN + kind_len + options_len;
    let mut data = vec![0_u8; total];
    let mut header = NetlinkBuffer::new(&mut data[..]);
    header.set_length(u32::try_from(total).map_err(io::Error::other)?);
    header.set_message_type(RTM_NEWQDISC);
    header.set_flags(NLM_F_REQUEST | NLM_F_ACK);
    header.set_sequence_number(sequence);
    write_tcmsg(
        &mut data[NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + TCMSG_LEN],
        ifindex,
        handle,
        TC_H_ROOT,
    );
    let mut offset = NLMSG_HEADER_LEN + TCMSG_LEN;
    offset += write_nla(&mut data[offset..], TCA_KIND, b"cake\0")?;
    write_nla(
        &mut data[offset..],
        TCA_OPTIONS | NLA_F_NESTED,
        &nested[..nested_len],
    )?;
    Ok(data)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DumpOutcome {
    Continue,
    Done,
    Found(Cake),
    Error,
}

fn decode_qdisc_dump(data: &[u8], ifindex: i32, sequence: u32) -> DumpOutcome {
    let mut offset = 0;
    while offset + NLMSG_HEADER_LEN <= data.len() {
        let Ok(message) = NetlinkBuffer::new_checked(&data[offset..]) else {
            return DumpOutcome::Error;
        };
        let length = usize::try_from(message.length()).unwrap_or(0);
        if length < NLMSG_HEADER_LEN || offset + length > data.len() {
            return DumpOutcome::Error;
        }
        if message.sequence_number() == sequence {
            match message.message_type() {
                NLMSG_DONE => return DumpOutcome::Done,
                NLMSG_ERROR => return DumpOutcome::Error,
                RTM_NEWQDISC => {
                    if let Some(cake) = matching_cake(message.payload(), ifindex) {
                        return DumpOutcome::Found(cake);
                    }
                }
                _ => (),
            }
        }
        offset += align4(length);
    }
    DumpOutcome::Continue
}

fn matching_cake(payload: &[u8], target_ifindex: i32) -> Option<Cake> {
    let (head, attrs) = payload.split_at_checked(TCMSG_LEN)?;
    let ifindex = i32::from_ne_bytes([head[4], head[5], head[6], head[7]]);
    let handle = u32::from_ne_bytes([head[8], head[9], head[10], head[11]]);
    let parent = u32::from_ne_bytes([head[12], head[13], head[14], head[15]]);
    if ifindex != target_ifindex || parent != TC_H_ROOT {
        return None;
    }
    let mut cake = false;
    let mut bytes_per_second = 0_u64;
    for attr in NlasIterator::new(attrs) {
        let attr = attr.ok()?;
        if attr.kind() == TCA_KIND {
            cake = attr.value() == b"cake\0" || attr.value() == b"cake";
        } else if attr.kind() == TCA_OPTIONS {
            for option in NlasIterator::new(attr.value()).flatten() {
                if option.kind() == TCA_CAKE_BASE_RATE64
                    && let Ok(raw) = <[u8; 8]>::try_from(option.value())
                {
                    bytes_per_second = u64::from_ne_bytes(raw);
                }
            }
        }
    }
    cake.then(|| Cake {
        handle,
        kbps: u32::try_from(bytes_per_second / 125).unwrap_or(u32::MAX),
    })
}

fn decode_netlink_ack(data: &[u8], sequence: u32) -> Option<io::Result<()>> {
    let mut offset = 0;
    while offset + NLMSG_HEADER_LEN <= data.len() {
        let message = NetlinkBuffer::new_checked(&data[offset..]).ok()?;
        let length = usize::try_from(message.length()).unwrap_or(0);
        if length < NLMSG_HEADER_LEN || offset + length > data.len() {
            return Some(Err(io::Error::other("truncated netlink reply")));
        }
        if message.sequence_number() == sequence && message.message_type() == NLMSG_ERROR {
            let payload = message.payload();
            if payload.len() < 4 {
                return Some(Err(io::Error::other("truncated netlink error payload")));
            }
            let code = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
            return Some(if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code.saturating_neg()))
            });
        }
        offset += align4(length);
    }
    None
}

struct DirectionState {
    rate_kbps: u32,
    base_kbps: u32,
    original: Option<Cake>,
    applied: Option<Cake>,
    action: SqmAction,
    last_decrease: Option<Instant>,
}

impl DirectionState {
    const fn new() -> Self {
        Self {
            rate_kbps: 0,
            base_kbps: 0,
            original: None,
            applied: None,
            action: SqmAction::Idle,
            last_decrease: None,
        }
    }

    fn sync(&mut self, cake: Cake, recalled: Option<Cake>, bounds: &DirectionBounds) -> bool {
        let replaced = self
            .original
            .is_none_or(|original| original.handle != cake.handle);
        let changed = self
            .applied
            .is_some_and(|applied| applied.kbps != cake.kbps);
        if !replaced && !changed && self.applied.is_some() {
            return false;
        }
        let original = recalled
            .filter(|recalled| replaced && recalled.handle == cake.handle)
            .unwrap_or(cake);
        let limit = |kbps: u32| {
            if kbps == 0 {
                bounds.max
            } else {
                kbps.clamp(bounds.min, bounds.max)
            }
        };
        self.original = Some(original);
        self.applied = Some(cake);
        self.base_kbps = limit(original.kbps);
        self.rate_kbps = limit(cake.kbps);
        self.last_decrease = None;
        true
    }
}

struct DirectionBounds {
    min: u32,
    max: u32,
}

struct LinkState {
    iface: SqmInterface,
    dl: DirectionState,
    ul: DirectionState,
    rx_kbps: u32,
    tx_kbps: u32,
    baseline_us: Option<u32>,
    delta_us: Option<i64>,
    adjustments: u64,
    prev_bytes: Option<(Instant, u64, u64)>,
    last_updated: Option<Instant>,
    open: Option<SqmBucket>,
    closed: VecDeque<SqmBucket>,
}

impl LinkState {
    const fn new(iface: SqmInterface) -> Self {
        Self {
            iface,
            dl: DirectionState::new(),
            ul: DirectionState::new(),
            rx_kbps: 0,
            tx_kbps: 0,
            baseline_us: None,
            delta_us: None,
            adjustments: 0,
            prev_bytes: None,
            last_updated: None,
            open: None,
            closed: VecDeque::new(),
        }
    }

    const fn shaped(&self) -> bool {
        self.dl.applied.is_some() || self.ul.applied.is_some()
    }

    fn close_bucket(&mut self, wall_ms: u64) {
        if let Some(bucket) = self
            .open
            .filter(|bucket| wall_ms >= (bucket.at + BUCKET_SECONDS) * 1_000)
        {
            self.open = None;
            if bucket.samples > 0 {
                if self.closed.len() == RETAINED {
                    self.closed.pop_front();
                }
                self.closed.push_back(bucket);
            }
        }
    }

    fn record_bucket(&mut self, wall_ms: u64) {
        self.close_bucket(wall_ms);
        let at = bucket_start(wall_ms);
        let bucket = self.open.get_or_insert_with(|| SqmBucket {
            at,
            ..SqmBucket::default()
        });
        bucket.samples = bucket.samples.saturating_add(1);
        bucket.dl_kbps_sum = bucket
            .dl_kbps_sum
            .saturating_add(u64::from(self.dl.rate_kbps));
        bucket.dl_kbps_min = Some(
            bucket
                .dl_kbps_min
                .map_or(self.dl.rate_kbps, |low| low.min(self.dl.rate_kbps)),
        );
        bucket.dl_kbps_max = Some(
            bucket
                .dl_kbps_max
                .map_or(self.dl.rate_kbps, |high| high.max(self.dl.rate_kbps)),
        );
        bucket.ul_kbps_sum = bucket
            .ul_kbps_sum
            .saturating_add(u64::from(self.ul.rate_kbps));
        bucket.ul_kbps_min = Some(
            bucket
                .ul_kbps_min
                .map_or(self.ul.rate_kbps, |low| low.min(self.ul.rate_kbps)),
        );
        bucket.ul_kbps_max = Some(
            bucket
                .ul_kbps_max
                .map_or(self.ul.rate_kbps, |high| high.max(self.ul.rate_kbps)),
        );
        if let Some(base) = self.baseline_us {
            bucket.baseline_us_n = bucket.baseline_us_n.saturating_add(1);
            bucket.baseline_us_sum = bucket.baseline_us_sum.saturating_add(u64::from(base));
        }
        if let Some(delta) = self.delta_us {
            bucket.delta_us_max = Some(bucket.delta_us_max.map_or(delta, |high| high.max(delta)));
        }
        if self.dl.action == SqmAction::Increase {
            bucket.increases = bucket.increases.saturating_add(1);
        }
        if self.ul.action == SqmAction::Increase {
            bucket.increases = bucket.increases.saturating_add(1);
        }
        if self.dl.action == SqmAction::Decrease {
            bucket.decreases = bucket.decreases.saturating_add(1);
        }
        if self.ul.action == SqmAction::Decrease {
            bucket.decreases = bucket.decreases.saturating_add(1);
        }
    }

    fn buckets(&self, since: Option<u64>) -> Vec<SqmBucket> {
        let skip = if since.is_some() {
            0
        } else {
            self.closed.len().saturating_sub(SNAPSHOT)
        };
        self.closed
            .iter()
            .skip(skip)
            .filter(|bucket| since.is_none_or(|cursor| bucket.at > cursor))
            .copied()
            .collect()
    }
}

pub struct Controller<S: Shaper> {
    config: SqmConfig,
    shaper: S,
    links: Vec<LinkState>,
    recalled: Originals,
}

impl<S: Shaper> Controller<S> {
    #[must_use]
    pub fn new(config: SqmConfig, mut shaper: S) -> Self {
        let links = config
            .interfaces
            .iter()
            .cloned()
            .map(LinkState::new)
            .collect();
        let recalled = shaper.recall();
        Self {
            config,
            shaper,
            links,
            recalled,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &SqmConfig {
        &self.config
    }

    pub fn step(&mut self, now: Instant, rtts: &[(String, u32)]) {
        self.step_at(now, unix_millis(), rtts);
    }

    pub fn step_at(&mut self, now: Instant, wall_ms: u64, rtts: &[(String, u32)]) {
        let threshold_us = i64::from(self.config.delay_threshold_ms) * 1_000;
        let dl_bounds = DirectionBounds {
            min: self.config.min_dl_kbps,
            max: self.config.max_dl_kbps,
        };
        let ul_bounds = DirectionBounds {
            min: self.config.min_ul_kbps,
            max: self.config.max_ul_kbps,
        };
        let mut synced = false;
        for link in &mut self.links {
            let (rx_kbps, tx_kbps) = measure_throughput(&mut self.shaper, link, now);
            link.rx_kbps = rx_kbps;
            link.tx_kbps = tx_kbps;
            let Some(rtt_us) = match_rtt(&link.iface, rtts) else {
                continue;
            };
            link.last_updated = Some(now);
            let base = update_baseline(link, rtt_us, rx_kbps, tx_kbps);
            let delta_us = i64::from(rtt_us) - i64::from(base);
            link.delta_us = Some(delta_us);
            let directions = [
                (
                    &link.iface.ingress_device,
                    &mut link.dl,
                    rx_kbps,
                    &dl_bounds,
                ),
                (&link.iface.egress_device, &mut link.ul, tx_kbps, &ul_bounds),
            ];
            for (device, dir, rate_kbps, bounds) in directions {
                let step = Step {
                    rate_kbps,
                    delta_us,
                    threshold_us,
                    bounds,
                    now,
                };
                let outcome = apply_direction(&mut self.shaper, &self.recalled, device, dir, &step);
                synced |= outcome.synced;
                if outcome.adjusted {
                    link.adjustments = link.adjustments.saturating_add(1);
                }
            }
            if link.shaped() {
                link.record_bucket(wall_ms);
            }
        }
        if synced {
            self.shaper.remember(&self.originals());
        }
    }

    fn originals(&self) -> Originals {
        self.links
            .iter()
            .flat_map(|link| {
                [
                    (&link.iface.ingress_device, &link.dl),
                    (&link.iface.egress_device, &link.ul),
                ]
            })
            .filter_map(|(device, dir)| Some((device.clone(), dir.original?)))
            .collect()
    }

    pub fn restore(&mut self) {
        for link in &mut self.links {
            for (device, dir) in [
                (&link.iface.ingress_device, &mut link.dl),
                (&link.iface.egress_device, &mut link.ul),
            ] {
                let (Some(original), Some(applied)) = (dir.original, dir.applied) else {
                    continue;
                };
                if applied.handle != original.handle || applied.kbps == original.kbps {
                    continue;
                }
                let live = self.shaper.cake(device);
                if live.is_some_and(|live| live.handle == original.handle)
                    && self
                        .shaper
                        .set_cake_bandwidth(device, original.handle, original.kbps)
                        .is_ok()
                {
                    dir.applied = Some(original);
                    dir.rate_kbps = original.kbps;
                }
            }
        }
        self.recalled.clear();
        self.shaper.remember(&Originals::new());
    }

    #[must_use]
    pub fn status(&self, now: Instant, since: Option<u64>) -> SqmStatus {
        let active = self.links.iter().any(LinkState::shaped);
        let links = self
            .links
            .iter()
            .map(|link| SqmLinkStatus {
                id: link.iface.id.clone(),
                name: link.iface.name.clone(),
                egress_device: link.iface.egress_device.clone(),
                ingress_device: link.iface.ingress_device.clone(),
                dl_kbps: link.dl.rate_kbps,
                ul_kbps: link.ul.rate_kbps,
                rx_kbps: link.rx_kbps,
                tx_kbps: link.tx_kbps,
                baseline_us: link.baseline_us,
                delta_us: link.delta_us,
                dl_action: link.dl.action,
                ul_action: link.ul.action,
                adjustments: link.adjustments,
                updated_age_ms: link
                    .last_updated
                    .map(|at| millis(now.saturating_duration_since(at))),
            })
            .collect();
        let buckets = self
            .links
            .iter()
            .map(|link| SqmHistory {
                id: link.iface.id.clone(),
                buckets: link.buckets(since),
            })
            .collect();
        SqmStatus {
            active,
            links,
            buckets,
        }
    }
}

impl<S: Shaper> Drop for Controller<S> {
    fn drop(&mut self) {
        self.restore();
    }
}

fn match_rtt(iface: &SqmInterface, rtts: &[(String, u32)]) -> Option<u32> {
    let device_match = rtts
        .iter()
        .filter(|(name, _)| *name == iface.egress_device || *name == iface.ingress_device)
        .map(|(_, rtt)| *rtt)
        .min();
    device_match.or_else(|| {
        rtts.iter()
            .filter(|(name, _)| *name == iface.id || *name == iface.name)
            .map(|(_, rtt)| *rtt)
            .min()
    })
}

fn measure_throughput<S: Shaper>(shaper: &mut S, link: &mut LinkState, now: Instant) -> (u32, u32) {
    let Some((rx_bytes, tx_bytes)) = shaper.read_bytes(&link.iface.egress_device) else {
        link.prev_bytes = None;
        return (0, 0);
    };
    let rates = link
        .prev_bytes
        .map_or((0, 0), |(prev_at, prev_rx, prev_tx)| {
            let elapsed_us = now.saturating_duration_since(prev_at).as_micros();
            if elapsed_us == 0 {
                (0, 0)
            } else {
                let to_kbps = |delta: u64| -> u32 {
                    let kbps = u128::from(delta).saturating_mul(8_000) / elapsed_us;
                    u32::try_from(kbps).unwrap_or(u32::MAX)
                };
                (
                    to_kbps(rx_bytes.saturating_sub(prev_rx)),
                    to_kbps(tx_bytes.saturating_sub(prev_tx)),
                )
            }
        });
    link.prev_bytes = Some((now, rx_bytes, tx_bytes));
    rates
}

fn update_baseline(link: &mut LinkState, rtt_us: u32, rx_kbps: u32, tx_kbps: u32) -> u32 {
    let next = match link.baseline_us {
        None => rtt_us,
        Some(base) if rtt_us < base => {
            let ewma = (u64::from(base) * 9 + u64::from(rtt_us)) / 10;
            u32::try_from(ewma).unwrap_or(base)
        }
        Some(base) if rx_kbps < link.dl.rate_kbps / 4 && tx_kbps < link.ul.rate_kbps / 4 => {
            base.saturating_add((rtt_us - base) / 100)
        }
        Some(base) => base,
    };
    link.baseline_us = Some(next);
    next
}

struct Step<'a> {
    rate_kbps: u32,
    delta_us: i64,
    threshold_us: i64,
    bounds: &'a DirectionBounds,
    now: Instant,
}

fn decide_rate(dir: &DirectionState, step: &Step) -> (u32, SqmAction) {
    let bounds = step.bounds;
    let shaper = u64::from(dir.rate_kbps);
    let rate = u64::from(step.rate_kbps);
    let in_cooldown = dir
        .last_decrease
        .is_some_and(|at| step.now.saturating_duration_since(at) < COOLDOWN);
    if step.delta_us > step.threshold_us {
        let under_load = rate >= shaper * 15 / 100 || step.rate_kbps >= bounds.min;
        if under_load && !in_cooldown {
            let target = (rate * 85 / 100).min(shaper * 85 / 100);
            let clamped = u32::try_from(target)
                .unwrap_or(bounds.max)
                .clamp(bounds.min, bounds.max);
            (clamped, SqmAction::Decrease)
        } else {
            (dir.rate_kbps, SqmAction::Hold)
        }
    } else if rate >= shaper * 65 / 100 {
        let target = (shaper * 105 / 100).max(shaper.saturating_add(500));
        let clamped = u32::try_from(target)
            .unwrap_or(bounds.max)
            .clamp(bounds.min, bounds.max);
        (clamped, SqmAction::Increase)
    } else if rate < shaper * 15 / 100 {
        let gap = dir.rate_kbps.abs_diff(dir.base_kbps);
        let move_by = gap.min(gap.max(500) / 20);
        let next = if dir.rate_kbps > dir.base_kbps {
            dir.rate_kbps - move_by
        } else {
            dir.rate_kbps + move_by
        };
        (next, SqmAction::Idle)
    } else {
        (dir.rate_kbps, SqmAction::Hold)
    }
}

#[derive(Default)]
struct Outcome {
    adjusted: bool,
    synced: bool,
}

fn apply_direction<S: Shaper>(
    shaper: &mut S,
    recalled: &Originals,
    device: &str,
    dir: &mut DirectionState,
    step: &Step,
) -> Outcome {
    let Some(cake) = shaper.cake(device) else {
        dir.applied = None;
        dir.action = SqmAction::MissingQdisc;
        return Outcome::default();
    };
    let synced = dir.sync(cake, recalled.get(device).copied(), step.bounds);
    let (next_kbps, action) = decide_rate(dir, step);
    let applied = dir.applied.map_or(0, |applied| applied.kbps);
    let adjusted = if applied == 0 || next_kbps.abs_diff(applied) >= (applied / 50).max(500) {
        if shaper
            .set_cake_bandwidth(device, cake.handle, next_kbps)
            .is_err()
        {
            dir.action = SqmAction::Error;
            return Outcome {
                adjusted: false,
                synced,
            };
        }
        dir.applied = Some(Cake {
            handle: cake.handle,
            kbps: next_kbps,
        });
        true
    } else {
        false
    };
    dir.rate_kbps = next_kbps;
    dir.action = action;
    if action == SqmAction::Decrease {
        dir.last_decrease = Some(step.now);
    }
    Outcome { adjusted, synced }
}

#[cfg(test)]
mod tests {
    use super::{
        Cake, Controller, DumpOutcome, NLA_F_NESTED, NLMSG_DONE, NLMSG_ERROR, NLMSG_HEADER_LEN,
        Originals, RTM_NEWQDISC, Shaper, TC_H_ROOT, TCA_CAKE_BASE_RATE64, TCA_KIND, TCA_OPTIONS,
        TCMSG_LEN, align4, decode_netlink_ack, decode_qdisc_dump, encode_set_cake_bandwidth,
        write_nla, write_tcmsg,
    };
    use netlink_packet_core::{NetlinkBuffer, NlaBuffer, NlasIterator};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use wayline_protocol::{SqmAction, SqmConfig, SqmInterface};

    type Log = Rc<RefCell<Vec<(String, u32)>>>;

    #[derive(Default)]
    struct FakeShaper {
        bytes: BTreeMap<String, (u64, u64)>,
        cakes: BTreeMap<String, Cake>,
        fail_set: BTreeSet<String>,
        applied: Log,
        store: Rc<RefCell<Originals>>,
    }

    impl Shaper for FakeShaper {
        fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)> {
            self.bytes.get(device).copied()
        }

        fn cake(&mut self, device: &str) -> Option<Cake> {
            self.cakes.get(device).copied()
        }

        fn set_cake_bandwidth(&mut self, device: &str, handle: u32, kbps: u32) -> io::Result<()> {
            if self.fail_set.contains(device) {
                return Err(io::Error::other("simulated netlink failure"));
            }
            let cake = self
                .cakes
                .get_mut(device)
                .filter(|cake| cake.handle == handle)
                .ok_or_else(|| io::Error::other("no such qdisc"))?;
            cake.kbps = kbps;
            self.applied.borrow_mut().push((device.to_owned(), kbps));
            Ok(())
        }

        fn recall(&mut self) -> Originals {
            self.store.borrow().clone()
        }

        fn remember(&mut self, originals: &Originals) {
            originals.clone_into(&mut self.store.borrow_mut());
        }
    }

    const DL: u32 = 0x8002_0000;
    const UL: u32 = 0x8001_0000;

    fn test_config() -> SqmConfig {
        SqmConfig {
            interfaces: vec![SqmInterface {
                id: "wan".into(),
                name: "Primary WAN".into(),
                egress_device: "eth1".into(),
                ingress_device: "ifb4eth1".into(),
            }],
            delay_threshold_ms: 20,
            min_dl_kbps: 5_000,
            max_dl_kbps: 100_000,
            min_ul_kbps: 5_000,
            max_ul_kbps: 50_000,
        }
    }

    fn ready_shaper() -> FakeShaper {
        let mut shaper = FakeShaper::default();
        shaper.cakes.insert(
            "ifb4eth1".into(),
            Cake {
                handle: DL,
                kbps: 40_000,
            },
        );
        shaper.cakes.insert(
            "eth1".into(),
            Cake {
                handle: UL,
                kbps: 20_000,
            },
        );
        shaper.bytes.insert("eth1".into(), (0, 0));
        shaper
    }

    #[test]
    fn baseline_initializes_and_uses_asymmetric_ewma() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 20_000)]);
        let status = controller.status(t0, None);
        assert_eq!(status.links[0].baseline_us, Some(20_000));
        assert_eq!(status.links[0].delta_us, Some(0));

        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 10_000)],
        );
        let status = controller.status(t0 + Duration::from_secs(1), None);
        assert_eq!(status.links[0].baseline_us, Some(19_000));

        controller.step_at(
            t0 + Duration::from_secs(2),
            3_000,
            &[("eth1".into(), 29_000)],
        );
        let status = controller.status(t0 + Duration::from_secs(2), None);
        assert_eq!(status.links[0].baseline_us, Some(19_100));
    }

    #[test]
    fn bufferbloat_spike_steps_down_loaded_direction_only() {
        let shaper = ready_shaper();
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);

        controller
            .shaper
            .bytes
            .insert("eth1".into(), (3_750_000, 10_000));
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 50_000)],
        );

        let status = controller.status(t0 + Duration::from_secs(1), None);
        assert_eq!(status.links[0].rx_kbps, 30_000);
        assert_eq!(status.links[0].tx_kbps, 80);
        assert_eq!(status.links[0].dl_action, SqmAction::Decrease);
        assert_eq!(status.links[0].ul_action, SqmAction::Hold);
        assert_eq!(status.links[0].dl_kbps, 25_500);
        assert_eq!(status.links[0].ul_kbps, 20_000);
    }

    #[test]
    fn refractory_cooldown_prevents_rapid_double_drops() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);

        controller
            .shaper
            .bytes
            .insert("eth1".into(), (3_750_000, 0));
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 50_000)],
        );
        assert_eq!(
            controller.status(t0 + Duration::from_secs(1), None).links[0].dl_action,
            SqmAction::Decrease
        );

        controller
            .shaper
            .bytes
            .insert("eth1".into(), (5_000_000, 0));
        controller.step_at(
            t0 + Duration::from_millis(1_500),
            2_500,
            &[("eth1".into(), 50_000)],
        );
        let mid = controller.status(t0 + Duration::from_millis(1_500), None);
        assert_eq!(mid.links[0].dl_action, SqmAction::Hold);
        assert_eq!(mid.links[0].dl_kbps, 25_500);

        controller
            .shaper
            .bytes
            .insert("eth1".into(), (6_250_000, 0));
        controller.step_at(
            t0 + Duration::from_secs(2),
            3_000,
            &[("eth1".into(), 50_000)],
        );
        let after = controller.status(t0 + Duration::from_secs(2), None);
        assert_eq!(after.links[0].dl_action, SqmAction::Decrease);
        assert!(after.links[0].dl_kbps < 25_500);
    }

    #[test]
    fn clean_high_load_ramps_up_toward_max() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);

        let mut rx = 0_u64;
        for sec in 1..=5 {
            rx += 4_500_000;
            controller.shaper.bytes.insert("eth1".into(), (rx, 0));
            controller.step_at(
                t0 + Duration::from_secs(sec),
                (sec + 1) * 1_000,
                &[("eth1".into(), 20_000)],
            );
        }
        let status = controller.status(t0 + Duration::from_secs(5), None);
        assert_eq!(status.links[0].dl_action, SqmAction::Increase);
        assert!(status.links[0].dl_kbps > 40_000);
        assert!(status.links[0].dl_kbps <= 100_000);
    }

    #[test]
    fn idle_decay_gently_returns_above_base_rate_toward_base() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);

        controller
            .shaper
            .bytes
            .insert("eth1".into(), (4_500_000, 0));
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 18_000)],
        );
        assert_eq!(
            controller.status(t0 + Duration::from_secs(1), None).links[0].dl_kbps,
            42_000
        );

        controller.step_at(
            t0 + Duration::from_secs(2),
            3_000,
            &[("eth1".into(), 16_000)],
        );
        let status = controller.status(t0 + Duration::from_secs(2), None);
        assert_eq!(status.links[0].dl_action, SqmAction::Idle);
        assert_eq!(status.links[0].dl_kbps, 41_900);
    }

    #[test]
    fn missing_cake_qdisc_reports_missing_qdisc_cleanly() {
        let mut shaper = FakeShaper::default();
        shaper.bytes.insert("eth1".into(), (100_000, 50_000));
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        let status = controller.status(t0, None);
        assert!(!status.active);
        assert_eq!(status.links[0].dl_action, SqmAction::MissingQdisc);
        assert_eq!(status.links[0].ul_action, SqmAction::MissingQdisc);
        assert_eq!(status.links[0].adjustments, 0);
        assert_eq!(status.buckets[0].buckets, []);
        assert!(controller.links[0].open.is_none());
    }

    #[test]
    fn buckets_accumulate_and_close_every_15_seconds() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 15_000, &[("eth1".into(), 15_000)]);
        controller
            .shaper
            .bytes
            .insert("eth1".into(), (4_500_000, 0));
        controller.step_at(
            t0 + Duration::from_secs(1),
            16_000,
            &[("eth1".into(), 18_000)],
        );
        controller.step_at(
            t0 + Duration::from_secs(16),
            31_000,
            &[("eth1".into(), 17_000)],
        );

        let status = controller.status(t0 + Duration::from_secs(16), None);
        assert_eq!(status.buckets.len(), 1);
        assert_eq!(status.buckets[0].buckets.len(), 1);
        let bucket = status.buckets[0].buckets[0];
        assert_eq!(bucket.at, 15);
        assert_eq!(bucket.samples, 2);
        assert_eq!(bucket.increases, 1);
    }

    #[test]
    fn starting_keeps_the_kernel_rate_until_a_decision_changes_it() {
        let shaper = ready_shaper();
        let applied = Rc::clone(&shaper.applied);
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        let status = controller.status(t0, None);
        assert!(status.active);
        assert_eq!(
            (status.links[0].dl_kbps, status.links[0].ul_kbps),
            (40_000, 20_000)
        );
        assert_eq!(*applied.borrow(), []);
    }

    #[test]
    fn an_unlimited_or_out_of_range_shaper_starts_inside_the_limits() {
        let mut shaper = ready_shaper();
        shaper.cakes.insert(
            "ifb4eth1".into(),
            Cake {
                handle: DL,
                kbps: 0,
            },
        );
        shaper.cakes.insert(
            "eth1".into(),
            Cake {
                handle: UL,
                kbps: 900_000,
            },
        );
        let applied = Rc::clone(&shaper.applied);
        let mut controller = Controller::new(test_config(), shaper);
        controller.step_at(Instant::now(), 1_000, &[("eth1".into(), 15_000)]);
        assert_eq!(
            *applied.borrow(),
            [
                ("ifb4eth1".to_owned(), 100_000),
                ("eth1".to_owned(), 50_000)
            ]
        );
        drop(controller);
        assert_eq!(
            applied.borrow()[2..],
            [("ifb4eth1".to_owned(), 0), ("eth1".to_owned(), 900_000)]
        );
    }

    #[test]
    fn a_link_without_a_new_sample_keeps_its_rates() {
        let mut controller = Controller::new(test_config(), ready_shaper());
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 50_000)],
        );
        for sec in 2..=6 {
            controller
                .shaper
                .bytes
                .insert("eth1".into(), (3_750_000 * sec, 0));
            controller.step_at(t0 + Duration::from_secs(sec), (sec + 1) * 1_000, &[]);
        }
        let status = controller.status(t0 + Duration::from_secs(6), None);
        assert!(status.links[0].dl_kbps >= 25_500);
    }

    #[test]
    fn stopping_restores_the_original_rates_once() {
        let shaper = ready_shaper();
        let applied = Rc::clone(&shaper.applied);
        let store = Rc::clone(&shaper.store);
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        controller
            .shaper
            .bytes
            .insert("eth1".into(), (3_750_000, 10_000));
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 50_000)],
        );
        assert_eq!(*applied.borrow(), [("ifb4eth1".to_owned(), 25_500)]);
        assert_eq!(
            store.borrow().get("ifb4eth1").map(|cake| cake.kbps),
            Some(40_000)
        );
        controller.restore();
        drop(controller);
        assert_eq!(
            *applied.borrow(),
            [
                ("ifb4eth1".to_owned(), 25_500),
                ("ifb4eth1".to_owned(), 40_000)
            ]
        );
        assert_eq!(*store.borrow(), Originals::new());
    }

    #[test]
    fn a_restarted_controller_restores_the_rate_from_before_the_crash() {
        let mut shaper = ready_shaper();
        shaper.cakes.insert(
            "ifb4eth1".into(),
            Cake {
                handle: DL,
                kbps: 25_500,
            },
        );
        shaper.store.borrow_mut().insert(
            "ifb4eth1".into(),
            Cake {
                handle: DL,
                kbps: 40_000,
            },
        );
        let applied = Rc::clone(&shaper.applied);
        let mut controller = Controller::new(test_config(), shaper);
        controller.step_at(Instant::now(), 1_000, &[("eth1".into(), 15_000)]);
        let rate = controller.status(Instant::now(), None).links[0].dl_kbps;
        assert!(
            rate > 25_500 && rate < 40_000,
            "idle drifts back toward 40000, not {rate}"
        );
        drop(controller);
        assert_eq!(
            applied.borrow().last(),
            Some(&("ifb4eth1".to_owned(), 40_000))
        );
    }

    #[test]
    fn a_rate_set_by_someone_else_becomes_the_original() {
        let shaper = ready_shaper();
        let applied = Rc::clone(&shaper.applied);
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        controller.shaper.cakes.insert(
            "ifb4eth1".into(),
            Cake {
                handle: DL,
                kbps: 60_000,
            },
        );
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 15_000)],
        );
        assert_eq!(controller.status(t0, None).links[0].dl_kbps, 60_000);
        drop(controller);
        assert_eq!(*applied.borrow(), []);
    }

    #[test]
    fn a_replaced_qdisc_is_not_restored_over() {
        let shaper = ready_shaper();
        let applied = Rc::clone(&shaper.applied);
        let mut controller = Controller::new(test_config(), shaper);
        let t0 = Instant::now();
        controller.step_at(t0, 1_000, &[("eth1".into(), 15_000)]);
        controller
            .shaper
            .bytes
            .insert("eth1".into(), (3_750_000, 10_000));
        controller.step_at(
            t0 + Duration::from_secs(1),
            2_000,
            &[("eth1".into(), 50_000)],
        );
        controller.shaper.cakes.insert(
            "ifb4eth1".into(),
            Cake {
                handle: 0x8009_0000,
                kbps: 40_000,
            },
        );
        drop(controller);
        assert_eq!(*applied.borrow(), [("ifb4eth1".to_owned(), 25_500)]);
    }

    #[test]
    fn netlink_newqdisc_encoding_sets_cake_base_rate64() -> io::Result<()> {
        let bytes = encode_set_cake_bandwidth(7, 0x8001_0000, 40_000, 99)?;
        let header = NetlinkBuffer::new_checked(&bytes).map_err(io::Error::other)?;
        assert_eq!(header.message_type(), RTM_NEWQDISC);
        assert_eq!(header.sequence_number(), 99);
        assert_eq!(usize::try_from(header.length()).unwrap_or(0), bytes.len());
        let (tcmsg, attrs) = header
            .payload()
            .split_at_checked(TCMSG_LEN)
            .ok_or_else(|| io::Error::other("missing tcmsg"))?;
        assert_eq!(
            i32::from_ne_bytes([tcmsg[4], tcmsg[5], tcmsg[6], tcmsg[7]]),
            7
        );
        assert_eq!(
            u32::from_ne_bytes([tcmsg[8], tcmsg[9], tcmsg[10], tcmsg[11]]),
            0x8001_0000
        );
        assert_eq!(
            u32::from_ne_bytes([tcmsg[12], tcmsg[13], tcmsg[14], tcmsg[15]]),
            TC_H_ROOT
        );

        let mut saw_kind = false;
        let mut saw_rate = None;
        for attr in NlasIterator::new(attrs) {
            let attr = attr.map_err(io::Error::other)?;
            if attr.kind() == TCA_KIND {
                saw_kind = attr.value() == b"cake\0";
            } else if attr.kind() == TCA_OPTIONS && attr.nested_flag() {
                for inner in NlasIterator::new(attr.value()) {
                    let inner = inner.map_err(io::Error::other)?;
                    if inner.kind() == TCA_CAKE_BASE_RATE64 {
                        let raw: [u8; 8] = inner
                            .value()
                            .try_into()
                            .map_err(|_| io::Error::other("bad rate64 len"))?;
                        saw_rate = Some(u64::from_ne_bytes(raw));
                    }
                }
            }
        }
        assert!(saw_kind);
        assert_eq!(saw_rate, Some(40_000 * 125));
        Ok(())
    }

    #[test]
    fn netlink_qdisc_dump_reads_the_cake_rate() -> io::Result<()> {
        let mut options = [0_u8; 16];
        let pad = write_nla(&mut options, 1, &[])?;
        let rate = write_nla(
            &mut options[pad..],
            TCA_CAKE_BASE_RATE64,
            &(50_000_u64 * 125).to_ne_bytes(),
        )?;
        let mut attrs = [0_u8; 32];
        let kind = write_nla(&mut attrs, TCA_KIND, b"cake\0")?;
        let nested = write_nla(&mut attrs[kind..], TCA_OPTIONS, &options[..pad + rate])?;
        let msg_len = NLMSG_HEADER_LEN + TCMSG_LEN + kind + nested;
        let mut dump = vec![0_u8; msg_len];
        {
            let mut header = NetlinkBuffer::new(&mut dump[..]);
            header.set_length(u32::try_from(msg_len).map_err(io::Error::other)?);
            header.set_message_type(RTM_NEWQDISC);
            header.set_sequence_number(1);
        }
        write_tcmsg(
            &mut dump[NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + TCMSG_LEN],
            3,
            0x8001_0000,
            TC_H_ROOT,
        );
        dump[NLMSG_HEADER_LEN + TCMSG_LEN..].copy_from_slice(&attrs[..kind + nested]);
        assert_eq!(
            decode_qdisc_dump(&dump, 3, 1),
            DumpOutcome::Found(Cake {
                handle: 0x8001_0000,
                kbps: 50_000,
            })
        );
        Ok(())
    }

    #[test]
    fn netlink_qdisc_dump_and_ack_decoding() -> io::Result<()> {
        let kind_len = align4(4 + 5);
        let msg_len = NLMSG_HEADER_LEN + TCMSG_LEN + kind_len;
        let mut dump = vec![0_u8; msg_len + NLMSG_HEADER_LEN];
        {
            let mut header = NetlinkBuffer::new(&mut dump[..msg_len]);
            header.set_length(u32::try_from(msg_len).map_err(io::Error::other)?);
            header.set_message_type(RTM_NEWQDISC);
            header.set_sequence_number(1);
            write_tcmsg(
                &mut dump[NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + TCMSG_LEN],
                5,
                0x8002_0000,
                TC_H_ROOT,
            );
            write_nla(
                &mut dump[NLMSG_HEADER_LEN + TCMSG_LEN..],
                TCA_KIND,
                b"cake\0",
            )?;
        }
        {
            let mut done = NetlinkBuffer::new(&mut dump[msg_len..]);
            done.set_length(u32::try_from(NLMSG_HEADER_LEN).map_err(io::Error::other)?);
            done.set_message_type(NLMSG_DONE);
            done.set_sequence_number(1);
        }
        assert_eq!(
            decode_qdisc_dump(&dump, 5, 1),
            DumpOutcome::Found(Cake {
                handle: 0x8002_0000,
                kbps: 0,
            })
        );
        assert_eq!(decode_qdisc_dump(&dump, 9, 1), DumpOutcome::Done);

        let mut ack = vec![0_u8; NLMSG_HEADER_LEN + 4];
        let ack_len = u32::try_from(ack.len()).map_err(io::Error::other)?;
        {
            let mut header = NetlinkBuffer::new(&mut ack[..]);
            header.set_length(ack_len);
            header.set_message_type(NLMSG_ERROR);
            header.set_sequence_number(2);
        }
        assert!(matches!(decode_netlink_ack(&ack, 2), Some(Ok(()))));

        ack[NLMSG_HEADER_LEN..].copy_from_slice(&(-22_i32).to_ne_bytes());
        assert!(matches!(
            decode_netlink_ack(&ack, 2),
            Some(Err(err)) if err.raw_os_error() == Some(22)
        ));
        let _ = (NLA_F_NESTED, NlaBuffer::new(&[0; 4]));
        Ok(())
    }
}
