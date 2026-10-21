use monoio::net::udp::UdpSocket;
use socket2::Socket;
use std::{io, ops::Deref};

pub struct RegisteredSocket {
    raw: Socket,
    pub ready: UdpSocket,
}

impl RegisteredSocket {
    pub fn new(raw: Socket) -> io::Result<Self> {
        let duplicate = std::net::UdpSocket::from(raw.try_clone()?);
        let ready = UdpSocket::from_std(duplicate)?;
        Ok(Self { raw, ready })
    }
}

impl Deref for RegisteredSocket {
    type Target = Socket;

    fn deref(&self) -> &Socket {
        &self.raw
    }
}
