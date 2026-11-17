//! online: how the host finds the addresses its code gives, and asks its
//! router to let the joiner in.
//!
//! - **Its own network's address**: the IPv4 address the system would
//!   reach the Internet from (a UDP socket "connected" to a public address:
//!   nothing is sent).
//! - **IPv6**: the same for IPv6; a global address (2000::/3) can be
//!   reached directly where the other player has IPv6 too.
//! - **The router's port mapping**: UPnP IGD (SSDP to find the router,
//!   then its WANIPConnection's AddPortMapping / GetExternalIPAddress /
//!   DeletePortMapping), else NAT-PMP or PCP (RFC 6886 / 6887) to the
//!   gateway. The mapping lasts two hours and is removed when the session
//!   ends.
//! - **STUN** (RFC 5389 Binding request): the public address and port the
//!   host's own socket shows outside its NAT. Without a mapping that is the
//!   way in only through NATs that let anyone in once a mapping exists
//!   ("full cone"); it is still put in the code, as the best guess.
//!
//! Every step has a short timeout and runs on the transport's thread
//! before the code is shown; a step that fails is skipped.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// the IPv4 address this PC uses to reach the Internet, when it has one.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // a UDP "connect" only picks the route: nothing is sent
    socket.connect("192.0.2.1:9").or_else(|_| socket.connect("8.8.8.8:53")).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// a global IPv6 address of this PC's, when it has one.
pub fn global_ipv6() -> Option<Ipv6Addr> {
    let socket = UdpSocket::bind("[::]:0").ok()?;
    socket.connect("[2001:4860:4860::8888]:53").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V6(ip) if ip.segments()[0] & 0xE000 == 0x2000 => Some(ip),
        _ => None,
    }
}

pub fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || (ip.octets()[0] == 100 && ip.octets()[1] & 0xC0 == 64)
}

// -- STUN -------------------------------------------------------------------

/// the address and port `socket` shows to the Internet, asked of a STUN
/// server (host:port). The socket is the host's own, so the answer is its
/// mapping. Packets that are not the answer are left alone (none come
/// before the code is shown).
pub fn stun(socket: &UdpSocket, server: &str) -> Option<SocketAddrV4> {
    let address = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    let id: [u8; 12] = super::crypto::random();
    let mut request = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xA4, 0x42];
    request.extend_from_slice(&id);
    let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
    let mut buffer = [0u8; 576];
    for _ in 0..3 {
        socket.send_to(&request, address).ok()?;
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            let Ok((size, from)) = socket.recv_from(&mut buffer) else { break };
            if from != address || size < 20 || buffer[0..2] != [0x01, 0x01] || buffer[8..20] != id {
                continue;
            }
            if let Some(mapped) = mapped_address(&buffer[20..size]) {
                return Some(mapped);
            }
        }
    }
    None
}

fn mapped_address(mut attributes: &[u8]) -> Option<SocketAddrV4> {
    let mut plain = None;
    while attributes.len() >= 4 {
        let kind = u16::from_be_bytes([attributes[0], attributes[1]]);
        let len = u16::from_be_bytes([attributes[2], attributes[3]]) as usize;
        let value = attributes.get(4..4 + len)?;
        if value.len() >= 8 && value[1] == 0x01 {
            let port = u16::from_be_bytes([value[2], value[3]]);
            let ip = [value[4], value[5], value[6], value[7]];
            match kind {
                // XOR-MAPPED-ADDRESS, with the magic cookie
                0x0020 | 0x8020 => {
                    let port = port ^ 0x2112;
                    let ip = [ip[0] ^ 0x21, ip[1] ^ 0x12, ip[2] ^ 0xA4, ip[3] ^ 0x42];
                    return Some(SocketAddrV4::new(Ipv4Addr::from(ip), port));
                }
                // MAPPED-ADDRESS, from older servers
                0x0001 => plain = Some(SocketAddrV4::new(Ipv4Addr::from(ip), port)),
                _ => {}
            }
        }
        attributes = attributes.get(4 + (len + 3) / 4 * 4..).unwrap_or_default();
    }
    plain
}

// -- the router's port mapping ------------------------------------------------

/// a port the router forwards to this PC, removed with `release`.
#[derive(Debug, Clone)]
pub enum Mapping {
    Upnp { control: String, service: String, external_port: u16 },
    NatPmp { gateway: Ipv4Addr, internal_port: u16 },
}

/// what the router answered: its public address and the port it opened.
#[derive(Debug, Clone)]
pub struct Mapped {
    pub public: Option<Ipv4Addr>,
    pub port: u16,
    pub how: Mapping,
}

/// asks the router to forward UDP `port` to `lan`:`port`, by UPnP then
/// NAT-PMP/PCP.
pub fn map_port(lan: Ipv4Addr, port: u16, upnp: bool, natpmp: bool) -> Option<Mapped> {
    let mut gateway_hint = None;
    if upnp {
        match upnp_map(lan, port) {
            Ok(mapped) => return Some(mapped),
            Err((error, gateway)) => {
                log::info!("online: UPnP: {error}");
                gateway_hint = gateway;
            }
        }
    }
    if natpmp {
        // the gateway: the UPnP device's address when one answered, else
        // the usual .1 of this network (a router that speaks neither just
        // does not answer)
        let o = lan.octets();
        let gateway = gateway_hint.unwrap_or(Ipv4Addr::new(o[0], o[1], o[2], 1));
        match natpmp_map(gateway, port) {
            Some(mapped) => return Some(mapped),
            None => log::info!("online: NAT-PMP/PCP: {gateway} did not map the port"),
        }
    }
    None
}

/// removes a mapping the session made.
pub fn release(mapping: &Mapping) {
    match mapping {
        Mapping::Upnp { control, service, external_port } => {
            let body = format!(
                "<NewRemoteHost></NewRemoteHost><NewExternalPort>{external_port}</NewExternalPort><NewProtocol>UDP</NewProtocol>"
            );
            match soap(control, service, "DeletePortMapping", &body) {
                Ok(_) => log::info!("online: UPnP: the port mapping {external_port} is removed"),
                Err(error) => log::info!("online: UPnP: the port mapping could not be removed, {error}"),
            }
        }
        Mapping::NatPmp { gateway, internal_port } => {
            let _ = natpmp_request(*gateway, *internal_port, 0);
            log::info!("online: NAT-PMP: the port mapping {internal_port} is removed");
        }
    }
}

const UPNP_SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

fn upnp_map(lan: Ipv4Addr, port: u16) -> Result<Mapped, (String, Option<Ipv4Addr>)> {
    let location = ssdp_search(lan).ok_or(("no router answered the search".to_owned(), None))?;
    let host = url_host(&location);
    let gateway = host.as_deref().and_then(|h| h.split(':').next()).and_then(|h| h.parse::<Ipv4Addr>().ok());
    let fail = |e: String| (e, gateway);
    let description = http(&location, "GET", None, &[]).map_err(fail)?;
    let (service, control) = UPNP_SERVICES
        .iter()
        .find_map(|service| control_url(&description, service).map(|control| (service.to_string(), control)))
        .ok_or_else(|| fail("the router has no WAN connection service".into()))?;
    let control = absolute(&location, &control);
    let mut external = port;
    for _ in 0..4 {
        let body = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{external}</NewExternalPort><NewProtocol>UDP</NewProtocol>\
             <NewInternalPort>{port}</NewInternalPort><NewInternalClient>{lan}</NewInternalClient><NewEnabled>1</NewEnabled>\
             <NewPortMappingDescription>Haiku online battle</NewPortMappingDescription><NewLeaseDuration>7200</NewLeaseDuration>"
        );
        match soap(&control, &service, "AddPortMapping", &body) {
            Ok(_) => {
                let public = soap(&control, &service, "GetExternalIPAddress", "")
                    .ok()
                    .and_then(|reply| tag_text(&reply, "NewExternalIPAddress"))
                    .and_then(|ip| ip.trim().parse::<Ipv4Addr>().ok());
                log::info!("online: UPnP: the router forwards UDP {external} to {lan}:{port}, its address {public:?}");
                return Ok(Mapped { public, port: external, how: Mapping::Upnp { control, service, external_port: external } });
            }
            // taken by another PC: try the next
            Err(error) if error.contains("718") => external = external.wrapping_add(1).max(1024),
            Err(error) => return Err(fail(format!("AddPortMapping failed, {error}"))),
        }
    }
    Err(fail("every port tried was taken".into()))
}

fn ssdp_search(lan: Ipv4Addr) -> Option<String> {
    let socket = UdpSocket::bind(SocketAddrV4::new(lan, 0)).ok()?;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
    let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\n\r\n";
    let until = Instant::now() + Duration::from_millis(2500);
    let mut sent = 0;
    let mut buffer = [0u8; 2048];
    while Instant::now() < until {
        if sent < 3 {
            let _ = socket.send_to(search.as_bytes(), "239.255.255.250:1900");
            sent += 1;
        }
        if let Ok((size, _)) = socket.recv_from(&mut buffer) {
            let text = String::from_utf8_lossy(&buffer[..size]);
            for line in text.lines() {
                if let Some((name, value)) = line.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("location") {
                        return Some(value.trim().to_owned());
                    }
                }
            }
        }
    }
    None
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    Some(rest.split('/').next()?.to_owned())
}

fn absolute(location: &str, path: &str) -> String {
    if path.starts_with("http://") {
        return path.to_owned();
    }
    let host = url_host(location).unwrap_or_default();
    let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
    format!("http://{host}{path}")
}

fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].to_owned())
}

/// the controlURL of the service in a device description.
fn control_url(description: &str, service: &str) -> Option<String> {
    let at = description.find(&format!("<serviceType>{service}</serviceType>"))?;
    let rest = &description[at..];
    let end = rest.find("</service>").unwrap_or(rest.len());
    tag_text(&rest[..end], "controlURL")
}

fn soap(control: &str, service: &str, action: &str, arguments: &str) -> Result<String, String> {
    let body = format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{arguments}</u:{action}></s:Body></s:Envelope>"
    );
    let action_header = format!("\"{service}#{action}\"");
    http(control, "POST", Some(&body), &[("Content-Type", "text/xml; charset=\"utf-8\""), ("SOAPAction", &action_header)])
}

/// a plain HTTP/1.1 request on the router's own network.
fn http(url: &str, method: &str, body: Option<&str>, headers: &[(&str, &str)]) -> Result<String, String> {
    let host = url_host(url).ok_or("not an http address")?;
    let path = &url["http://".len() + host.len()..];
    let path = if path.is_empty() { "/" } else { path };
    let address = if host.contains(':') { host.clone() } else { format!("{host}:80") };
    let target = address.to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    // only the router's own network, never the Internet
    match target.ip() {
        IpAddr::V4(ip) if is_private(ip) => {}
        _ => return Err(format!("{host} is not on this network")),
    }
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    let _ = stream.take(256 * 1024).read_to_end(&mut reply);
    let text = String::from_utf8_lossy(&reply).into_owned();
    let status = text.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        let code = tag_text(&text, "errorCode").unwrap_or_default();
        return Err(format!("HTTP {status} {code}"));
    }
    Ok(text.split_once("\r\n\r\n").map_or(text.clone(), |(_, body)| body.to_owned()))
}

/// NAT-PMP's UDP mapping request (RFC 6886): lifetime 0 removes it.
fn natpmp_request(gateway: Ipv4Addr, port: u16, lifetime: u32) -> Option<(u16, Option<Ipv4Addr>)> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    let target = SocketAddr::new(IpAddr::V4(gateway), 5351);
    let mut request = vec![0u8, 1, 0, 0];
    request.extend_from_slice(&port.to_be_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    request.extend_from_slice(&lifetime.to_be_bytes());
    let mut buffer = [0u8; 64];
    for _ in 0..3 {
        socket.send_to(&request, target).ok()?;
        if let Ok((size, from)) = socket.recv_from(&mut buffer) {
            if from.ip() == target.ip() && size >= 16 && buffer[0] == 0 && buffer[1] == 129 && buffer[2..4] == [0, 0] {
                let external = u16::from_be_bytes([buffer[10], buffer[11]]);
                // the public address: a request of its own
                let public = natpmp_public(&socket, target);
                return Some((external, public));
            }
        }
    }
    None
}

fn natpmp_public(socket: &UdpSocket, target: SocketAddr) -> Option<Ipv4Addr> {
    socket.send_to(&[0, 0], target).ok()?;
    let mut buffer = [0u8; 16];
    let (size, _) = socket.recv_from(&mut buffer).ok()?;
    (size >= 12 && buffer[1] == 128 && buffer[2..4] == [0, 0]).then(|| Ipv4Addr::new(buffer[8], buffer[9], buffer[10], buffer[11]))
}

/// PCP's MAP request (RFC 6887) for UDP, for routers that speak PCP only.
fn pcp_map(gateway: Ipv4Addr, port: u16) -> Option<(u16, Option<Ipv4Addr>)> {
    let lan = local_ipv4()?;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    let target = SocketAddr::new(IpAddr::V4(gateway), 5351);
    let mut request = vec![2u8, 1, 0, 0];
    request.extend_from_slice(&7200u32.to_be_bytes());
    request.extend_from_slice(&lan.to_ipv6_mapped().octets());
    let nonce: [u8; 12] = super::crypto::random();
    request.extend_from_slice(&nonce);
    request.extend_from_slice(&[17, 0, 0, 0]);
    request.extend_from_slice(&port.to_be_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    request.extend_from_slice(&Ipv4Addr::UNSPECIFIED.to_ipv6_mapped().octets());
    let mut buffer = [0u8; 128];
    for _ in 0..3 {
        socket.send_to(&request, target).ok()?;
        if let Ok((size, _)) = socket.recv_from(&mut buffer) {
            if size >= 60 && buffer[0] == 2 && buffer[1] == 0x81 && buffer[3] == 0 && buffer[24..36] == nonce {
                let external = u16::from_be_bytes([buffer[42], buffer[43]]);
                let ip: [u8; 16] = buffer[44..60].try_into().unwrap();
                let public = Ipv6Addr::from(ip).to_ipv4_mapped();
                return Some((external, public));
            }
        }
    }
    None
}

fn natpmp_map(gateway: Ipv4Addr, port: u16) -> Option<Mapped> {
    if let Some((external, public)) = natpmp_request(gateway, port, 7200) {
        log::info!("online: NAT-PMP: {gateway} forwards UDP {external} to port {port}, its address {public:?}");
        return Some(Mapped { public, port: external, how: Mapping::NatPmp { gateway, internal_port: port } });
    }
    let (external, public) = pcp_map(gateway, port)?;
    log::info!("online: PCP: {gateway} forwards UDP {external} to port {port}, its address {public:?}");
    // PCP mappings end with their lifetime; removing one takes the nonce,
    // so it is left to expire
    Some(Mapped { public, port: external, how: Mapping::NatPmp { gateway, internal_port: port } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stun_answer_reads() {
        // XOR-MAPPED-ADDRESS of 203.0.113.7:40000
        let port = 40000u16 ^ 0x2112;
        let ip = [203 ^ 0x21, 0 ^ 0x12, 113 ^ 0xA4, 7 ^ 0x42];
        let mut attribute = vec![0x00, 0x20, 0x00, 0x08, 0x00, 0x01];
        attribute.extend_from_slice(&port.to_be_bytes());
        attribute.extend_from_slice(&ip);
        assert_eq!(mapped_address(&attribute), Some("203.0.113.7:40000".parse().unwrap()));
    }

    #[test]
    fn a_device_description_gives_its_control_url() {
        let xml = "<root><URLBase>http://192.168.1.1:5000</URLBase><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><serviceId>x</serviceId><controlURL>/ctl/IPConn</controlURL></service></serviceList></device></root>";
        assert_eq!(control_url(xml, UPNP_SERVICES[1]).as_deref(), Some("/ctl/IPConn"));
        assert_eq!(absolute("http://192.168.1.1:5000/rootDesc.xml", "/ctl/IPConn"), "http://192.168.1.1:5000/ctl/IPConn");
    }
}
