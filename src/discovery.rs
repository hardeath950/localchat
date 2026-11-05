use crate::protocol::{Beacon, DISCOVERY_PORT, PROTO_VERSION};
use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket as StdUdp};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::net::UdpSocket;

#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub fp: String,
    pub last_seen: SystemTime,
}

pub type PeerTable = Arc<Mutex<HashMap<String, PeerInfo>>>;

fn make_socket(port: u16) -> Result<StdUdp> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_broadcast(true)?;
    sock.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

pub fn bind_udp(port: u16) -> Result<UdpSocket> {
    Ok(UdpSocket::from_std(make_socket(port)?)?)
}

fn beacon(kind: &str, name: &str, port: u16, fp: &str) -> Vec<u8> {
    serde_json::to_vec(&Beacon {
        app: "localchat".into(),
        v: PROTO_VERSION,
        kind: kind.into(),
        name: name.into(),
        port,
        fp: fp.into(),
    })
    .expect("beacon serializa")
}

/// Anuncia a cada 3s que este no esta online.
pub async fn announce_loop(sock: Arc<UdpSocket>, name: String, port: u16, fp: String) {
    let msg = beacon("announce", &name, port, &fp);
    loop {
        let _ = sock.send_to(&msg, ("255.255.255.255", DISCOVERY_PORT)).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Escuta anuncios e responde a consultas ("query") na LAN.
pub async fn listen_loop(
    sock: Arc<UdpSocket>,
    table: PeerTable,
    self_fp: String,
    name: String,
    port: u16,
) {
    let mut buf = [0u8; 8192];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let Ok(b) = serde_json::from_slice::<Beacon>(&buf[..n]) else {
            continue;
        };
        if b.app != "localchat" || b.fp == self_fp || b.fp.is_empty() {
            continue;
        }
        if b.kind == "query" {
            let ans = beacon("answer", &name, port, &self_fp);
            let _ = sock.send_to(&ans, src).await;
            continue;
        }
        table.lock().unwrap().insert(
            b.fp.clone(),
            PeerInfo {
                name: b.name,
                ip: src.ip().to_string(),
                port: b.port,
                fp: b.fp,
                last_seen: SystemTime::now(),
            },
        );
    }
}

/// Varre a LAN por N segundos e retorna os pares encontrados.
/// Escuta na mesma porta de descoberta: no Windows, broadcast so chega a um
/// soquete por porta, entao o scan precisa estar na 47820 para ouvir anuncios.
pub async fn scan(secs: u64) -> Result<Vec<PeerInfo>> {
    let sock = Arc::new(UdpSocket::from_std(make_socket(DISCOVERY_PORT)?)?);
    let q = beacon("query", "?", 0, "");
    let _ = sock.send_to(&q, ("255.255.255.255", DISCOVERY_PORT)).await;
    let mut found: HashMap<String, PeerInfo> = HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let wait = left.min(Duration::from_millis(500));
        if let Ok(Ok((n, src))) = tokio::time::timeout(wait, sock.recv_from(&mut buf)).await {
            let Ok(b) = serde_json::from_slice::<Beacon>(&buf[..n]) else {
                continue;
            };
            if b.app != "localchat" || b.fp.is_empty() || b.kind == "query" {
                continue;
            }
            found.insert(
                b.fp.clone(),
                PeerInfo {
                    name: b.name,
                    ip: src.ip().to_string(),
                    port: b.port,
                    fp: b.fp,
                    last_seen: SystemTime::now(),
                },
            );
        }
    }
    let mut v: Vec<_> = found.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}
