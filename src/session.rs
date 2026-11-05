use crate::channel::{Msg, SecureChannel};
use crate::identity::{fingerprint, trust_check};
use crate::protocol::{Control, CHUNK_SIZE};
use anyhow::{bail, Context, Result};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Eventos que saem das conexoes para consumo da interface.
#[derive(Debug, Clone)]
pub enum Event {
    Chat { from: String, text: String },
    FileReceived { from: String, path: String, size: u64 },
    Notice { text: String },
    /// Par novo pedindo conexao: a UI responde com Confiar/Recusar.
    ApprovalRequest { id: u64, name: String, code: String },
    /// Progresso de transferencia de arquivo.
    Progress { name: String, done: u64, total: u64 },
    /// Clique no icone da bandeja ou menu "Abrir".
    GuiShow,
    /// Menu "Sair" da bandeja.
    GuiQuit,
}

/// Fila de aprovacoes pendentes: a UI decide "Confiar / Recusar" cada par novo.
#[derive(Default)]
pub struct ApprovalGate {
    seq: std::sync::atomic::AtomicU64,
    pending: std::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<bool>>>,
}

impl ApprovalGate {
    pub fn open(&self, tx: tokio::sync::oneshot::Sender<bool>) -> u64 {
        let id = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.pending.lock().unwrap().insert(id, tx);
        id
    }

    pub fn respond(&self, id: u64, ok: bool) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(ok);
        }
    }
}

pub struct Ctx {
    pub name: String,
    pub private: Vec<u8>,
    pub public: Vec<u8>,
    pub download_dir: PathBuf,
    pub auto_accept: bool,
    pub notify: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
    pub approvals: Arc<ApprovalGate>,
}

impl Ctx {
    pub fn fp(&self) -> String {
        fingerprint(&self.public)
    }

    pub fn emit(&self, ev: Event) {
        if let Some(tx) = &self.notify {
            let _ = tx.send(ev);
        }
    }

    /// Pede aprovacao para um par novo. Sem UI (CLI) aprova sozinho.
    /// Aguarda ate 5 minutos; a resposta vem de `respond_approval`.
    pub async fn request_approval(&self, name: &str, code: &str) -> bool {
        let Some(tx) = &self.notify else {
            return true;
        };
        let (otx, orx) = tokio::sync::oneshot::channel();
        let id = self.approvals.open(otx);
        let _ = tx.send(Event::ApprovalRequest {
            id,
            name: name.to_string(),
            code: code.to_string(),
        });
        match tokio::time::timeout(std::time::Duration::from_secs(300), orx).await {
            Ok(Ok(v)) => v,
            _ => {
                self.approvals.respond(id, false);
                false
            }
        }
    }

    pub fn respond_approval(&self, id: u64, ok: bool) {
        self.approvals.respond(id, ok);
    }
}

/// Conecta, faz o handshake e troca o Hello (com verificacao de identidade + TOFU).
pub async fn connect(addr: &str, ctx: &Ctx) -> Result<(SecureChannel<TcpStream>, String)> {
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("nao conectei em {addr}"))?;
    stream.set_nodelay(true)?;
    let mut chan = SecureChannel::initiator(stream, &ctx.private).await?;
    chan.send_control(&Control::Hello {
        name: ctx.name.clone(),
        fingerprint: ctx.fp(),
        version: crate::protocol::PROTO_VERSION,
    })
    .await?;
    let (remote_name, remote_fp) = match chan.recv().await? {
        Msg::Control(Control::Hello { name, fingerprint, .. }) => (name, fingerprint),
        _ => bail!("esperava Hello do par"),
    };
    if remote_fp != chan.remote_fingerprint() {
        bail!("seguranca: o par declarou fingerprint diferente do handshake");
    }
    let is_new = trust_check(&remote_fp, &remote_name).map_err(anyhow::Error::msg)?;
    if is_new {
        let code = crate::identity::pairing_code(&ctx.fp(), &remote_fp);
        let msg = format!(
            "primeiro contato com '{remote_name}' — codigo de pareamento: {code} (confira com o outro aparelho)"
        );
        println!("{msg}");
        ctx.emit(Event::Notice { text: msg });
    }
    Ok((chan, remote_name))
}

/// Lado servidor: atende uma conexao entrante (chat + recebimento de arquivos).
pub async fn handle_conn(stream: TcpStream, ctx: std::sync::Arc<Ctx>) -> Result<()> {
    stream.set_nodelay(true)?;
    let mut chan = SecureChannel::responder(stream, &ctx.private).await?;
    chan.send_control(&Control::Hello {
        name: ctx.name.clone(),
        fingerprint: ctx.fp(),
        version: crate::protocol::PROTO_VERSION,
    })
    .await?;
    let (remote_name, remote_fp) = match chan.recv().await? {
        Msg::Control(Control::Hello { name, fingerprint, .. }) => (name, fingerprint),
        _ => bail!("esperava Hello do par"),
    };
    if remote_fp != chan.remote_fingerprint() {
        bail!("seguranca: o par declarou fingerprint diferente do handshake");
    }
    let is_new = crate::identity::trust_status(&remote_fp, &remote_name).map_err(anyhow::Error::msg)?;
    let code = crate::identity::pairing_code(&ctx.fp(), &remote_fp);
    if is_new {
        let msg = format!(
            "primeiro contato com '{remote_name}' — codigo de pareamento: {code} (confira com o outro aparelho)"
        );
        println!("{msg}");
        ctx.emit(Event::Notice { text: msg });
        // aprovacao explicita: a UI (ou o timeout) decide
        if !ctx.request_approval(&remote_name, &code).await {
            let _ = chan
                .send_control(&Control::Bye {
                    reason: "par nao confiado".into(),
                })
                .await;
            println!("conexao com '{remote_name}' recusada");
            ctx.emit(Event::Notice {
                text: format!("voce recusou o par '{remote_name}'"),
            });
            return Ok(());
        }
    }
    crate::identity::trust_save(&remote_fp, &remote_name);

    loop {
        let msg = match chan.recv_opt().await? {
            Some(m) => m,
            None => break,
        };
        match msg {
            Msg::Control(Control::Chat { text }) => {
                println!("[{}] {}", remote_name, text);
                ctx.emit(Event::Chat {
                    from: remote_name.clone(),
                    text,
                });
            }
            Msg::Control(Control::FileOffer { file_id, name, size }) => {
                receive_file(&mut chan, &file_id, &name, size, &ctx, &remote_name).await?;
            }
            Msg::Control(Control::Bye { .. }) => break,
            Msg::Control(_) => {}
            Msg::Chunk { .. } => bail!("chunk fora de contexto"),
        }
    }
    Ok(())
}

pub async fn send_chat(addr: &str, ctx: &Ctx, text: &str) -> Result<String> {
    let (mut chan, remote) = connect(addr, ctx).await?;
    chan.send_control(&Control::Chat { text: text.to_string() })
        .await?;
    graceful_bye(&mut chan).await;
    Ok(remote)
}

pub async fn send_file(addr: &str, ctx: &Ctx, path: &Path) -> Result<String> {
    let (mut chan, remote) = connect(addr, ctx).await?;
    send_file_over_channel(&mut chan, path, ctx).await?;
    graceful_bye(&mut chan).await;
    Ok(remote)
}

async fn graceful_bye(chan: &mut SecureChannel<TcpStream>) {
    if let Err(e) = chan
        .send_control(&Control::Bye { reason: "fim".into() })
        .await
    {
        eprintln!("aviso: nao consegui enviar Bye: {e:#}");
    }
    let _ = chan.shutdown().await;
}

/// SHA-256 de um arquivo (leitura em streaming).
async fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn send_file_over_channel(
    chan: &mut SecureChannel<TcpStream>,
    path: &Path,
    ctx: &Ctx,
) -> Result<()> {
    let meta = tokio::fs::metadata(path).await?;
    if !meta.is_file() {
        bail!("nao e um arquivo: {}", path.display());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("arquivo")
        .to_string();
    let size = meta.len();
    let mut id16 = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut id16);
    let file_id = hex::encode(id16);

    // hash primeiro: o destino recebe o hash do arquivo INTEIRO, o que
    // permite validar tambem o pedaco retomado de uma tentativa anterior
    let sha256 = hash_file(path).await?;

    chan.send_control(&Control::FileOffer {
        file_id: file_id.clone(),
        name: name.clone(),
        size,
    })
    .await?;
    let offset = match chan.recv().await? {
        Msg::Control(Control::FileAccept {
            file_id: fid,
            offset,
        }) => {
            if fid != file_id {
                bail!("FileAccept de outro arquivo");
            }
            if offset > size || offset % CHUNK_SIZE as u64 != 0 {
                bail!("offset de retomada invalido: {offset}");
            }
            offset
        }
        Msg::Control(Control::FileReject { reason, .. }) => bail!("recusado pelo par: {reason}"),
        _ => bail!("resposta inesperada ao FileOffer"),
    };

    let mut file = File::open(path).await?;
    if offset > 0 {
        use tokio::io::{AsyncSeekExt, SeekFrom};
        file.seek(SeekFrom::Start(offset)).await?;
        let pct = offset * 100 / size.max(1);
        let msg = format!("retomando envio de '{name}' a partir de {pct}%");
        println!("{msg}");
        ctx.emit(Event::Notice { text: msg });
    }

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut seq: u32 = (offset / CHUNK_SIZE as u64) as u32;
    let mut sent = offset;
    let mut since_progress = 0u32;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        chan.send_chunk(&id16, seq, &buf[..n]).await?;
        seq += 1;
        sent += n as u64;
        since_progress += 1;
        if since_progress >= 16 {
            since_progress = 0;
            ctx.emit(Event::Progress {
                name: name.clone(),
                done: sent,
                total: size,
            });
        }
    }
    ctx.emit(Event::Progress {
        name: name.clone(),
        done: size,
        total: size,
    });
    chan.send_control(&Control::FileDone {
        file_id: file_id.clone(),
        sha256,
    })
    .await?;
    match chan.recv().await? {
        Msg::Control(Control::FileAck { ok: true, note, .. }) => {
            println!("enviado com sucesso ({note})");
            Ok(())
        }
        Msg::Control(Control::FileAck { ok: false, note, .. }) => bail!("par recusou: {note}"),
        _ => bail!("sem FileAck do par"),
    }
}

async fn receive_file(
    chan: &mut SecureChannel<TcpStream>,
    file_id: &str,
    name: &str,
    size: u64,
    ctx: &Ctx,
    remote: &str,
) -> Result<()> {
    if !ctx.auto_accept {
        chan.send_control(&Control::FileReject {
            file_id: file_id.to_string(),
            reason: "auto-accept desativado neste no".into(),
        })
        .await?;
        return Ok(());
    }
    std::fs::create_dir_all(&ctx.download_dir)?;
    let safe = sanitize(name);
    let part_path = ctx.download_dir.join(format!("{safe}.part"));

    // retomada: se existe .part parcial menor que o arquivo, continua de la
    let mut offset: u64 = 0;
    if let Ok(meta) = tokio::fs::metadata(&part_path).await {
        let s = meta.len();
        if s > 0 && s < size {
            offset = s - (s % CHUNK_SIZE as u64);
            tokio::fs::OpenOptions::new()
                .write(true)
                .open(&part_path)
                .await?
                .set_len(offset)
                .await?;
            let pct = offset * 100 / size.max(1);
            let msg = format!("retomando recebimento de '{name}' a partir de {pct}%");
            println!("[{remote}] {msg}");
            ctx.emit(Event::Notice { text: msg });
        }
    }

    chan.send_control(&Control::FileAccept {
        file_id: file_id.to_string(),
        offset,
    })
    .await?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)
        .await
        .with_context(|| format!("abrir {}", part_path.display()))?;
    let mut received: u64 = 0;
    let mut expected: u32 = (offset / CHUNK_SIZE as u64) as u32;
    let want_id = hex::decode(file_id).unwrap_or_default();

    loop {
        match chan.recv().await? {
            Msg::Chunk { id, seq, data } => {
                if id != want_id {
                    bail!("chunk de arquivo inesperado");
                }
                if seq != expected {
                    bail!("ordem de chunk quebrada: {seq} != {expected}");
                }
                file.write_all(&data).await?;
                received += data.len() as u64;
                expected += 1;
                if expected % 16 == 0 {
                    ctx.emit(Event::Progress {
                        name: name.to_string(),
                        done: offset + received,
                        total: size,
                    });
                }
            }
            Msg::Control(Control::FileDone { file_id: fid, sha256 }) => {
                if fid != file_id {
                    bail!("FileDone de outro arquivo");
                }
                file.flush().await?;
                drop(file);
                if offset + received != size {
                    let note = format!(
                        "integridade falhou: {}/{} bytes recebidos",
                        offset + received,
                        size
                    );
                    chan.send_control(&Control::FileAck {
                        file_id: fid,
                        ok: false,
                        note: note.clone(),
                    })
                    .await?;
                    bail!(note);
                }
                // hash do arquivo COMPLETO (inclui o pedaco retomado)
                let got = hash_file(&part_path).await?;
                if got != sha256 {
                    let note = format!("integridade falhou: sha256 {got} != {sha256}");
                    chan.send_control(&Control::FileAck {
                        file_id: fid,
                        ok: false,
                        note: note.clone(),
                    })
                    .await?;
                    bail!(note);
                }
                let path = unique_path(&ctx.download_dir, &safe);
                tokio::fs::rename(&part_path, &path).await?;
                chan.send_control(&Control::FileAck {
                    file_id: file_id.to_string(),
                    ok: true,
                    note: "ok".into(),
                })
                .await?;
                println!(
                    "[{}] arquivo recebido: {} ({} bytes)",
                    remote,
                    path.display(),
                    size
                );
                ctx.emit(Event::FileReceived {
                    from: remote.to_string(),
                    path: path.display().to_string(),
                    size,
                });
                return Ok(());
            }
            other => bail!("frame inesperado durante transferencia: {:?}", debug_msg(&other)),
        }
    }
}

fn debug_msg(m: &Msg) -> String {
    match m {
        Msg::Control(c) => format!("control {c:?}"),
        Msg::Chunk { seq, data, .. } => format!("chunk seq={seq} len={}", data.len()),
    }
}

fn sanitize(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("arquivo");
    let cleaned: String = base
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    if cleaned.trim().is_empty() {
        "arquivo".into()
    } else {
        cleaned
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("arquivo");
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 1..10_000 {
        let candidate = dir.join(format!("{stem}-{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    p
}
