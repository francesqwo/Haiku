//! online: the code a host gives the player it invites.
//!
//! It holds what the joiner needs to reach the host and prove it was
//! invited, and nothing else:
//!
//! - the host's port (where it listens on its own network);
//! - the host's public IPv4 address, when it learned one (a router's port
//!   mapping, or STUN), and the public port when the router gave another;
//! - an address on the host's own network (IPv4), when it has no public
//!   one, so players on one network (or one PC) still meet;
//! - the host's IPv6 address, when it has a global one and no mapped IPv4;
//! - a 48-bit secret, made fresh for each session: it names the session
//!   and keys its encryption and authentication (see transport.rs);
//! - a CRC-16 of all of the above, so a mistyped code is caught at once.
//!
//! The bytes are written in Crockford's base32 (0-9 and the letters but
//! I, L, O and U), in groups of four joined by dashes. Reading is
//! case-insensitive, ignores dashes and spaces, and takes O for 0 and I
//! or L for 1, so what a player reads out or types is what was meant.
//! A typical code is 24 characters: `ABCD-EFGH-JKMN-PQRS-TVWX-YZ01`.

use std::net::{Ipv4Addr, Ipv6Addr};

/// the code layout's version, in the first byte's top three bits.
const LAYOUT: u8 = 1;

const HAS_PUBLIC4: u8 = 1;
const HAS_PUBLIC_PORT: u8 = 2;
const HAS_LAN4: u8 = 4;
const HAS_V6: u8 = 8;

pub const SECRET_LEN: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// the port the host listens on, on its own network.
    pub port: u16,
    /// its public IPv4 address, and the public port when it differs.
    pub public4: Option<Ipv4Addr>,
    pub public_port: Option<u16>,
    /// an IPv4 address on its own network.
    pub lan4: Option<Ipv4Addr>,
    pub v6: Option<Ipv6Addr>,
    pub secret: [u8; SECRET_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    /// nothing typed, or far too little.
    Empty,
    /// a character that is not in any code.
    Character(char),
    /// the wrong length, or the check does not match: a typo.
    Mistyped,
    /// a layout from another version of Haiku.
    Layout,
}

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// CRC-16/CCITT-FALSE.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in bytes {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn encode32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn value32(c: char) -> Option<u8> {
    let c = c.to_ascii_uppercase();
    let c = match c {
        'O' => '0',
        'I' | 'L' => '1',
        c => c,
    };
    ALPHABET.iter().position(|&a| a as char == c).map(|v| v as u8)
}

fn decode32(text: &str) -> Result<Vec<u8>, CodeError> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let value = value32(c).ok_or(CodeError::Character(c))?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

impl Code {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut flags = 0;
        let mut body = Vec::with_capacity(32);
        body.extend_from_slice(&self.port.to_be_bytes());
        if let Some(ip) = self.public4 {
            flags |= HAS_PUBLIC4;
            body.extend_from_slice(&ip.octets());
            if let Some(port) = self.public_port.filter(|&port| port != self.port) {
                flags |= HAS_PUBLIC_PORT;
                body.extend_from_slice(&port.to_be_bytes());
            }
        }
        if let Some(ip) = self.lan4 {
            flags |= HAS_LAN4;
            body.extend_from_slice(&ip.octets());
        }
        if let Some(ip) = self.v6 {
            flags |= HAS_V6;
            body.extend_from_slice(&ip.octets());
        }
        body.extend_from_slice(&self.secret);
        let mut bytes = vec![LAYOUT << 5 | flags];
        bytes.extend_from_slice(&body);
        let crc = crc16(&bytes);
        bytes.extend_from_slice(&crc.to_be_bytes());
        bytes
    }

    /// the code as a player reads it: groups of four, joined by dashes.
    pub fn text(&self) -> String {
        let plain = encode32(&self.to_bytes());
        plain.as_bytes().chunks(4).map(|chunk| std::str::from_utf8(chunk).unwrap()).collect::<Vec<_>>().join("-")
    }

    pub fn parse(text: &str) -> Result<Code, CodeError> {
        let bytes = decode32(text)?;
        if bytes.len() < 4 {
            return Err(if bytes.is_empty() { CodeError::Empty } else { CodeError::Mistyped });
        }
        let (data, check) = bytes.split_at(bytes.len() - 2);
        // base32 pads to whole characters: a code is exactly its bytes and
        // the few bits over, so a longer one is a typo too
        if crc16(data) != u16::from_be_bytes([check[0], check[1]]) {
            return Err(CodeError::Mistyped);
        }
        if data[0] >> 5 != LAYOUT {
            return Err(CodeError::Layout);
        }
        let flags = data[0] & 31;
        let mut at = 1;
        let mut take = |n: usize| -> Result<&[u8], CodeError> {
            let part = data.get(at..at + n).ok_or(CodeError::Mistyped)?;
            at += n;
            Ok(part)
        };
        let port = u16::from_be_bytes(take(2)?.try_into().unwrap());
        let mut code = Code { port, public4: None, public_port: None, lan4: None, v6: None, secret: [0; SECRET_LEN] };
        if flags & HAS_PUBLIC4 != 0 {
            let ip: [u8; 4] = take(4)?.try_into().unwrap();
            code.public4 = Some(Ipv4Addr::from(ip));
            if flags & HAS_PUBLIC_PORT != 0 {
                code.public_port = Some(u16::from_be_bytes(take(2)?.try_into().unwrap()));
            }
        }
        if flags & HAS_LAN4 != 0 {
            let ip: [u8; 4] = take(4)?.try_into().unwrap();
            code.lan4 = Some(Ipv4Addr::from(ip));
        }
        if flags & HAS_V6 != 0 {
            let ip: [u8; 16] = take(16)?.try_into().unwrap();
            code.v6 = Some(Ipv6Addr::from(ip));
        }
        code.secret = take(SECRET_LEN)?.try_into().unwrap();
        if at != data.len() {
            return Err(CodeError::Mistyped);
        }
        Ok(code)
    }

    /// where the joiner tries to reach the host, best first.
    pub fn endpoints(&self) -> Vec<std::net::SocketAddr> {
        use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
        let mut out = Vec::new();
        if let Some(ip) = self.lan4 {
            out.push(SocketAddr::V4(SocketAddrV4::new(ip, self.port)));
        }
        if let Some(ip) = self.public4 {
            out.push(SocketAddr::V4(SocketAddrV4::new(ip, self.public_port.unwrap_or(self.port))));
            // the router may not map the port back for its own network:
            // the host's own port on the public address as well
            if self.public_port.is_some_and(|p| p != self.port) {
                out.push(SocketAddr::V4(SocketAddrV4::new(ip, self.port)));
            }
        }
        if let Some(ip) = self.v6 {
            out.push(SocketAddr::V6(SocketAddrV6::new(ip, self.port, 0, 0)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Code {
        Code {
            port: 24874,
            public4: Some(Ipv4Addr::new(203, 0, 113, 7)),
            public_port: None,
            lan4: None,
            v6: None,
            secret: [1, 2, 3, 4, 5, 6],
        }
    }

    #[test]
    fn a_code_reads_back_whatever_the_case_and_dashes() {
        let code = sample();
        let text = code.text();
        assert_eq!(text.replace('-', "").len(), 24, "{text}");
        assert_eq!(Code::parse(&text).unwrap(), code);
        assert_eq!(Code::parse(&text.to_lowercase().replace('-', " ")).unwrap(), code);
        // O and I typed for 0 and 1
        let swapped: String = text.chars().map(|c| match c {
            '0' => 'O',
            '1' => 'I',
            c => c,
        }).collect();
        assert_eq!(Code::parse(&swapped).unwrap(), code);
    }

    #[test]
    fn every_layout_reads_back() {
        let mut code = sample();
        code.public_port = Some(40000);
        code.lan4 = Some(Ipv4Addr::new(192, 168, 1, 20));
        code.v6 = Some("2001:db8::1".parse().unwrap());
        assert_eq!(Code::parse(&code.text()).unwrap(), code);
        let lan = Code { public4: None, lan4: Some(Ipv4Addr::LOCALHOST), ..sample() };
        assert_eq!(Code::parse(&lan.text()).unwrap(), lan);
    }

    #[test]
    fn a_typo_is_caught() {
        let text = sample().text();
        for i in 0..text.len() {
            let mut chars: Vec<char> = text.chars().collect();
            if chars[i] == '-' {
                continue;
            }
            chars[i] = if chars[i] == 'X' { 'Y' } else { 'X' };
            let typo: String = chars.into_iter().collect();
            assert!(Code::parse(&typo).is_err(), "{typo}");
        }
        assert_eq!(Code::parse(&text[..text.len() - 1]), Err(CodeError::Mistyped));
        assert_eq!(Code::parse("ABCU"), Err(CodeError::Character('U')));
        assert_eq!(Code::parse(""), Err(CodeError::Empty));
    }
}
