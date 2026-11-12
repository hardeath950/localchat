use crate::protocol::{Beacon, DISCOVERY_PORT, PROTO_VERSION};
use crate::session::{Ctx, Event};
use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket as StdUdp};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub fp: String,
    pub last_seen: Instant,
    /// Versao do protocolo que ele anuncia (diferente da nossa = nao conversam).
    pub v: u32,
    /// Versao do perfil anunciada (vazia = versao antiga, sem perfil).
    pub rev: String,
}

pub type PeerTable = Arc<Mutex<HashMap<String, PeerInfo>>>;

/// Pares online agora. Quem nao anuncia ha 10s (anuncio a cada 3s) saiu da rede.
pub fn online(table: &PeerTable) -> Vec<PeerInfo> {
    let mut t = table.lock().unwrap();
    t.retain(|_, p| p.last_seen.elapsed() < Duration::from_secs(10));
    t.values().cloned().collect()
}

/// IPs digitados pelo usuario ("Conectar por IP"), para redes onde o broadcast
/// nao passa (VPN, varias placas de rede). Recebem anuncio e consulta diretos.
static MANUAL: Mutex<Vec<Ipv4Addr>> = Mutex::new(Vec::new());

fn manual_path() -> std::path::PathBuf {
    crate::identity::config_dir().join("manual_peers.txt")
}

pub fn load_manual() {
    let text = std::fs::read_to_string(manual_path()).unwrap_or_default();
    *MANUAL.lock().unwrap() = text.lines().filter_map(|l| l.trim().parse().ok()).collect();
}

// shortcut: nao ha tela para remover um IP (edite manual_peers.txt); criar se virar necessidade
pub fn add_manual(ip: Ipv4Addr) {
    let mut m = MANUAL.lock().unwrap();
    if !m.contains(&ip) {
        m.push(ip);
        let lines: Vec<String> = m.iter().map(|i| i.to_string()).collect();
        let _ = std::fs::write(manual_path(), lines.join("\n"));
    }
}

/// Pede para a instancia ja aberta nesta maquina mostrar a janela.
pub fn show_running_instance() {
    if let Ok(s) = StdUdp::bind(("127.0.0.1", 0)) {
        let _ = s.send_to(&beacon("show", "", 0, "", ""), ("127.0.0.1", DISCOVERY_PORT));
    }
}

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

fn beacon(kind: &str, name: &str, port: u16, fp: &str, rev: &str) -> Vec<u8> {
    serde_json::to_vec(&Beacon {
        app: "localchat".into(),
        v: PROTO_VERSION,
        kind: kind.into(),
        name: name.into(),
        port,
        fp: fp.into(),
        rev: rev.into(),
    })
    .expect("beacon serializa")
}

/// Anuncia a cada 3s que este no esta online.
pub async fn announce_loop(sock: Arc<UdpSocket>, ctx: Arc<Ctx>, port: u16) {
    let fp = ctx.fp();
    loop {
        let name = ctx.name();
        let rev = ctx.profile_rev();
        let msg = beacon("announce", &name, port, &fp, &rev);
        let _ = sock.send_to(&msg, ("255.255.255.255", DISCOVERY_PORT)).await;
        // IPs manuais: o anuncio faz eles nos verem, a consulta faz nos os vermos
        let manual = MANUAL.lock().unwrap().clone();
        if !manual.is_empty() {
            let query = beacon("query", &name, port, &fp, &rev);
            for ip in manual {
                let _ = sock.send_to(&msg, (ip, DISCOVERY_PORT)).await;
                let _ = sock.send_to(&query, (ip, DISCOVERY_PORT)).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Escuta anuncios e responde a consultas ("query") na LAN.
pub async fn listen_loop(sock: Arc<UdpSocket>, table: PeerTable, ctx: Arc<Ctx>, port: u16) {
    let self_fp = ctx.fp();
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
        if b.app != "localchat" {
            continue;
        }
        match b.kind.as_str() {
            // o atalho foi aberto de novo nesta maquina: a janela aparece
            "show" => {
                if src.ip().is_loopback() {
                    ctx.emit(Event::GuiShow);
                }
                continue;
            }
            // a consulta do `localchat peers` vem sem fingerprint
            "query" => {
                if b.fp != self_fp {
                    let ans = beacon("answer", &ctx.name(), port, &self_fp, &ctx.profile_rev());
                    let _ = sock.send_to(&ans, src).await;
                }
                continue;
            }
            _ => {}
        }
        if b.fp == self_fp || b.fp.is_empty() || !crate::session::valid_name(&b.name) {
            continue;
        }
        // anuncio falso em massa (fingerprints aleatorios) nao enche a lista
        {
            let t = table.lock().unwrap();
            if t.len() >= 128 && !t.contains_key(&b.fp) {
                continue;
            }
        }
        table.lock().unwrap().insert(
            b.fp.clone(),
            PeerInfo {
                name: b.name,
                ip: src.ip().to_string(),
                port: b.port,
                fp: b.fp,
                last_seen: Instant::now(),
                v: b.v,
                rev: b.rev,
            },
        );
    }
}

/// Varre a LAN por N segundos e retorna os pares encontrados.
/// Escuta na mesma porta de descoberta: no Windows, broadcast so chega a um
/// soquete por porta, entao o scan precisa estar na 47820 para ouvir anuncios.
pub async fn scan(secs: u64) -> Result<Vec<PeerInfo>> {
    let sock = Arc::new(UdpSocket::from_std(make_socket(DISCOVERY_PORT)?)?);
    let q = beacon("query", "?", 0, "", "");
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
                    last_seen: Instant::now(),
                    v: b.v,
                    rev: b.rev,
                },
            );
        }
    }
    let mut v: Vec<_> = found.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}
