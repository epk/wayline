use rustix::event::{self, PollFd, PollFlags, Timespec};
use rustix::fd::{AsFd, OwnedFd};
use rustix::io::Errno;
use rustix::net::netlink::{self, SocketAddrNetlink};
use rustix::net::sockopt::{self, Timeout};
use rustix::net::{AddressFamily, RecvFlags, SendFlags, SocketType};
use std::io;
use std::time::Duration;
use wayline_protocol::Rate;

const NLMSG_HEADER: usize = 16;
const GENL_HEADER: usize = 4;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP: u16 = 0x300;
const NLA_TYPE_MASK: u16 = 0x3fff;
const RECEIVE: usize = 64 * 1024;
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

const GENL_ID_CTRL: u16 = 16;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_MCAST_GRP_NAME: u16 = 1;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;

const CMD_GET_INTERFACE: u8 = 5;
const CMD_GET_STATION: u8 = 17;
const CMD_GET_SCAN: u8 = 32;
const CMD_CONNECT: u8 = 46;
const CMD_ROAM: u8 = 47;
const CMD_DISCONNECT: u8 = 48;
const CMD_GET_SURVEY: u8 = 50;

const ATTR_WIPHY: u16 = 1;
const ATTR_IFINDEX: u16 = 3;
const ATTR_IFNAME: u16 = 4;
const ATTR_IFTYPE: u16 = 5;
const ATTR_MAC: u16 = 6;
const ATTR_STA_INFO: u16 = 21;
const ATTR_WIPHY_FREQ: u16 = 38;
const ATTR_BSS: u16 = 47;
const ATTR_SSID: u16 = 52;
const ATTR_SURVEY_INFO: u16 = 84;
const ATTR_WIPHY_TX_POWER_LEVEL: u16 = 98;
const ATTR_CHANNEL_WIDTH: u16 = 159;

const IFTYPE_STATION: u32 = 2;

const STA_INACTIVE_TIME: u16 = 1;
const STA_RX_BYTES: u16 = 2;
const STA_TX_BYTES: u16 = 3;
const STA_SIGNAL: u16 = 7;
const STA_TX_BITRATE: u16 = 8;
const STA_RX_PACKETS: u16 = 9;
const STA_TX_PACKETS: u16 = 10;
const STA_TX_RETRIES: u16 = 11;
const STA_TX_FAILED: u16 = 12;
const STA_SIGNAL_AVG: u16 = 13;
const STA_RX_BITRATE: u16 = 14;
const STA_CONNECTED_TIME: u16 = 16;
const STA_BEACON_LOSS: u16 = 18;
const STA_RX_BYTES64: u16 = 23;
const STA_TX_BYTES64: u16 = 24;
const STA_CHAIN_SIGNAL: u16 = 25;
const STA_EXPECTED_THROUGHPUT: u16 = 27;
const STA_RX_DURATION: u16 = 32;
const STA_TX_DURATION: u16 = 39;

const RATE_BITRATE: u16 = 1;
const RATE_MCS: u16 = 2;
const RATE_40_MHZ: u16 = 3;
const RATE_SHORT_GI: u16 = 4;
const RATE_BITRATE32: u16 = 5;
const RATE_VHT_MCS: u16 = 6;
const RATE_VHT_NSS: u16 = 7;
const RATE_80_MHZ: u16 = 8;
const RATE_80P80_MHZ: u16 = 9;
const RATE_160_MHZ: u16 = 10;
const RATE_10_MHZ: u16 = 11;
const RATE_5_MHZ: u16 = 12;
const RATE_HE_MCS: u16 = 13;
const RATE_HE_NSS: u16 = 14;
const RATE_320_MHZ: u16 = 18;
const RATE_EHT_MCS: u16 = 19;
const RATE_EHT_NSS: u16 = 20;

const SURVEY_FREQUENCY: u16 = 1;
const SURVEY_NOISE: u16 = 2;
const SURVEY_IN_USE: u16 = 3;
const SURVEY_TIME: u16 = 4;
const SURVEY_TIME_BUSY: u16 = 5;
const SURVEY_TIME_RX: u16 = 7;
const SURVEY_TIME_TX: u16 = 8;
const SURVEY_TIME_BSS_RX: u16 = 11;

const BSS_BSSID: u16 = 1;
const BSS_FREQUENCY: u16 = 2;
const BSS_INFORMATION_ELEMENTS: u16 = 6;
const BSS_STATUS: u16 = 9;
const BSS_STATUS_ASSOCIATED: u32 = 1;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTMGRP_LINK: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interface {
    pub index: u32,
    pub name: String,
    pub station: bool,
    pub mac: String,
    pub ssid: String,
    pub frequency: Option<u32>,
    pub width: Option<u16>,
    pub txpower: Option<i16>,
    pub wiphy: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub mac: String,
    pub signal: Option<i16>,
    pub signal_avg: Option<i16>,
    pub chains: Vec<i16>,
    pub inactive_ms: Option<u64>,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_packets: Option<u64>,
    pub tx_packets: Option<u64>,
    pub tx_retries: Option<u64>,
    pub tx_failed: Option<u64>,
    pub beacon_loss: Option<u64>,
    pub connected_seconds: Option<u64>,
    pub expected_kbps: Option<u32>,
    pub rx_airtime_us: Option<u64>,
    pub tx_airtime_us: Option<u64>,
    pub tx: Rate,
    pub rx: Rate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub frequency: u32,
    pub noise: Option<i16>,
    pub in_use: bool,
    pub active_ms: Option<u64>,
    pub busy_ms: Option<u64>,
    pub rx_ms: Option<u64>,
    pub tx_ms: Option<u64>,
    pub bss_rx_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bss {
    pub bssid: String,
    pub frequency: Option<u32>,
    pub ssid: Option<String>,
    pub security: Option<String>,
}

pub struct Attributes<'data> {
    rest: &'data [u8],
}

impl<'data> Iterator for Attributes<'data> {
    type Item = (u16, &'data [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let header = self.rest.get(..4)?;
        let length = usize::from(u16::from_ne_bytes([header[0], header[1]]));
        let kind = u16::from_ne_bytes([header[2], header[3]]) & NLA_TYPE_MASK;
        if length < 4 {
            self.rest = &[];
            return None;
        }
        let payload = self.rest.get(4..length)?;
        self.rest = self.rest.get(aligned(length)..).unwrap_or(&[]);
        Some((kind, payload))
    }
}

pub const fn attributes(data: &[u8]) -> Attributes<'_> {
    Attributes { rest: data }
}

const fn aligned(length: usize) -> usize {
    (length + 3) & !3
}

fn find(data: &[u8], kind: u16) -> Option<&[u8]> {
    attributes(data)
        .find(|(found, _)| *found == kind)
        .map(|(_, payload)| payload)
}

const fn u8_of(payload: &[u8]) -> Option<u8> {
    payload.first().copied()
}

fn u16_of(payload: &[u8]) -> Option<u16> {
    Some(u16::from_ne_bytes(payload.get(..2)?.try_into().ok()?))
}

fn u32_of(payload: &[u8]) -> Option<u32> {
    Some(u32::from_ne_bytes(payload.get(..4)?.try_into().ok()?))
}

fn i32_of(payload: &[u8]) -> Option<i32> {
    Some(i32::from_ne_bytes(payload.get(..4)?.try_into().ok()?))
}

fn u64_of(payload: &[u8]) -> Option<u64> {
    Some(u64::from_ne_bytes(payload.get(..8)?.try_into().ok()?))
}

fn dbm_of(payload: &[u8]) -> Option<i16> {
    let value = i16::from(i8::from_ne_bytes([u8_of(payload)?]));
    (-110..=-1).contains(&value).then_some(value)
}

fn text_of(payload: &[u8]) -> String {
    let end = payload
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(payload.len());
    String::from_utf8_lossy(&payload[..end]).into_owned()
}

pub fn mac_of(payload: &[u8]) -> Option<String> {
    let octets = payload.get(..6)?;
    if octets.iter().all(|octet| *octet == 0) || octets.iter().all(|octet| *octet == 0xff) {
        return None;
    }
    Some(
        octets
            .iter()
            .map(|octet| format!("{octet:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

const fn width_of(code: u32) -> Option<u16> {
    match code {
        0 | 1 => Some(20),
        2 => Some(40),
        3 | 4 => Some(80),
        5 => Some(160),
        6 => Some(5),
        7 => Some(10),
        13 => Some(320),
        _ => None,
    }
}

pub fn channel_of(frequency: u32) -> Option<u16> {
    let channel = match frequency {
        2_484 => 14,
        2_412..=2_472 => (frequency - 2_407) / 5,
        5_955..=7_115 => (frequency - 5_950) / 5,
        5_160..=5_885 => (frequency - 5_000) / 5,
        _ => return None,
    };
    u16::try_from(channel).ok()
}

pub fn parse_interface(data: &[u8]) -> Option<Interface> {
    let mut found = Interface::default();
    let mut index = None;
    for (kind, payload) in attributes(data) {
        match kind {
            ATTR_IFINDEX => index = u32_of(payload),
            ATTR_IFNAME => found.name = text_of(payload),
            ATTR_IFTYPE => found.station = u32_of(payload) == Some(IFTYPE_STATION),
            ATTR_MAC => found.mac = mac_of(payload).unwrap_or_default(),
            ATTR_SSID => {
                found.ssid = String::from_utf8_lossy(payload)
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect();
            }
            ATTR_WIPHY_FREQ => found.frequency = u32_of(payload),
            ATTR_CHANNEL_WIDTH => found.width = u32_of(payload).and_then(width_of),
            ATTR_WIPHY_TX_POWER_LEVEL => {
                found.txpower = i32_of(payload).and_then(|mbm| i16::try_from(mbm / 100).ok());
            }
            ATTR_WIPHY => found.wiphy = u32_of(payload),
            _ => {}
        }
    }
    found.index = index?;
    (!found.name.is_empty()).then_some(found)
}

pub fn parse_rate(data: &[u8]) -> Rate {
    let mut kbps = None;
    let mut legacy = None;
    let mut mode = "Legacy";
    let mut mcs = None;
    let mut nss = None;
    let mut mhz = None;
    let mut short_gi = None;
    for (kind, payload) in attributes(data) {
        match kind {
            RATE_BITRATE32 => kbps = u32_of(payload).map(|rate| rate.saturating_mul(100)),
            RATE_BITRATE => legacy = u16_of(payload).map(|rate| u32::from(rate) * 100),
            RATE_MCS => {
                mcs = u8_of(payload);
                mode = "HT";
            }
            RATE_VHT_MCS => {
                mcs = u8_of(payload);
                mode = "VHT";
            }
            RATE_VHT_NSS | RATE_HE_NSS | RATE_EHT_NSS => nss = u8_of(payload),
            RATE_HE_MCS => {
                mcs = u8_of(payload);
                mode = "HE";
            }
            RATE_EHT_MCS => {
                mcs = u8_of(payload);
                mode = "EHT";
            }
            RATE_40_MHZ => mhz = Some(40),
            RATE_80_MHZ | RATE_80P80_MHZ => mhz = Some(80),
            RATE_160_MHZ => mhz = Some(160),
            RATE_320_MHZ => mhz = Some(320),
            RATE_10_MHZ => mhz = Some(10),
            RATE_5_MHZ => mhz = Some(5),
            RATE_SHORT_GI => short_gi = Some(true),
            _ => {}
        }
    }
    if mode == "HT" && mcs.is_some() && nss.is_none() {
        nss = mcs.map(|index| index / 8 + 1);
        mcs = mcs.map(|index| index % 8);
    }
    let kbps = kbps.or(legacy).filter(|rate| *rate > 0);
    Rate {
        kbps,
        mode: kbps.map(|_| mode.to_owned()),
        mcs: mcs.filter(|_| mode != "Legacy"),
        nss,
        mhz: mhz.or_else(|| kbps.map(|_| 20)),
        short_gi: short_gi.or_else(|| kbps.map(|_| false)),
    }
}

pub fn parse_station(data: &[u8]) -> Option<Peer> {
    let mut peer = Peer {
        mac: mac_of(find(data, ATTR_MAC)?)?,
        ..Peer::default()
    };
    let info = find(data, ATTR_STA_INFO)?;
    let (mut rx32, mut tx32) = (None, None);
    for (kind, payload) in attributes(info) {
        match kind {
            STA_INACTIVE_TIME => peer.inactive_ms = u32_of(payload).map(u64::from),
            STA_RX_BYTES => rx32 = u32_of(payload).map(u64::from),
            STA_TX_BYTES => tx32 = u32_of(payload).map(u64::from),
            STA_RX_BYTES64 => peer.rx_bytes = u64_of(payload),
            STA_TX_BYTES64 => peer.tx_bytes = u64_of(payload),
            STA_SIGNAL => peer.signal = dbm_of(payload),
            STA_SIGNAL_AVG => peer.signal_avg = dbm_of(payload),
            STA_CHAIN_SIGNAL => {
                peer.chains = attributes(payload)
                    .filter_map(|(_, chain)| dbm_of(chain))
                    .collect();
            }
            STA_TX_BITRATE => peer.tx = parse_rate(payload),
            STA_RX_BITRATE => peer.rx = parse_rate(payload),
            STA_RX_PACKETS => peer.rx_packets = u32_of(payload).map(u64::from),
            STA_TX_PACKETS => peer.tx_packets = u32_of(payload).map(u64::from),
            STA_TX_RETRIES => peer.tx_retries = u32_of(payload).map(u64::from),
            STA_TX_FAILED => peer.tx_failed = u32_of(payload).map(u64::from),
            STA_BEACON_LOSS => peer.beacon_loss = u32_of(payload).map(u64::from),
            STA_CONNECTED_TIME => peer.connected_seconds = u32_of(payload).map(u64::from),
            STA_EXPECTED_THROUGHPUT => {
                peer.expected_kbps = u32_of(payload).filter(|rate| *rate > 0);
            }
            STA_RX_DURATION => peer.rx_airtime_us = u64_of(payload),
            STA_TX_DURATION => peer.tx_airtime_us = u64_of(payload),
            _ => {}
        }
    }
    peer.rx_bytes = peer.rx_bytes.or(rx32);
    peer.tx_bytes = peer.tx_bytes.or(tx32);
    Some(peer)
}

pub fn parse_survey(data: &[u8]) -> Option<Channel> {
    let info = find(data, ATTR_SURVEY_INFO)?;
    let mut channel = Channel::default();
    for (kind, payload) in attributes(info) {
        match kind {
            SURVEY_FREQUENCY => channel.frequency = u32_of(payload)?,
            SURVEY_NOISE => {
                channel.noise = dbm_of(payload).filter(|noise| (-120..=-50).contains(noise));
            }
            SURVEY_IN_USE => channel.in_use = true,
            SURVEY_TIME => channel.active_ms = u64_of(payload),
            SURVEY_TIME_BUSY => channel.busy_ms = u64_of(payload),
            SURVEY_TIME_RX => channel.rx_ms = u64_of(payload),
            SURVEY_TIME_TX => channel.tx_ms = u64_of(payload),
            SURVEY_TIME_BSS_RX => channel.bss_rx_ms = u64_of(payload),
            _ => {}
        }
    }
    (channel.frequency > 0).then_some(channel)
}

fn suites(body: &[u8]) -> Option<(Vec<u32>, &[u8])> {
    let count = usize::from(u16::from_le_bytes(body.get(..2)?.try_into().ok()?));
    let list = body.get(2..2 + count * 4)?;
    let found = list
        .as_chunks::<4>()
        .0
        .iter()
        .map(|suite| u32::from_be_bytes(*suite))
        .collect();
    Some((found, body.get(2 + count * 4..).unwrap_or(&[])))
}

fn akm_names(akms: &[u32]) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for akm in akms {
        let name = match akm {
            0x000f_ac01 | 0x000f_ac03 | 0x000f_ac05 | 0x0050_f201 => "EAP",
            0x000f_ac02 | 0x000f_ac04 | 0x000f_ac06 | 0x0050_f202 => "PSK",
            0x000f_ac08 | 0x000f_ac09 | 0x000f_ac18 | 0x000f_ac19 => "SAE",
            0x000f_ac12 => "OWE",
            _ => continue,
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

pub fn security_of(elements: &[u8]) -> Option<String> {
    let mut rsn = None;
    let mut wpa = None;
    let mut rest = elements;
    while let [id, length, body @ ..] = rest {
        let length = usize::from(*length);
        let Some(content) = body.get(..length) else {
            break;
        };
        match id {
            48 => {
                rsn = content
                    .get(2..)
                    .and_then(|after| after.get(4..))
                    .and_then(suites);
            }
            221 if content.starts_with(&[0x00, 0x50, 0xf2, 0x01]) => {
                wpa = content.get(10..).and_then(suites);
            }
            _ => {}
        }
        rest = body.get(length..).unwrap_or(&[]);
    }
    let akms = |found: Option<(Vec<u32>, &[u8])>| {
        found
            .and_then(|(_, after)| suites(after))
            .map(|(akms, _)| akms)
            .unwrap_or_default()
    };
    let rsn_akms = akms(rsn);
    if !rsn_akms.is_empty() {
        let names = akm_names(&rsn_akms);
        let version = match (names.contains(&"SAE"), names.iter().any(|n| *n != "SAE")) {
            (true, true) => "WPA2/3",
            (true, false) => "WPA3",
            _ => "WPA2",
        };
        return Some(format!("{version} {}", names.join("/")));
    }
    let wpa_akms = akms(wpa);
    if !wpa_akms.is_empty() {
        return Some(format!("WPA {}", akm_names(&wpa_akms).join("/")));
    }
    None
}

pub fn ssid_of(elements: &[u8]) -> Option<String> {
    match elements {
        [0, length, body @ ..] => {
            let ssid = body.get(..usize::from(*length))?;
            (!ssid.is_empty()).then(|| String::from_utf8_lossy(ssid).into_owned())
        }
        _ => None,
    }
}

pub fn parse_bss(data: &[u8]) -> Option<Bss> {
    let bss = find(data, ATTR_BSS)?;
    if find(bss, BSS_STATUS).and_then(u32_of) != Some(BSS_STATUS_ASSOCIATED) {
        return None;
    }
    let elements = find(bss, BSS_INFORMATION_ELEMENTS).unwrap_or(&[]);
    Some(Bss {
        bssid: mac_of(find(bss, BSS_BSSID)?)?,
        frequency: find(bss, BSS_FREQUENCY).and_then(u32_of),
        ssid: ssid_of(elements),
        security: security_of(elements),
    })
}

fn message(kind: u16, flags: u16, sequence: u32, command: u8, attrs: &[(u16, &[u8])]) -> Vec<u8> {
    let mut out = vec![0_u8; NLMSG_HEADER];
    out.extend_from_slice(&[command, 1, 0, 0]);
    for (attr, payload) in attrs {
        let length = 4 + payload.len();
        out.extend_from_slice(&u16::try_from(length).unwrap_or(u16::MAX).to_ne_bytes());
        out.extend_from_slice(&attr.to_ne_bytes());
        out.extend_from_slice(payload);
        out.resize(aligned(out.len()), 0);
    }
    let total = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[..4].copy_from_slice(&total.to_ne_bytes());
    out[4..6].copy_from_slice(&kind.to_ne_bytes());
    out[6..8].copy_from_slice(&(flags | NLM_F_REQUEST).to_ne_bytes());
    out[8..12].copy_from_slice(&sequence.to_ne_bytes());
    out
}

pub struct Frame<'data> {
    pub kind: u16,
    pub sequence: u32,
    pub body: &'data [u8],
}

pub fn frames(data: &[u8]) -> impl Iterator<Item = Frame<'_>> {
    let mut rest = data;
    std::iter::from_fn(move || {
        let header = rest.get(..NLMSG_HEADER)?;
        let length = usize::try_from(u32::from_ne_bytes(header[..4].try_into().ok()?)).ok()?;
        if length < NLMSG_HEADER {
            return None;
        }
        let frame = Frame {
            kind: u16::from_ne_bytes([header[4], header[5]]),
            sequence: u32::from_ne_bytes(header[8..12].try_into().ok()?),
            body: rest.get(NLMSG_HEADER..length)?,
        };
        rest = rest.get(aligned(length)..).unwrap_or(&[]);
        Some(frame)
    })
}

fn socket(protocol: Option<rustix::net::Protocol>, groups: u32) -> io::Result<OwnedFd> {
    let fd = rustix::net::socket(AddressFamily::NETLINK, SocketType::RAW, protocol)?;
    sockopt::set_socket_timeout(&fd, Timeout::Recv, Some(REPLY_TIMEOUT))?;
    rustix::net::bind(&fd, &SocketAddrNetlink::new(0, groups))?;
    Ok(fd)
}

pub struct Nl80211 {
    fd: OwnedFd,
    family: u16,
    mlme: Option<u32>,
    sequence: u32,
    buffer: Vec<u8>,
}

impl Nl80211 {
    pub fn open() -> io::Result<Self> {
        let fd = socket(Some(netlink::GENERIC), 0)?;
        let mut found = Self {
            fd,
            family: GENL_ID_CTRL,
            mlme: None,
            sequence: 0,
            buffer: vec![0; RECEIVE],
        };
        let mut family = None;
        let mut mlme = None;
        found.request(
            GENL_ID_CTRL,
            CTRL_CMD_GETFAMILY,
            0,
            &[(CTRL_ATTR_FAMILY_NAME, b"nl80211\0")],
            |body| {
                family = find(body, CTRL_ATTR_FAMILY_ID).and_then(u16_of);
                mlme = find(body, CTRL_ATTR_MCAST_GROUPS).and_then(|groups| {
                    attributes(groups).find_map(|(_, group)| {
                        (find(group, CTRL_ATTR_MCAST_GRP_NAME)
                            .map(text_of)
                            .as_deref()
                            == Some("mlme"))
                        .then(|| find(group, CTRL_ATTR_MCAST_GRP_ID).and_then(u32_of))
                        .flatten()
                    })
                });
            },
        )?;
        found.family = family.ok_or_else(|| io::Error::other("nl80211 is not available"))?;
        found.mlme = mlme;
        Ok(found)
    }

    pub const fn mlme_group(&self) -> Option<u32> {
        self.mlme
    }

    fn request(
        &mut self,
        family: u16,
        command: u8,
        flags: u16,
        attrs: &[(u16, &[u8])],
        mut each: impl FnMut(&[u8]),
    ) -> io::Result<()> {
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence;
        let request = message(family, flags, sequence, command, attrs);
        rustix::net::sendto(
            &self.fd,
            &request,
            SendFlags::empty(),
            &SocketAddrNetlink::new(0, 0),
        )?;
        let dump = flags & NLM_F_DUMP != 0;
        loop {
            let (_, size) = rustix::net::recv(&self.fd, &mut self.buffer[..], RecvFlags::empty())?;
            let data = self
                .buffer
                .get(..size.min(self.buffer.len()))
                .unwrap_or_default();
            for frame in frames(data) {
                if frame.sequence != sequence {
                    continue;
                }
                match frame.kind {
                    NLMSG_DONE => return Ok(()),
                    NLMSG_ERROR => {
                        let code = frame.body.get(..4).and_then(i32_of).unwrap_or(0);
                        return if code == 0 {
                            Ok(())
                        } else {
                            Err(io::Error::from_raw_os_error(-code))
                        };
                    }
                    _ => {
                        each(frame.body.get(GENL_HEADER..).unwrap_or_default());
                        if !dump {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    fn dump<T>(
        &mut self,
        command: u8,
        attrs: &[(u16, &[u8])],
        parse: impl Fn(&[u8]) -> Option<T>,
    ) -> io::Result<Vec<T>> {
        let mut found = Vec::new();
        self.request(self.family, command, NLM_F_DUMP, attrs, |body| {
            found.extend(parse(body));
        })?;
        Ok(found)
    }

    pub fn interfaces(&mut self) -> io::Result<Vec<Interface>> {
        self.dump(CMD_GET_INTERFACE, &[], parse_interface)
    }

    pub fn stations(&mut self, index: u32) -> io::Result<Vec<Peer>> {
        self.dump(
            CMD_GET_STATION,
            &[(ATTR_IFINDEX, &index.to_ne_bytes())],
            parse_station,
        )
    }

    pub fn survey(&mut self, index: u32) -> io::Result<Vec<Channel>> {
        self.dump(
            CMD_GET_SURVEY,
            &[(ATTR_IFINDEX, &index.to_ne_bytes())],
            parse_survey,
        )
    }

    pub fn associated(&mut self, index: u32) -> io::Result<Option<Bss>> {
        Ok(self
            .dump(
                CMD_GET_SCAN,
                &[(ATTR_IFINDEX, &index.to_ne_bytes())],
                parse_bss,
            )?
            .into_iter()
            .next())
    }
}

pub struct Events {
    links: Option<OwnedFd>,
    mlme: Option<OwnedFd>,
    buffer: Vec<u8>,
}

impl Events {
    pub fn open(mlme_group: Option<u32>) -> Self {
        let mlme = mlme_group
            .filter(|group| (1..=32).contains(group))
            .and_then(|group| socket(Some(netlink::GENERIC), 1 << (group - 1)).ok());
        Self {
            links: socket(None, RTMGRP_LINK).ok(),
            mlme,
            buffer: vec![0; RECEIVE],
        }
    }

    pub const fn live(&self) -> (bool, bool) {
        (self.links.is_some(), self.mlme.is_some())
    }

    pub fn wait(&mut self, timeout: Duration, watched: &[u32]) -> bool {
        let mut fds: Vec<PollFd<'_>> = self
            .links
            .iter()
            .chain(self.mlme.iter())
            .map(|fd| PollFd::new(fd, PollFlags::IN))
            .collect();
        if fds.is_empty() {
            std::thread::sleep(timeout);
            return false;
        }
        let spec = Timespec {
            tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(timeout.subsec_nanos()),
        };
        match event::poll(&mut fds, Some(&spec)) {
            Ok(0) | Err(Errno::INTR) => return false,
            Ok(_) => {}
            Err(_) => {
                std::thread::sleep(timeout);
                return false;
            }
        }
        let ready: Vec<bool> = fds.iter().map(|fd| !fd.revents().is_empty()).collect();
        drop(fds);
        let mut relevant = false;
        let mut index = 0;
        if let Some(fd) = &self.links {
            if ready.get(index).copied().unwrap_or(false) {
                relevant |= drain(fd, &mut self.buffer, |frame| link_event(&frame, watched));
            }
            index += 1;
        }
        if let Some(fd) = &self.mlme
            && ready.get(index).copied().unwrap_or(false)
        {
            relevant |= drain(fd, &mut self.buffer, |frame| mlme_event(&frame, watched));
        }
        relevant
    }
}

fn drain(fd: &impl AsFd, buffer: &mut [u8], relevant: impl Fn(Frame<'_>) -> bool) -> bool {
    let mut found = false;
    loop {
        match rustix::net::recv(fd, &mut buffer[..], RecvFlags::DONTWAIT) {
            Ok((_, size)) => {
                let data = buffer.get(..size.min(buffer.len())).unwrap_or_default();
                found |= frames(data).any(&relevant);
            }
            Err(_) => return found,
        }
    }
}

pub fn link_event(frame: &Frame<'_>, watched: &[u32]) -> bool {
    (frame.kind == RTM_NEWLINK || frame.kind == RTM_DELLINK)
        && frame
            .body
            .get(4..8)
            .and_then(i32_of)
            .and_then(|index| u32::try_from(index).ok())
            .is_some_and(|index| watched.is_empty() || watched.contains(&index))
}

pub fn mlme_event(frame: &Frame<'_>, watched: &[u32]) -> bool {
    let Some(&command) = frame.body.first() else {
        return false;
    };
    matches!(command, CMD_CONNECT | CMD_ROAM | CMD_DISCONNECT)
        && frame
            .body
            .get(GENL_HEADER..)
            .and_then(|attrs| find(attrs, ATTR_IFINDEX))
            .and_then(u32_of)
            .is_none_or(|index| watched.is_empty() || watched.contains(&index))
}

#[cfg(test)]
pub mod tests {
    use super::{
        Frame, attributes, channel_of, frames, link_event, message, mlme_event, parse_bss,
        parse_interface, parse_rate, parse_station, parse_survey, security_of,
    };

    pub fn attr(kind: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u16::try_from(4 + payload.len()).unwrap_or(0).to_ne_bytes());
        out.extend_from_slice(&kind.to_ne_bytes());
        out.extend_from_slice(payload);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out
    }

    pub fn nest(kind: u16, parts: &[Vec<u8>]) -> Vec<u8> {
        attr(kind | 0x8000, &parts.concat())
    }

    pub fn interface(index: u32, name: &str, station: bool, ssid: &str, frequency: u32) -> Vec<u8> {
        let mut parts = vec![
            attr(3, &index.to_ne_bytes()),
            attr(4, format!("{name}\0").as_bytes()),
            attr(5, &(if station { 2_u32 } else { 3 }).to_ne_bytes()),
            attr(6, &[0x92, 0x83, 0xc4, 0xd3, 0x6c, 0xda]),
            attr(38, &frequency.to_ne_bytes()),
            attr(159, &3_u32.to_ne_bytes()),
            attr(98, &2_200_i32.to_ne_bytes()),
        ];
        if !ssid.is_empty() {
            parts.push(attr(52, ssid.as_bytes()));
        }
        parts.concat()
    }

    pub fn station(mac: [u8; 6], packets: u32, retries: u32, connected: u32) -> Vec<u8> {
        let tx_rate = nest(
            8,
            &[
                attr(5, &6_485_u32.to_ne_bytes()),
                attr(13, &[6]),
                attr(14, &[2]),
                attr(8, &[]),
            ],
        );
        let rx_rate = nest(
            14,
            &[
                attr(5, &3_242_u32.to_ne_bytes()),
                attr(13, &[6]),
                attr(14, &[1]),
            ],
        );
        [
            attr(6, &mac),
            nest(
                21,
                &[
                    attr(1, &0_u32.to_ne_bytes()),
                    attr(23, &120_704_u64.to_ne_bytes()),
                    attr(24, &102_112_u64.to_ne_bytes()),
                    attr(7, &[(-68_i8).to_ne_bytes()[0]]),
                    nest(
                        25,
                        &[
                            attr(0, &[(-64_i8).to_ne_bytes()[0]]),
                            attr(1, &[(-72_i8).to_ne_bytes()[0]]),
                        ],
                    ),
                    attr(9, &533_u32.to_ne_bytes()),
                    attr(10, &packets.to_ne_bytes()),
                    attr(11, &retries.to_ne_bytes()),
                    attr(12, &96_u32.to_ne_bytes()),
                    attr(16, &connected.to_ne_bytes()),
                    attr(39, &906_593_u64.to_ne_bytes()),
                    tx_rate,
                    rx_rate,
                ],
            ),
        ]
        .concat()
    }

    pub fn survey(frequency: u32, in_use: bool, active: u64, busy: u64) -> Vec<u8> {
        let mut parts = vec![
            attr(1, &frequency.to_ne_bytes()),
            attr(2, &[(-92_i8).to_ne_bytes()[0]]),
            attr(4, &active.to_ne_bytes()),
            attr(5, &busy.to_ne_bytes()),
            attr(7, &(busy / 2).to_ne_bytes()),
            attr(8, &(busy / 4).to_ne_bytes()),
        ];
        if in_use {
            parts.push(attr(3, &[]));
        }
        nest(84, &parts)
    }

    #[test]
    fn interfaces_parse_from_nl80211_attributes() -> Result<(), &'static str> {
        let parsed = parse_interface(&interface(19, "apclii0", true, "candelaria200", 5_200))
            .ok_or("missing interface")?;
        assert_eq!(parsed.index, 19);
        assert_eq!(parsed.name, "apclii0");
        assert!(parsed.station);
        assert_eq!(parsed.mac, "92:83:c4:d3:6c:da");
        assert_eq!(parsed.ssid, "candelaria200");
        assert_eq!(parsed.frequency, Some(5_200));
        assert_eq!(parsed.width, Some(80));
        assert_eq!(parsed.txpower, Some(22));
        assert_eq!(channel_of(5_200), Some(40));
        assert_eq!(channel_of(2_437), Some(6));
        assert_eq!(channel_of(2_484), Some(14));
        assert_eq!(channel_of(5_975), Some(5));
        Ok(())
    }

    #[test]
    fn stations_parse_counters_signals_and_rates() -> Result<(), &'static str> {
        let peer = parse_station(&station([0x76, 0xfa, 0xce, 0x8a, 0xc4, 0xb3], 746, 96, 24))
            .ok_or("missing station")?;
        assert_eq!(peer.mac, "76:fa:ce:8a:c4:b3");
        assert_eq!(peer.signal, Some(-68));
        assert_eq!(peer.chains, [-64, -72]);
        assert_eq!(peer.rx_bytes, Some(120_704));
        assert_eq!(peer.tx_packets, Some(746));
        assert_eq!(peer.tx_retries, Some(96));
        assert_eq!(peer.tx_failed, Some(96));
        assert_eq!(peer.connected_seconds, Some(24));
        assert_eq!(peer.tx_airtime_us, Some(906_593));
        assert_eq!(peer.tx.kbps, Some(648_500));
        assert_eq!(peer.tx.mode.as_deref(), Some("HE"));
        assert_eq!(
            (peer.tx.mcs, peer.tx.nss, peer.tx.mhz),
            (Some(6), Some(2), Some(80))
        );
        assert_eq!(peer.rx.kbps, Some(324_200));
        assert_eq!(peer.rx.mhz, Some(20));
        let ht = parse_rate(
            &[
                super::tests::attr(1, &1_300_u16.to_ne_bytes()),
                super::tests::attr(2, &[15]),
            ]
            .concat(),
        );
        assert_eq!(
            (ht.mode.as_deref(), ht.mcs, ht.nss),
            (Some("HT"), Some(7), Some(2))
        );
        Ok(())
    }

    #[test]
    fn surveys_and_security_parse() -> Result<(), &'static str> {
        let channel =
            parse_survey(&survey(5_200, true, 83_703_836, 12_024_353)).ok_or("missing channel")?;
        assert!(channel.in_use);
        assert_eq!(channel.noise, Some(-92));
        assert_eq!(channel.busy_ms, Some(12_024_353));
        let rsn = |akms: &[u8]| {
            let mut body = vec![1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4];
            body.extend_from_slice(&u16::try_from(akms.len()).unwrap_or(0).to_le_bytes());
            for akm in akms {
                body.extend_from_slice(&[0x00, 0x0f, 0xac, *akm]);
            }
            let mut element = vec![48, u8::try_from(body.len()).unwrap_or(0)];
            element.extend(body);
            element
        };
        assert_eq!(security_of(&rsn(&[2])).as_deref(), Some("WPA2 PSK"));
        assert_eq!(security_of(&rsn(&[8])).as_deref(), Some("WPA3 SAE"));
        assert_eq!(
            security_of(&rsn(&[2, 8])).as_deref(),
            Some("WPA2/3 PSK/SAE")
        );
        assert_eq!(security_of(&[0, 3, b'a', b'b', b'c']), None);
        let mut elements = vec![0, 4];
        elements.extend_from_slice(b"Cafe");
        elements.extend(rsn(&[2]));
        let bss = super::tests::nest(
            47,
            &[
                attr(1, &[0x76, 0xfa, 0xce, 0x8a, 0xc4, 0xb3]),
                attr(2, &5_200_u32.to_ne_bytes()),
                attr(6, &elements),
                attr(9, &1_u32.to_ne_bytes()),
            ],
        );
        let parsed = parse_bss(&bss).ok_or("missing BSS")?;
        assert_eq!(parsed.ssid.as_deref(), Some("Cafe"));
        assert_eq!(parsed.security.as_deref(), Some("WPA2 PSK"));
        Ok(())
    }

    #[test]
    fn messages_frame_and_events_filter() {
        let request = message(0x1c, 0x300, 7, 17, &[(3, &19_u32.to_ne_bytes())]);
        let parsed: Vec<Frame<'_>> = frames(&request).collect();
        assert_eq!(parsed.len(), 1);
        assert_eq!((parsed[0].kind, parsed[0].sequence), (0x1c, 7));
        let attrs: Vec<(u16, &[u8])> = attributes(&parsed[0].body[4..]).collect();
        assert_eq!(attrs, [(3, &19_u32.to_ne_bytes()[..])]);
        let mut link = vec![0_u8; 4];
        link.extend_from_slice(&19_i32.to_ne_bytes());
        link.extend_from_slice(&[0; 8]);
        let frame = Frame {
            kind: 16,
            sequence: 0,
            body: &link,
        };
        assert!(link_event(&frame, &[19]));
        assert!(!link_event(&frame, &[6]));
        let mut connect = vec![46, 1, 0, 0];
        connect.extend(attr(3, &19_u32.to_ne_bytes()));
        let frame = Frame {
            kind: 0x1c,
            sequence: 0,
            body: &connect,
        };
        assert!(mlme_event(&frame, &[19]));
        assert!(!mlme_event(&frame, &[6]));
        connect[0] = 33;
        let frame = Frame {
            kind: 0x1c,
            sequence: 0,
            body: &connect,
        };
        assert!(!mlme_event(&frame, &[19]));
    }
}
