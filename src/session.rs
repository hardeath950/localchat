use crate::channel::{Msg, SecureChannel};
use crate::identity::{fingerprint, trust_check};
use crate::protocol::{Control, ShareEntry, CHUNK_SIZE};
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
    Chat { from: String, fp: String, text: String },
    FileReceived { from: String, fp: String, path: String, size: u64 },
    /// Resultado de um envio iniciado pela UI (`err` = por que nao foi entregue).
    Sent { id: u64, err: Option<String> },
    Notice { text: String },
    /// Par novo pedindo conexao: a UI responde com Confiar/Recusar.
    ApprovalRequest { id: u64, name: String, code: String },
    /// Progresso de transferencia de arquivo.
    /// `fp` = o outro lado (para mostrar dentro da conversa certa).
    Progress { fp: String, name: String, done: u64, total: u64 },
    /// Clique no icone da bandeja ou menu "Abrir".
    GuiShow,
    /// Menu "Sair" da bandeja.
    GuiQuit,
    /// Ha versao nova publicada (manifesto ja verificado).
    UpdateAvailable(crate::update::Manifest),
    /// O download/instalacao do update falhou.
    UpdateFailed(String),
    /// Perfil de outro computador buscado (foto ja normalizada; vazia = sem foto).
    ProfileFetched { fp: String, rev: String, about: String, photo: Vec<u8> },
    /// Resposta ao pedido de listagem da pasta que o par `fp` compartilhou.
    ShareListed { fp: String, path: String, res: Result<Listing, String> },
    /// O download de um arquivo de pasta compartilhada falhou.
    ShareFailed(String),
}

/// Uma pasta compartilhada pelo par: (nome da pasta raiz, expira em, itens).
pub type Listing = (String, u64, Vec<ShareEntry>);

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
    /// Trocavel em tempo de execucao (renomear na GUI).
    pub name: std::sync::RwLock<String>,
    pub private: Vec<u8>,
    pub public: Vec<u8>,
    pub download_dir: PathBuf,
    pub auto_accept: bool,
    pub notify: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
    pub approvals: Arc<ApprovalGate>,
    /// Recado e foto deste computador (publicos na rede).
    pub profile: std::sync::RwLock<crate::profile::Profile>,
}

impl Ctx {
    pub fn fp(&self) -> String {
        fingerprint(&self.public)
    }

    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    /// Versao do perfil para o anuncio na rede.
    pub fn profile_rev(&self) -> String {
        crate::profile::rev(&self.name(), &self.profile.read().unwrap())
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
        if self.approvals.pending.lock().unwrap().len() >= MAX_PENDING_APPROVALS {
            return false;
        }
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
/// `expect_fp`: a chave que a UI mostrou para este par; outra chave no mesmo
/// endereco e recusada (senao a mensagem iria para a conversa errada).
pub async fn connect(
    addr: &str,
    ctx: &Ctx,
    expect_fp: Option<&str>,
) -> Result<(SecureChannel<TcpStream>, String)> {
    let (chan, remote_name, remote_fp) = open(addr, ctx, expect_fp).await?;
    let is_new = trust_check(&remote_fp, &remote_name).map_err(anyhow::Error::msg)?;
    if is_new {
        let code = crate::identity::pairing_code(&ctx.fp(), &remote_fp);
        let msg = format!(
            "primeiro contato com '{remote_name}' — código de pareamento: {code} (confira com o outro aparelho)"
        );
        println!("{msg}");
        ctx.emit(Event::Notice { text: msg });
    }
    Ok((chan, remote_name))
}

/// Pede o perfil (recado + foto) de outro computador. Nao confia nele (nada vai
/// para o known_peers) e o outro lado responde sem pedir aprovacao.
pub async fn fetch_profile(addr: &str, ctx: &Ctx, fp: &str) -> Result<(String, Vec<u8>)> {
    let (mut chan, _, _) = open(addr, ctx, Some(fp)).await?;
    chan.send_control(&Control::ProfileRequest).await?;
    let (about, len) = match chan.recv().await? {
        Msg::Control(Control::Profile { about, photo_len }) => (about, photo_len as usize),
        _ => bail!("resposta inesperada ao pedido de perfil"),
    };
    let photo = if len == 0 {
        Vec::new()
    } else {
        match chan.recv().await? {
            Msg::Chunk { data, .. } if data.len() == len && len <= crate::profile::MAX_PHOTO_BYTES => data,
            _ => bail!("foto do perfil inválida"),
        }
    };
    graceful_bye(&mut chan).await;
    Ok((crate::profile::clean_about(&about), photo))
}

async fn serve_profile(chan: &mut SecureChannel<TcpStream>, ctx: &Ctx) -> Result<()> {
    let p = ctx.profile.read().unwrap().clone();
    chan.send_control(&Control::Profile {
        about: p.about,
        photo_len: p.photo.len() as u32,
    })
    .await?;
    if !p.photo.is_empty() {
        chan.send_chunk(&[0u8; 16], 0, &p.photo).await?;
    }
    Ok(())
}

/// TCP + Noise + Hello, com as checagens de identidade; nao grava confianca.
async fn open(
    addr: &str,
    ctx: &Ctx,
    expect_fp: Option<&str>,
) -> Result<(SecureChannel<TcpStream>, String, String)> {
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("nao conectei em {addr}"))?;
    stream.set_nodelay(true)?;
    let mut chan = SecureChannel::initiator(stream, &ctx.private).await?;
    chan.send_control(&Control::Hello {
        name: ctx.name(),
        fingerprint: ctx.fp(),
        version: crate::protocol::PROTO_VERSION,
    })
    .await?;
    let (remote_name, remote_fp) = match chan.recv().await? {
        Msg::Control(Control::Hello { name, fingerprint, version }) => {
            if version != crate::protocol::PROTO_VERSION {
                bail!("o outro computador usa outra versão do LocalChat (atualize os dois)");
            }
            (name, fingerprint)
        }
        _ => bail!("esperava Hello do par"),
    };
    if remote_fp != chan.remote_fingerprint() {
        bail!("segurança: o par declarou fingerprint diferente do handshake");
    }
    if expect_fp.is_some_and(|fp| fp != remote_fp) {
        bail!("segurança: quem respondeu nesse endereço tem outra chave");
    }
    Ok((chan, remote_name, remote_fp))
}

/// Conexoes entrantes atendidas ao mesmo tempo; o resto e fechado na hora.
static CONNS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);
/// Ate o Hello o par ainda e um estranho: quem nao completa nisso e cortado.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// O primeiro pedido vem logo apos o Hello (o hash do arquivo e feito antes de
/// conectar); conexao parada alem disso e fechada.
const FIRST_MSG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Pedidos de "Confiar / Recusar" na tela ao mesmo tempo (contra enxurrada na LAN).
const MAX_PENDING_APPROVALS: usize = 3;

/// Nome que aparece na tela: curto e sem caracteres de controle.
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control)
}

/// Lado servidor: atende uma conexao entrante (chat + recebimento de arquivos).
pub async fn handle_conn(stream: TcpStream, ctx: std::sync::Arc<Ctx>) -> Result<()> {
    let Ok(_permit) = CONNS.try_acquire() else {
        return Ok(()); // lotado: derruba sem gastar handshake
    };
    stream.set_nodelay(true)?;
    let hello = async {
        let mut chan = SecureChannel::responder(stream, &ctx.private).await?;
        chan.send_control(&Control::Hello {
            name: ctx.name(),
            fingerprint: ctx.fp(),
            version: crate::protocol::PROTO_VERSION,
        })
        .await?;
        let (name, fp) = match chan.recv().await? {
            Msg::Control(Control::Hello { name, fingerprint, version }) => {
                if version != crate::protocol::PROTO_VERSION {
                    bail!("o outro computador usa outra versão do LocalChat (atualize os dois)");
                }
                (name, fingerprint)
            }
            _ => bail!("esperava Hello do par"),
        };
        anyhow::Ok((chan, name, fp))
    };
    let (mut chan, remote_name, remote_fp) = tokio::time::timeout(HANDSHAKE_TIMEOUT, hello)
        .await
        .context("o par demorou demais para se apresentar")??;
    if !valid_name(&remote_name) {
        bail!("nome do par inválido");
    }
    if remote_fp != chan.remote_fingerprint() {
        bail!("segurança: o par declarou fingerprint diferente do handshake");
    }
    // o primeiro pedido decide: perfil e publico (responde sem aprovar nem confiar)
    let Some(first) = tokio::time::timeout(FIRST_MSG_TIMEOUT, chan.recv_opt())
        .await
        .context("o par conectou e não pediu nada")??
    else {
        return Ok(());
    };
    if matches!(first, Msg::Control(Control::ProfileRequest)) {
        serve_profile(&mut chan, &ctx).await?;
        // espera o Bye de quem pediu: fechar antes vira RST e a foto se perde
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), chan.recv_opt()).await;
        return Ok(());
    }
    let is_new = crate::identity::trust_status(&remote_fp, &remote_name).map_err(anyhow::Error::msg)?;
    let code = crate::identity::pairing_code(&ctx.fp(), &remote_fp);
    if is_new {
        let msg = format!(
            "primeiro contato com '{remote_name}' — código de pareamento: {code} (confira com o outro aparelho)"
        );
        println!("{msg}");
        ctx.emit(Event::Notice { text: msg });
        // aprovacao explicita: a UI (ou o timeout) decide
        if !ctx.request_approval(&remote_name, &code).await {
            let _ = chan
                .send_control(&Control::Bye {
                    reason: "o outro lado não aceitou a conexão".into(),
                })
                .await;
            // fechar com dado nao lido (a mensagem que ele ja mandou) vira RST no
            // Windows e o Bye se perde: fecha so a escrita e drena antes de sair
            let _ = chan.shutdown().await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while let Ok(Some(_)) = chan.recv_opt().await {}
            })
            .await;
            println!("conexao com '{remote_name}' recusada");
            ctx.emit(Event::Notice {
                text: format!("você recusou '{remote_name}'"),
            });
            return Ok(());
        }
    }
    crate::identity::trust_save(&remote_fp, &remote_name);

    let mut next = Some(first);
    loop {
        let msg = match next.take() {
            Some(m) => m,
            None => match chan.recv_opt().await? {
                Some(m) => m,
                None => break,
            },
        };
        match msg {
            Msg::Control(Control::Chat { text }) => {
                println!("[{}] {}", remote_name, text);
                ctx.emit(Event::Chat {
                    from: remote_name.clone(),
                    fp: remote_fp.clone(),
                    text,
                });
                chan.send_control(&Control::ChatAck).await?;
            }
            Msg::Control(Control::FileOffer { file_id, name, size }) => {
                receive_file(&mut chan, &file_id, &name, size, &ctx, &remote_name, &remote_fp)
                    .await?;
            }
            Msg::Control(Control::ShareList { path }) => {
                serve_share(&mut chan, &ctx, &remote_fp, &path, false).await?;
            }
            Msg::Control(Control::ShareGet { path }) => {
                serve_share(&mut chan, &ctx, &remote_fp, &path, true).await?;
            }
            Msg::Control(Control::Bye { .. }) => break,
            Msg::Control(_) => {}
            Msg::Chunk { .. } => bail!("chunk fora de contexto"),
        }
    }
    Ok(())
}

/// Listagem maior que isto nao e montada nem aceita.
// shortcut: pasta com dezenas de milhares de itens nao lista; paginar se alguem precisar
const MAX_LISTING: usize = 8 << 20;

/// Atende `ShareList` (`get` = false) e `ShareGet`: so para quem tem um
/// compartilhamento valido (a chave e a do handshake) e so dentro da pasta.
/// Recusa = `Bye` com o motivo, sem caminhos deste computador.
async fn serve_share(
    chan: &mut SecureChannel<TcpStream>,
    ctx: &Ctx,
    fp: &str,
    rel: &str,
    get: bool,
) -> Result<()> {
    let found = crate::share::active(fp)
        .context("nenhuma pasta compartilhada com você (ou o prazo acabou)")
        .and_then(|share| {
            let real = crate::share::resolve(&share.path, rel)?;
            if get {
                if !real.is_file() {
                    bail!("não é um arquivo");
                }
                return Ok((share, real, Vec::new()));
            }
            let json = serde_json::to_vec(&crate::share::list(&real)?)?;
            if json.len() > MAX_LISTING {
                bail!("pasta grande demais para listar");
            }
            Ok((share, real, json))
        });
    let (share, real, json) = match found {
        Ok(v) => v,
        Err(e) => return chan.send_control(&Control::Bye { reason: e.to_string() }).await,
    };
    if get {
        let sha256 = hash_file(&real).await?;
        return send_file_over_channel(chan, &real, ctx, sha256, false).await;
    }
    chan.send_control(&Control::ShareListing {
        name: share.name(),
        expires: share.expires,
        len: json.len() as u32,
    })
    .await?;
    for (i, part) in json.chunks(CHUNK_SIZE).enumerate() {
        chan.send_chunk(&[0u8; 16], i as u32, part).await?;
    }
    Ok(())
}

/// Lista `path` ("" = raiz) na pasta que o par compartilhou conosco.
pub async fn share_list(addr: &str, ctx: &Ctx, fp: &str, path: &str) -> Result<Listing> {
    let (mut chan, _) = connect(addr, ctx, Some(fp)).await?;
    chan.send_control(&Control::ShareList { path: path.to_string() })
        .await?;
    let (name, expires, len) = match chan.recv().await? {
        Msg::Control(Control::ShareListing { name, expires, len }) => (name, expires, len as usize),
        Msg::Control(Control::Bye { reason }) => bail!("{reason}"),
        _ => bail!("resposta inesperada ao pedido de listagem"),
    };
    if len > MAX_LISTING {
        bail!("pasta grande demais para listar");
    }
    let mut json = Vec::with_capacity(len);
    while json.len() < len {
        match chan.recv().await? {
            Msg::Chunk { data, .. } if !data.is_empty() => json.extend_from_slice(&data),
            _ => bail!("listagem interrompida"),
        }
    }
    graceful_bye(&mut chan).await;
    Ok((name, expires, serde_json::from_slice(&json).context("listagem inválida")?))
}

/// Baixa um arquivo da pasta que o par compartilhou: ele responde com um envio
/// normal, que cai na pasta de recebidos (com retomada, hash e progresso).
pub async fn share_get(addr: &str, ctx: &Ctx, fp: &str, path: &str) -> Result<()> {
    let (mut chan, remote) = connect(addr, ctx, Some(fp)).await?;
    chan.send_control(&Control::ShareGet { path: path.to_string() })
        .await?;
    match chan.recv().await? {
        Msg::Control(Control::FileOffer { file_id, name, size }) => {
            receive_file(&mut chan, &file_id, &name, size, ctx, &remote, fp).await?;
        }
        Msg::Control(Control::Bye { reason }) => bail!("{reason}"),
        _ => bail!("resposta inesperada ao pedido de arquivo"),
    }
    graceful_bye(&mut chan).await;
    Ok(())
}

/// Envia uma mensagem e espera a confirmacao do destino. Se ele ainda nao
/// confia em nos, isso espera o "Confiar / Recusar" de la (ate 5 min).
pub async fn send_chat(addr: &str, ctx: &Ctx, text: &str, expect_fp: Option<&str>) -> Result<String> {
    let (mut chan, remote) = connect(addr, ctx, expect_fp).await?;
    chan.send_control(&Control::Chat { text: text.to_string() })
        .await?;
    match chan.recv().await? {
        Msg::Control(Control::ChatAck) => {}
        Msg::Control(Control::Bye { reason }) => bail!("{reason}"),
        _ => bail!("resposta inesperada do par"),
    }
    graceful_bye(&mut chan).await;
    Ok(remote)
}

pub async fn send_file(addr: &str, ctx: &Ctx, path: &Path, expect_fp: Option<&str>) -> Result<String> {
    // hash do arquivo INTEIRO antes de conectar (valida tambem um pedaco retomado).
    // Em arquivo grande leva segundos: a barra (com Cancelar) ja aparece em 0%,
    // e o destino nao fica com uma conexao parada esperando
    let size = tokio::fs::metadata(path).await?.len();
    if let Some(fp) = expect_fp {
        ctx.emit(Event::Progress {
            fp: fp.to_string(),
            name: file_label(path),
            done: 0,
            total: size,
        });
    }
    let sha256 = hash_file(path).await?;
    let (mut chan, remote) = connect(addr, ctx, expect_fp).await?;
    send_file_over_channel(&mut chan, path, ctx, sha256, true).await?;
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

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("arquivo")
        .to_string()
}

async fn send_file_over_channel(
    chan: &mut SecureChannel<TcpStream>,
    path: &Path,
    ctx: &Ctx,
    sha256: String,
    // false = download de pasta compartilhada: quem serve nao ve barra nem aviso
    show: bool,
) -> Result<()> {
    // a UI mostra o progresso dentro da conversa deste par
    let peer_fp = chan.remote_fingerprint();
    let emit = |ev| {
        if show {
            ctx.emit(ev)
        }
    };
    let meta = tokio::fs::metadata(path).await?;
    if !meta.is_file() {
        bail!("nao e um arquivo: {}", path.display());
    }
    let name = file_label(path);
    let size = meta.len();
    let mut id16 = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut id16);
    let file_id = hex::encode(id16);

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
        Msg::Control(Control::Bye { reason }) => bail!("{reason}"),
        _ => bail!("resposta inesperada ao FileOffer"),
    };

    let mut file = File::open(path).await?;
    if offset > 0 {
        use tokio::io::{AsyncSeekExt, SeekFrom};
        file.seek(SeekFrom::Start(offset)).await?;
        let pct = offset * 100 / size.max(1);
        let msg = format!("retomando envio de '{name}' a partir de {pct}%");
        println!("{msg}");
        emit(Event::Notice { text: msg });
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
            emit(Event::Progress {
                name: name.clone(),
                fp: peer_fp.clone(),
                done: sent,
                total: size,
            });
        }
    }
    emit(Event::Progress {
        name: name.clone(),
        fp: peer_fp.clone(),
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
    remote_fp: &str,
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

    // write + set_len (nao append): tudo alem do offset e sobra de tentativa
    // anterior e sai, senao um .part velho corrompe todas as proximas tentativas
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part_path)
        .await
        .with_context(|| format!("abrir {}", part_path.display()))?;
    file.set_len(offset).await?;
    tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset)).await?;
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
                        fp: remote_fp.to_string(),
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
                    // .part invalido nao serve para retomar: a proxima tentativa comeca do zero
                    let _ = tokio::fs::remove_file(&part_path).await;
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
                    let _ = tokio::fs::remove_file(&part_path).await;
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
                ctx.emit(Event::Progress {
                    name: name.to_string(),
                    fp: remote_fp.to_string(),
                    done: size,
                    total: size,
                });
                ctx.emit(Event::FileReceived {
                    from: remote.to_string(),
                    fp: remote_fp.to_string(),
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
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    // Windows ignora ponto/espaco no fim e trata CON, NUL, COM1... (com qualquer
    // extensao) como dispositivo, nao arquivo
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    let stem = cleaned.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if cleaned.trim().is_empty() {
        "arquivo".into()
    } else if reserved {
        format!("_{cleaned}")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str, dir: &Path) -> Arc<Ctx> {
        let kp = snow::Builder::new(crate::identity::NOISE_PARAMS.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Arc::new(Ctx {
            name: std::sync::RwLock::new(name.into()),
            private: kp.private,
            public: kp.public,
            download_dir: dir.into(),
            auto_accept: true,
            notify: None,
            approvals: Default::default(),
            profile: Default::default(),
        })
    }

    #[test]
    fn nomes_de_arquivo_seguros() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize(r"..\..\Windows\win.ini"), if cfg!(windows) { "win.ini" } else { ".._.._Windows_win.ini" });
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("com1"), "_com1");
        assert_eq!(sanitize("console.txt"), "console.txt");
        assert_eq!(sanitize("foto. "), "foto");
        assert_eq!(sanitize("a\nb\0.txt"), "a_b_.txt");
        assert_eq!(sanitize(".."), "arquivo");
        assert!(valid_name("Ana (notebook)"));
        assert!(!valid_name("x\ny") && !valid_name(&"a".repeat(65)) && !valid_name("  "));
    }

    /// Um .part velho de OUTRO arquivo com o mesmo nome nao pode travar o nome para sempre.
    #[tokio::test]
    async fn part_velho_nao_envenena_o_nome() {
        let tmp = std::env::temp_dir().join(format!("localchat-test-{}", std::process::id()));
        let dl = tmp.join("dl");
        std::fs::create_dir_all(&dl).unwrap();
        std::env::set_var("LOCALCHAT_HOME", &tmp);
        let src = tmp.join("a.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        std::fs::write(dl.join("a.bin.part"), vec![7u8; 70_000]).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let rx = ctx("rx", &dl);
        *rx.profile.write().unwrap() = crate::profile::Profile {
            about: "na sala".into(),
            photo: vec![0xFF, 0xD8, 1, 2, 3],
        };
        let rx_fp = rx.fp();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(handle_conn(stream, rx.clone()));
            }
        });
        let tx = ctx("tx", &tmp);

        // perfil e publico: busca sem confiar e sem ser confiado (nada no known_peers)
        let (about, photo) = fetch_profile(&addr, &tx, &rx_fp).await.unwrap();
        assert_eq!((about.as_str(), photo.as_slice()), ("na sala", &[0xFF, 0xD8, 1, 2, 3][..]));
        let known = crate::identity::load_trust().peers;
        assert!(!known.contains_key(&rx_fp) && !known.contains_key(&tx.fp()));
        // pedir o perfil de quem nao e quem diz ser falha
        assert!(fetch_profile(&addr, &tx, "outra-chave").await.is_err());
        // a 1a tentativa retoma em cima do lixo e falha no hash; a 2a tem de comecar limpa
        assert!(send_file(&addr, &tx, &src, None).await.is_err());
        send_file(&addr, &tx, &src, None).await.unwrap();
        // mensagem so conta como entregue com a confirmacao do destino
        send_chat(&addr, &tx, "oi", None).await.unwrap();
        assert!(send_chat(&addr, &tx, "oi", Some("outra-chave")).await.is_err());
        assert_eq!(std::fs::read(dl.join("a.bin")).unwrap(), data);

        // pasta compartilhada: so para quem ela foi aberta, so dentro dela, so no prazo
        let shared = tmp.join("pasta");
        std::fs::create_dir_all(shared.join("sub")).unwrap();
        std::fs::write(shared.join("sub/doc.bin"), &data).unwrap();
        assert!(share_list(&addr, &tx, &rx_fp, "").await.is_err());
        let now = crate::identity::now();
        let share = |expires| Some(crate::share::Share { path: shared.clone(), expires });
        crate::share::set(&tx.fp(), share(now + 60)).unwrap();
        let (name, _, items) = share_list(&addr, &tx, &rx_fp, "").await.unwrap();
        assert_eq!((name.as_str(), items.len(), items[0].dir), ("pasta", 1, true));
        assert_eq!(share_list(&addr, &tx, &rx_fp, "sub").await.unwrap().2[0].size, data.len() as u64);
        assert!(share_list(&addr, &tx, &rx_fp, "../dl").await.is_err());
        assert!(share_get(&addr, &tx, &rx_fp, "../a.bin").await.is_err());
        assert!(share_get(&addr, &tx, &rx_fp, "sub").await.is_err());
        share_get(&addr, &tx, &rx_fp, "sub/doc.bin").await.unwrap();
        assert_eq!(std::fs::read(tmp.join("doc.bin")).unwrap(), data);
        crate::share::set(&tx.fp(), share(now - 1)).unwrap();
        assert!(share_list(&addr, &tx, &rx_fp, "").await.is_err());
        assert!(share_get(&addr, &tx, &rx_fp, "sub/doc.bin").await.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
