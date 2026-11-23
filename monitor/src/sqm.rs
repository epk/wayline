use crate::probe::{bucket_start, millis, unix_millis};
use crate::wifi::valid_device;
use netlink_packet_core::{NLM_F_REQUEST, NetlinkBuffer, NlaBuffer, NlasIterator};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read};
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

pub trait Shaper {
    fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)>;
    fn has_cake(&mut self, device: &str) -> bool;
    fn set_cake_bandwidth(&mut self, device: &str, kbps: u32) -> io::Result<()>;
}

pub struct LinuxShaper;

impl Shaper for LinuxShaper {
    fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)> {
        valid_device(device).then_some(())?;
        let rx = read_sysfs_u64(device, "rx_bytes")?;
        let tx = read_sysfs_u64(device, "tx_bytes")?;
        Some((rx, tx))
    }

    fn has_cake(&mut self, device: &str) -> bool {
        read_ifindex(device).and_then(cake_handle).is_some()
    }

    fn set_cake_bandwidth(&mut self, device: &str, kbps: u32) -> io::Result<()> {
        let ifindex = read_ifindex(device)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown network device"))?;
        let handle = cake_handle(ifindex)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no root cake qdisc"))?;
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

fn cake_handle(ifindex: i32) -> Option<u32> {
    let socket = open_netlink().ok()?;
    let request = encode_get_qdisc(ifindex, 1);
    if socket.send(&request).ok()? != request.len() {
        return None;
    }
    let mut buffer = [0_u8; 8192];
    for _ in 0..MAX_DUMP_READS {
        let size = (&socket).read(&mut buffer).ok()?;
        match decode_qdisc_dump(&buffer[..size], ifindex, 1) {
            DumpOutcome::Found(handle) => return Some(handle),
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
    Found(u32),
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
                    if let Some(handle) = matching_cake_handle(message.payload(), ifindex) {
                        return DumpOutcome::Found(handle);
                    }
                }
                _ => (),
            }
        }
        offset += align4(length);
    }
    DumpOutcome::Continue
}

fn matching_cake_handle(payload: &[u8], target_ifindex: i32) -> Option<u32> {
    let (head, attrs) = payload.split_at_checked(TCMSG_LEN)?;
    let ifindex = i32::from_ne_bytes([head[4], head[5], head[6], head[7]]);
    let handle = u32::from_ne_bytes([head[8], head[9], head[10], head[11]]);
    let parent = u32::from_ne_bytes([head[12], head[13], head[14], head[15]]);
    if ifindex != target_ifindex || parent != TC_H_ROOT {
        return None;
    }
    for attr in NlasIterator::new(attrs) {
        let attr = attr.ok()?;
        if attr.kind() == TCA_KIND && (attr.value() == b"cake\0" || attr.value() == b"cake") {
            return Some(handle);
        }
    }
    None
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
    applied_kbps: Option<u32>,
    action: SqmAction,
    last_decrease: Option<Instant>,
}

impl DirectionState {
    const fn new(base_kbps: u32) -> Self {
        Self {
            rate_kbps: base_kbps,
            applied_kbps: None,
            action: SqmAction::Idle,
            last_decrease: None,
        }
    }
}

struct DirectionBounds {
    min: u32,
    base: u32,
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
    const fn new(iface: SqmInterface, config: &SqmConfig) -> Self {
        Self {
            iface,
            dl: DirectionState::new(config.base_dl_kbps),
            ul: DirectionState::new(config.base_ul_kbps),
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
}

impl<S: Shaper> Controller<S> {
    #[must_use]
    pub fn new(config: SqmConfig, shaper: S) -> Self {
        let links = config
            .interfaces
            .iter()
            .cloned()
            .map(|iface| LinkState::new(iface, &config))
            .collect();
        Self {
            config,
            shaper,
            links,
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
            base: self.config.base_dl_kbps,
            max: self.config.max_dl_kbps,
        };
        let ul_bounds = DirectionBounds {
            min: self.config.min_ul_kbps,
            base: self.config.base_ul_kbps,
            max: self.config.max_ul_kbps,
        };
        for link in &mut self.links {
            let (rx_kbps, tx_kbps) = measure_throughput(&mut self.shaper, link, now);
            link.rx_kbps = rx_kbps;
            link.tx_kbps = tx_kbps;
            let Some(rtt_us) = match_rtt(&link.iface, rtts) else {
                link.delta_us = None;
                link.dl.action = SqmAction::Hold;
                link.ul.action = SqmAction::Hold;
                continue;
            };
            link.last_updated = Some(now);
            let base = update_baseline(link, rtt_us, rx_kbps, tx_kbps);
            let delta_us = i64::from(rtt_us) - i64::from(base);
            link.delta_us = Some(delta_us);

            let dl_dev = link.iface.ingress_device.clone();
            let ul_dev = link.iface.egress_device.clone();
            let dl_changed = apply_direction(
                &mut self.shaper,
                &dl_dev,
                &mut link.dl,
                rx_kbps,
                delta_us,
                threshold_us,
                &dl_bounds,
                now,
            );
            if dl_changed {
                link.adjustments = link.adjustments.saturating_add(1);
            }
            let ul_changed = apply_direction(
                &mut self.shaper,
                &ul_dev,
                &mut link.ul,
                tx_kbps,
                delta_us,
                threshold_us,
                &ul_bounds,
                now,
            );
            if ul_changed {
                link.adjustments = link.adjustments.saturating_add(1);
            }
            link.record_bucket(wall_ms);
        }
    }

    #[must_use]
    pub fn status(&self, now: Instant, since: Option<u64>) -> SqmStatus {
        let active = self.links.iter().any(|link| {
            !matches!(link.dl.action, SqmAction::MissingQdisc | SqmAction::Error)
                || !matches!(link.ul.action, SqmAction::MissingQdisc | SqmAction::Error)
        });
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

fn decide_rate(
    dir: &DirectionState,
    rate_kbps: u32,
    delta_us: i64,
    threshold_us: i64,
    bounds: &DirectionBounds,
    now: Instant,
) -> (u32, SqmAction) {
    let shaper = u64::from(dir.rate_kbps);
    let rate = u64::from(rate_kbps);
    let in_cooldown = dir
        .last_decrease
        .is_some_and(|at| now.saturating_duration_since(at) < COOLDOWN);
    if delta_us > threshold_us {
        let under_load = rate >= shaper * 15 / 100 || rate_kbps >= bounds.min;
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
        if dir.rate_kbps > bounds.base {
            let step = (dir.rate_kbps - bounds.base).max(500) / 20;
            let next = dir.rate_kbps.saturating_sub(step).max(bounds.base);
            (next, SqmAction::Idle)
        } else {
            (dir.rate_kbps, SqmAction::Idle)
        }
    } else {
        (dir.rate_kbps, SqmAction::Hold)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "direction step passes isolated bounds"
)]
fn apply_direction<S: Shaper>(
    shaper: &mut S,
    device: &str,
    dir: &mut DirectionState,
    rate_kbps: u32,
    delta_us: i64,
    threshold_us: i64,
    bounds: &DirectionBounds,
    now: Instant,
) -> bool {
    let (next_kbps, action) = decide_rate(dir, rate_kbps, delta_us, threshold_us, bounds, now);
    if !shaper.has_cake(device) {
        dir.applied_kbps = None;
        dir.action = SqmAction::MissingQdisc;
        return false;
    }
    let should_apply = dir
        .applied_kbps
        .is_none_or(|applied| next_kbps.abs_diff(applied) >= (applied / 50).max(500));
    let adjusted = if should_apply {
        if shaper.set_cake_bandwidth(device, next_kbps).is_err() {
            dir.action = SqmAction::Error;
            return false;
        }
        dir.applied_kbps = Some(next_kbps);
        true
    } else {
        false
    };
    dir.rate_kbps = next_kbps;
    dir.action = action;
    if action == SqmAction::Decrease {
        dir.last_decrease = Some(now);
    }
    adjusted
}

#[cfg(test)]
mod tests {
    use super::{
        Controller, DumpOutcome, NLA_F_NESTED, NLMSG_DONE, NLMSG_ERROR, NLMSG_HEADER_LEN,
        RTM_NEWQDISC, Shaper, TC_H_ROOT, TCA_CAKE_BASE_RATE64, TCA_KIND, TCA_OPTIONS, TCMSG_LEN,
        align4, decode_netlink_ack, decode_qdisc_dump, encode_set_cake_bandwidth, write_nla,
        write_tcmsg,
    };
    use netlink_packet_core::{NetlinkBuffer, NlaBuffer, NlasIterator};
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::time::{Duration, Instant};
    use wayline_protocol::{SqmAction, SqmConfig, SqmInterface};

    #[derive(Default)]
    struct FakeShaper {
        bytes: BTreeMap<String, (u64, u64)>,
        cake_devices: BTreeSet<String>,
        fail_set: BTreeSet<String>,
        applied: Vec<(String, u32)>,
    }

    impl Shaper for FakeShaper {
        fn read_bytes(&mut self, device: &str) -> Option<(u64, u64)> {
            self.bytes.get(device).copied()
        }

        fn has_cake(&mut self, device: &str) -> bool {
            self.cake_devices.contains(device)
        }

        fn set_cake_bandwidth(&mut self, device: &str, kbps: u32) -> io::Result<()> {
            if self.fail_set.contains(device) {
                return Err(io::Error::other("simulated netlink failure"));
            }
            self.applied.push((device.to_owned(), kbps));
            Ok(())
        }
    }

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
            base_dl_kbps: 40_000,
            max_dl_kbps: 100_000,
            min_ul_kbps: 5_000,
            base_ul_kbps: 20_000,
            max_ul_kbps: 50_000,
        }
    }

    fn ready_shaper() -> FakeShaper {
        let mut shaper = FakeShaper::default();
        shaper.cake_devices.insert("eth1".into());
        shaper.cake_devices.insert("ifb4eth1".into());
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
            DumpOutcome::Found(0x8002_0000)
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
