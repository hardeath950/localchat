mod channel;
mod discovery;
mod gui;
mod identity;
mod profile;
mod protocol;
mod session;
mod update;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use discovery::PeerTable;
use identity::{fingerprint, load_or_init, pairing_code};
use session::Ctx;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(
    name = "localchat",
    version,
    about = "Chat e transferencia de arquivos na LAN, criptografado ponto a ponto"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Cria a identidade local (chave de criptografia)
    Init {
        #[arg(long)]
        name: Option<String>,
    },
    /// Roda o no: aceita conexoes, descobre vizinhos e recebe mensagens/arquivos
    Serve {
        #[arg(long, default_value = "0.0.0.0:47821")]
        listen: String,
        #[arg(long)]
        download_dir: Option<PathBuf>,
        /// Recusa arquivos automaticamente (precisa aceitar manualmente; v0 so tem auto)
        #[arg(long)]
        no_auto_accept: bool,
        /// Nao usa broadcast UDP para descoberta
        #[arg(long)]
        no_discovery: bool,
    },
    /// Mostra quem esta na rede rodando o app
    Peers {
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// Envia uma mensagem para um par (nome ou ip:porta)
    Msg {
        alvo: String,
        texto: Vec<String>,
    },
    /// Envia um arquivo para um par (nome ou ip:porta)
    Send {
        alvo: String,
        arquivo: PathBuf,
    },
    /// Abre a interface grafica (janela) — recomendado para o dia a dia
    Gui {
        #[arg(long, default_value = "0.0.0.0:47821")]
        listen: String,
        #[arg(long)]
        download_dir: Option<PathBuf>,
    },
    /// Lista ou esquece chaves confiadas (TOFU)
    Trust {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        forget: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Gui {
            listen,
            download_dir,
        } => gui::run(listen, download_dir),
        cmd => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run_cmd(cmd))
        }
    }
}

async fn run_cmd(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Init { name } => {
            let (id, _priv, pubk) = load_or_init(name)?;
            println!("Nome: {}", id.name);
            println!("Fingerprint: {}", fingerprint(&pubk));
            Ok(())
        }
        Cmd::Trust { list, forget } => {
            if let Some(q) = forget.as_ref() {
                let n = identity::trust_forget(q);
                println!("{n} entrada(s) removida(s)");
            }
            if list || forget.is_none() {
                let store = identity::load_trust();
                if store.peers.is_empty() {
                    println!("(nenhum par confiavel ainda)");
                }
                for (fp, e) in &store.peers {
                    println!("{fp}  {}  (ultimo contato: t={})", e.name, e.last_seen);
                }
            }
            Ok(())
        }
        Cmd::Peers { timeout } => {
            let (_id, _priv, pubk) = load_or_init(None)?;
            let fp = fingerprint(&pubk);
            let found = discovery::scan(timeout).await?;
            if found.is_empty() {
                println!("Nenhum par encontrado em {timeout}s.");
                return Ok(());
            }
            for p in found {
                println!(
                    "{:20} {}:{}  codigo {}",
                    p.name,
                    p.ip,
                    p.port,
                    pairing_code(&fp, &p.fp)
                );
            }
            Ok(())
        }
        Cmd::Msg { alvo, texto } => {
            let ctx = one_shot_ctx()?;
            let addr = resolve_alvo(&alvo, None).await?;
            let text = texto.join(" ");
            match session::send_chat(&addr, &ctx, &text, None).await {
                Ok(name) => {
                    println!("mensagem enviada para {name}");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Cmd::Send { alvo, arquivo } => {
            let ctx = one_shot_ctx()?;
            let addr = resolve_alvo(&alvo, None).await?;
            let name = session::send_file(&addr, &ctx, &arquivo, None).await?;
            println!("arquivo entregue para {name}");
            Ok(())
        }
        Cmd::Serve {
            listen,
            download_dir,
            no_auto_accept,
            no_discovery,
        } => run_serve(listen, download_dir, no_auto_accept, no_discovery).await,
        Cmd::Gui { .. } => unreachable!(),
    }
}

fn one_shot_ctx() -> Result<Ctx> {
    let (id, private, public) = load_or_init(None)?;
    Ok(Ctx {
        name: std::sync::RwLock::new(id.name),
        private,
        public,
        download_dir: PathBuf::new(),
        auto_accept: true,
        notify: None,
        approvals: Default::default(),
        profile: Default::default(),
    })
}

async fn run_serve(
    listen: String,
    download_dir: Option<PathBuf>,
    no_auto_accept: bool,
    no_discovery: bool,
) -> Result<()> {
    let (id, private, public) = load_or_init(None)?;
    let fp = fingerprint(&public);
    let addr: SocketAddr = listen.parse().context("listen invalido (use ip:porta)")?;
    let download_dir = download_dir.unwrap_or_else(default_download_dir);
    std::fs::create_dir_all(&download_dir)?;
    let ctx = Arc::new(Ctx {
        name: std::sync::RwLock::new(id.name.clone()),
        private,
        public,
        download_dir: download_dir.clone(),
        auto_accept: !no_auto_accept,
        notify: None,
        approvals: Default::default(),
        profile: std::sync::RwLock::new(profile::load_own()),
    });

    let table: PeerTable = Arc::new(Mutex::new(HashMap::new()));

    if !no_discovery {
        let udp = Arc::new(discovery::bind_udp(protocol::DISCOVERY_PORT)?);
        tokio::spawn(discovery::announce_loop(udp.clone(), ctx.clone(), addr.port()));
        tokio::spawn(discovery::listen_loop(udp, table.clone(), ctx.clone(), addr.port()));
    }

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("nao consegui abrir {addr}"))?;
    println!("No '{}' ouvindo em {}", id.name, addr);
    println!("Fingerprint: {fp}");
    println!("Arquivos recebidos em: {}", download_dir.display());
    println!("Comandos: /help");

    let accept_ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let c = accept_ctx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = session::handle_conn(stream, c).await {
                            eprintln!("[{peer}] conexao encerrada: {e:#}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("accept: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            _ => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('/') {
            println!("Use /help para ver os comandos.");
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let cmd = parts.next().unwrap_or("");
        match cmd {
            "/help" => print_help(),
            "/peers" => print_peers(&table, &fp),
            "/quit" => std::process::exit(0),
            "/msg" => {
                let (alvo, texto) = match (parts.next(), parts.next()) {
                    (Some(a), Some(t)) => (a.to_string(), t.to_string()),
                    _ => {
                        println!("uso: /msg <alvo> <texto>");
                        continue;
                    }
                };
                match resolve_alvo(&alvo, Some(&table)).await {
                    Ok(addr) => match session::send_chat(&addr, &ctx, &texto, None).await {
                        Ok(name) => println!("mensagem enviada para {name}"),
                        Err(e) => println!("falha: {e:#}"),
                    },
                    Err(e) => println!("{e:#}"),
                }
            }
            "/send" => {
                let (alvo, caminho) = match (parts.next(), parts.next()) {
                    (Some(a), Some(t)) => (a.to_string(), t.to_string()),
                    _ => {
                        println!("uso: /send <alvo> <caminho do arquivo>");
                        continue;
                    }
                };
                match resolve_alvo(&alvo, Some(&table)).await {
                    Ok(addr) => match session::send_file(&addr, &ctx, PathBuf::from(&caminho).as_path(), None).await
                    {
                        Ok(name) => println!("arquivo entregue para {name}"),
                        Err(e) => println!("falha: {e:#}"),
                    },
                    Err(e) => println!("{e:#}"),
                }
            }
            other => println!("comando desconhecido: {other}"),
        }
    }
    println!("Entrada encerrada; o no continua rodando (Ctrl+C para sair).");
    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    Ok(())
}

fn print_help() {
    println!("Comandos:");
    println!("  /peers                          quem esta na rede");
    println!("  /msg <alvo> <texto>             envia mensagem (alvo = nome ou ip:porta)");
    println!("  /send <alvo> <caminho>          envia arquivo");
    println!("  /quit                           sai");
}

fn print_peers(table: &PeerTable, fp: &str) {
    let mut v = discovery::online(table);
    if v.is_empty() {
        println!("Nenhum par na rede agora (aguarde uns segundos ou confira o firewall).");
        return;
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    for p in v {
        println!(
            "{:20} {}:{}  codigo {}",
            p.name,
            p.ip,
            p.port,
            pairing_code(fp, &p.fp)
        );
    }
}

/// Resolve o alvo: nome conhecido na tabela, "ip:porta" direto ou nome via varredura.
async fn resolve_alvo(target: &str, table: Option<&PeerTable>) -> Result<String> {
    if let Some(t) = table {
        let hit = discovery::online(t)
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(target))
            .map(|p| format!("{}:{}", p.ip, p.port));
        if let Some(addr) = hit {
            return Ok(addr);
        }
    }
    if target.contains(':') {
        return Ok(target.to_string());
    }
    let found = discovery::scan(3).await?;
    if let Some(p) = found.iter().find(|p| p.name.eq_ignore_ascii_case(target)) {
        return Ok(format!("{}:{}", p.ip, p.port));
    }
    bail!("alvo '{target}' nao encontrado (use /peers ou passe ip:porta)");
}

fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LocalChat")
}
