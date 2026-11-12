use crate::discovery::{self, PeerInfo, PeerTable};
use crate::identity::{fingerprint, load_or_init, pairing_code};
use crate::session::{Ctx, Event};
use anyhow::{Context as _, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

const TOOLTIP: &str = "LocalChat — chat na rede local";
const GREEN: egui::Color32 = egui::Color32::from_rgb(34, 197, 94);
const RED: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);

#[derive(Clone, PartialEq)]
enum Sent {
    Sending,
    Ok,
    Failed(String),
}

enum Kind {
    From(String),
    /// `file` = caminho de origem, para o "tentar de novo" de um arquivo.
    Mine {
        text: String,
        state: Sent,
        file: Option<PathBuf>,
    },
}

/// Uma linha de conversa. `peer` e o fingerprint do outro lado.
struct Line {
    /// Liga o resultado do envio (Event::Sent) a linha; 0 = veio do historico.
    id: u64,
    t: u64,
    peer: String,
    kind: Kind,
}

/// Linha do historico persistido (JSONL em ~/.localchat/history.jsonl).
#[derive(Serialize, Deserialize)]
struct HEntry {
    t: u64,
    k: String,
    who: String,
    text: String,
    #[serde(default)]
    peer: String,
}

fn history_path() -> PathBuf {
    crate::identity::config_dir().join("history.jsonl")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `known`: fingerprint -> nome dos pares confiados.
fn load_history(known: &BTreeMap<String, String>) -> Vec<Line> {
    let Ok(content) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for l in content.lines() {
        let Ok(mut e) = serde_json::from_str::<HEntry>(l) else {
            continue;
        };
        // historico da v0.1 nao gravava o par: o recebido se recupera pelo
        // nome; o enviado (sem destinatario) e os avisos ficam de fora
        if e.peer.is_empty() && e.k == "from" {
            if let Some((fp, _)) = known.iter().find(|(_, n)| n.eq_ignore_ascii_case(&e.who)) {
                e.peer = fp.clone();
            }
        }
        if e.peer.is_empty() {
            continue;
        }
        let kind = match e.k.as_str() {
            "from" => Kind::From(e.text),
            "mine" => Kind::Mine {
                text: e.text,
                state: Sent::Ok,
                file: None,
            },
            _ => continue,
        };
        out.push(Line {
            id: 0,
            t: e.t,
            peer: e.peer,
            kind,
        });
    }
    const MAX: usize = 1000;
    let skip = out.len().saturating_sub(MAX);
    out.split_off(skip)
}

/// Grava o que foi de fato trocado: recebidas e enviadas com confirmacao.
fn persist(line: &Line, who: &str) {
    let (k, text) = match &line.kind {
        Kind::From(text) => ("from", text),
        Kind::Mine { text, .. } => ("mine", text),
    };
    let e = HEntry {
        t: line.t,
        k: k.into(),
        who: who.into(),
        text: text.clone(),
        peer: line.peer.clone(),
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path())
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", serde_json::to_string(&e).unwrap_or_default());
    }
}

fn fmt_time(t: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(d) = Local.timestamp_opt(t as i64, 0).single() else {
        return String::new();
    };
    let fmt = if d.date_naive() == Local::now().date_naive() {
        "%H:%M"
    } else {
        "%d/%m %H:%M"
    };
    d.format(fmt).to_string()
}

fn fmt_size(bytes: u64) -> String {
    const MB: u64 = 1 << 20;
    if bytes >= 1024 * MB {
        format!("{:.1} GB", bytes as f64 / (1024 * MB) as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn open_folder(dir: &Path) {
    let cmd = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

/// IP desta maquina na rede local (o da rota padrao; nada e enviado).
fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip())
}

struct GuiApp {
    rt: tokio::runtime::Runtime,
    ctx: Arc<Ctx>,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    table: PeerTable,
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    input: String,
    log: Vec<Line>,
    next_id: u64,
    /// fingerprint -> nome: pares confiados + quem apareceu na rede.
    contacts: BTreeMap<String, String>,
    /// Quem esta na rede neste frame, por fingerprint.
    online: HashMap<String, PeerInfo>,
    unread: HashMap<String, usize>,
    /// Total de nao lidas ja refletido no titulo e na bandeja.
    badge: usize,
    /// Fingerprint da conversa aberta.
    selected: Option<String>,
    status: String,
    fp: String,
    my_ip: String,
    download_dir: PathBuf,
    /// Nome do arquivo -> (feito, total, ultima atualizacao), envio ou recebimento.
    transfers: HashMap<String, (u64, u64, Instant)>,
    /// Envios de arquivo em andamento: id da linha -> (nome, tarefa), para cancelar.
    sending: HashMap<u64, (String, tokio::task::JoinHandle<()>)>,
    renaming: Option<String>,
    ip_input: String,
    /// None = bandeja indisponivel (ex.: Linux sem D-Bus); o X entao fecha de verdade.
    tray: Option<tray_icon::TrayIcon>,
    quit: bool,
    approvals_ui: VecDeque<(u64, String, String)>,
    /// Versao nova encontrada (manifesto assinado) e se ja esta baixando.
    update: Option<crate::update::Manifest>,
    updating: bool,
}

/// Icone do app: balao de chat azul com tres pontinhos, desenhado numa grade
/// de 32x32 e amostrado em `s`x`s`. `badge` = bolinha vermelha de nao lidas.
fn icon_rgba(s: usize, badge: bool) -> Vec<u8> {
    let k = 32.0 / s as f32;
    let mut rgba = vec![0u8; s * s * 4];
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = ((x as f32 + 0.5) * k, (y as f32 + 0.5) * k);
            // retangulo arredondado (balao)
            let (x0, y0, x1, y1, r) = (2.0, 3.0, 30.0, 26.0, 7.0);
            let in_bubble = (x0..=x1).contains(&fx) && (y0..=y1).contains(&fy) && {
                let cx = fx.clamp(x0 + r, x1 - r);
                let cy = fy.clamp(y0 + r, y1 - r);
                (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r
            };
            // rabicho
            let in_tail = (25.0..=30.0).contains(&fy)
                && (10.0..=16.0).contains(&fx)
                && (fx - 10.0) <= (30.0 - fy);
            let in_dot = [10.5f32, 16.0, 21.5]
                .iter()
                .any(|cx| (fx - cx).powi(2) + (fy - 14.5).powi(2) <= 4.0);
            let in_badge = badge && (fx - 24.5).powi(2) + (fy - 7.5).powi(2) <= 49.0;
            let color = if in_badge {
                [RED.r(), RED.g(), RED.b(), 255]
            } else if in_dot {
                [255, 255, 255, 255]
            } else if in_bubble || in_tail {
                [37, 99, 235, 255]
            } else {
                continue;
            };
            let i = (y * s + x) * 4;
            rgba[i..i + 4].copy_from_slice(&color);
        }
    }
    rgba
}

fn tray_image(badge: bool) -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(icon_rgba(32, badge), 32, 32).expect("icone valido")
}

fn build_tray() -> Result<(tray_icon::TrayIcon, tray_icon::menu::MenuId, tray_icon::menu::MenuId)> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    let menu = Menu::new();
    let mi_open = MenuItem::new("Abrir LocalChat", true, None);
    let mi_quit = MenuItem::new("Sair", true, None);
    menu.append(&mi_open)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&mi_quit)?;
    let open_id = mi_open.id().clone();
    let quit_id = mi_quit.id().clone();
    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip(TOOLTIP)
        .with_icon(tray_image(false))
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| anyhow::anyhow!("bandeja: {e}"))?;
    Ok((tray, open_id, quit_id))
}

/// Contexto egui compartilhado com os handlers da bandeja: fora da thread da
/// UI eles podem (e devem) forcar um ciclo de logica com request_repaint.
static EGUI_CTX: std::sync::OnceLock<std::sync::Mutex<Option<egui::Context>>> =
    std::sync::OnceLock::new();

fn egui_ctx_cell() -> &'static std::sync::Mutex<Option<egui::Context>> {
    EGUI_CTX.get_or_init(|| std::sync::Mutex::new(None))
}

fn wake_ui() {
    if let Ok(g) = egui_ctx_cell().lock() {
        if let Some(c) = g.as_ref() {
            c.request_repaint();
        }
    }
}

fn show_window(c: &egui::Context) {
    // comandos de viewport so sao processados em um pass de pintura
    c.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
    c.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    c.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    c.send_viewport_cmd(egui::ViewportCommand::Focus);
}

/// Mostra a janela a partir de qualquer thread (handler da bandeja).
fn show_from_anywhere() {
    if let Ok(g) = egui_ctx_cell().lock() {
        if let Some(c) = g.as_ref() {
            show_window(c);
        }
    }
}

/// Aberto pelo atalho, o Windows cria um console so para nos: solta ele.
/// Num terminal (o shell tambem esta na lista) o console fica como esta.
#[cfg(windows)]
fn detach_own_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut pids = [0u32; 2];
    // shortcut: o console ainda pisca ao abrir pelo atalho; para sumir de vez,
    // separar um binario com windows_subsystem = "windows"
    unsafe {
        if GetConsoleProcessList(pids.as_mut_ptr(), 2) == 1 {
            FreeConsole();
        }
    }
}

/// Integra os eventos da bandeja ao nosso canal de eventos.
/// O canal proprio do tray-icon 0.26 nao entrega (receiver vazio mesmo com
/// send disparando); o caminho documentado e `set_event_handler`.
fn wire_tray_events(tx: tokio::sync::mpsc::UnboundedSender<Event>, open_id: tray_icon::menu::MenuId, quit_id: tray_icon::menu::MenuId) {
    let menu_tx = tx.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev: tray_icon::menu::MenuEvent| {
        if ev.id == quit_id {
            let _ = menu_tx.send(Event::GuiQuit);
        } else if ev.id == open_id {
            let _ = menu_tx.send(Event::GuiShow);
        }
        wake_ui();
    }));
    tray_icon::TrayIconEvent::set_event_handler(Some(move |ev: tray_icon::TrayIconEvent| {
        if let tray_icon::TrayIconEvent::Click {
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Up,
            ..
        } = ev
        {
            let _ = tx.send(Event::GuiShow);
            show_from_anywhere();
            wake_ui();
        }
    }));
}

/// Sobe o no completo (descoberta + servidor) e abre a janela.
pub fn run(listen: String, download_dir: Option<PathBuf>) -> Result<()> {
    #[cfg(windows)]
    detach_own_console();
    let res = run_inner(listen, download_dir);
    if let Err(e) = &res {
        // aberto pelo atalho nao ha console: o erro tem de aparecer numa janela
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("LocalChat")
            .set_description(format!("O LocalChat não conseguiu abrir:\n\n{e:#}"))
            .show();
    }
    res
}

fn run_inner(listen: String, download_dir: Option<PathBuf>) -> Result<()> {
    let (id, private, public) = load_or_init(None)?;
    let fp = fingerprint(&public);
    let download_dir = download_dir.unwrap_or_else(crate::default_download_dir);
    std::fs::create_dir_all(&download_dir)?;
    let addr: SocketAddr = listen.parse().context("listen invalido (use ip:porta)")?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // a porta TCP primeiro: se ja tem dono, o LocalChat ja esta aberto
    // (provavelmente escondido na bandeja) e basta pedir para ele aparecer
    let listener = match rt.block_on(TcpListener::bind(addr)) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            discovery::show_running_instance();
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("não consegui abrir a porta {addr}")),
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = Arc::new(Ctx {
        name: std::sync::RwLock::new(id.name.clone()),
        private,
        public,
        download_dir: download_dir.clone(),
        auto_accept: true,
        notify: Some(tx.clone()),
        approvals: Default::default(),
    });

    let table: PeerTable = Arc::new(Mutex::new(HashMap::new()));
    discovery::load_manual();
    // from_std precisa do contexto do runtime: fazer o bind dentro dele
    let udp = Arc::new(rt.block_on(async { discovery::bind_udp(crate::protocol::DISCOVERY_PORT) })?);
    rt.spawn(discovery::announce_loop(udp.clone(), ctx.clone(), addr.port()));
    rt.spawn(discovery::listen_loop(udp, table.clone(), ctx.clone(), addr.port()));

    let accept_ctx = ctx.clone();
    let accept_tx = tx.clone();
    rt.spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _peer)) => {
                    let c = accept_ctx.clone();
                    let t = accept_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = crate::session::handle_conn(stream, c).await {
                            let _ = t.send(Event::Notice {
                                text: format!("conexão encerrada: {e:#}"),
                            });
                        }
                    });
                }
                Err(e) => {
                    let _ = accept_tx.send(Event::Notice {
                        text: format!("accept: {e}"),
                    });
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });

    // bandeja e opcional: sem ela o app abre igual e o X fecha de verdade
    let tray = match build_tray() {
        Ok((tray, menu_open, menu_quit)) => {
            wire_tray_events(tx.clone(), menu_open, menu_quit);
            Some(tray)
        }
        Err(e) => {
            let _ = tx.send(Event::Notice {
                text: format!("sem ícone na bandeja ({e:#}); fechar a janela encerra o app"),
            });
            None
        }
    };

    // procura versao nova ao abrir e a cada 6h; sem internet so nao acha nada
    let utx = tx.clone();
    std::thread::spawn(move || loop {
        match crate::update::check() {
            Ok(Some(m)) => {
                let _ = utx.send(Event::UpdateAvailable(m));
                wake_ui();
            }
            Ok(None) => {}
            Err(e) => eprintln!("update: {e:#}"),
        }
        std::thread::sleep(Duration::from_secs(6 * 3600));
    });

    let contacts: BTreeMap<String, String> = crate::identity::load_trust()
        .peers
        .into_iter()
        .map(|(fp, e)| (fp, e.name))
        .collect();

    let app = GuiApp {
        rt,
        ctx,
        tx,
        table,
        events: rx,
        input: String::new(),
        log: load_history(&contacts),
        next_id: 0,
        contacts,
        online: HashMap::new(),
        unread: HashMap::new(),
        badge: 0,
        selected: None,
        status: "pronto".into(),
        fp,
        my_ip: local_ip().map(|ip| ip.to_string()).unwrap_or_default(),
        download_dir,
        transfers: HashMap::new(),
        sending: HashMap::new(),
        renaming: None,
        ip_input: String::new(),
        tray,
        quit: false,
        approvals_ui: VecDeque::new(),
        update: None,
        updating: false,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LocalChat")
            .with_icon(egui::IconData {
                rgba: icon_rgba(64, false),
                width: 64,
                height: 64,
            })
            .with_inner_size([940.0, 640.0])
            .with_min_inner_size([680.0, 440.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LocalChat",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_zoom_factor(1.1);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("falha ao abrir a janela: {e}"))
}

impl GuiApp {
    fn name_of(&self, fp: &str) -> String {
        self.contacts.get(fp).cloned().unwrap_or_default()
    }

    /// Conversa aberta, se o par estiver online: (fingerprint, nome, ip:porta).
    fn target(&mut self) -> Option<(String, String, String)> {
        let Some(fp) = self.selected.clone() else {
            self.status = "selecione uma conversa na lista da esquerda".into();
            return None;
        };
        let name = self.name_of(&fp);
        let Some(p) = self.online.get(&fp) else {
            self.status = format!("{name} está offline");
            return None;
        };
        if p.v != crate::protocol::PROTO_VERSION {
            self.status = format!("{name} usa outra versão do LocalChat: atualize os dois computadores");
            return None;
        }
        let addr = format!("{}:{}", p.ip, p.port);
        Some((fp, name, addr))
    }

    fn push_mine(&mut self, fp: &str, text: String, file: Option<PathBuf>) -> u64 {
        self.next_id += 1;
        self.log.push(Line {
            id: self.next_id,
            t: now_secs(),
            peer: fp.to_string(),
            kind: Kind::Mine {
                text,
                state: Sent::Sending,
                file,
            },
        });
        self.next_id
    }

    fn send_text(&mut self, text: String) {
        let Some((fp, _, addr)) = self.target() else {
            return;
        };
        let id = self.push_mine(&fp, text.clone(), None);
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        self.rt.spawn(async move {
            let res = crate::session::send_chat(&addr, &ctx, &text, Some(&fp)).await;
            let _ = tx.send(Event::Sent {
                id,
                err: res.err().map(|e| format!("{e:#}")),
            });
            wake_ui();
        });
    }

    fn send_path(&mut self, path: PathBuf) {
        let Some((fp, name, addr)) = self.target() else {
            return;
        };
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("arquivo")
            .to_string();
        let id = self.push_mine(&fp, format!("Arquivo: {label}"), Some(path.clone()));
        self.status = format!("enviando {label} para {name}…");
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        let task = self.rt.spawn(async move {
            let res = crate::session::send_file(&addr, &ctx, &path, Some(&fp)).await;
            let _ = tx.send(Event::Sent {
                id,
                err: res.err().map(|e| format!("{e:#}")),
            });
            wake_ui();
        });
        self.sending.insert(id, (label, task));
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        // offline: o texto fica no campo para quando o par voltar
        if text.is_empty() || self.target().is_none() {
            return;
        }
        self.input.clear();
        self.send_text(text);
    }

    fn pick_file(&mut self) {
        if self.target().is_none() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Escolher arquivo para enviar")
            .pick_file()
        {
            self.send_path(path);
        }
    }

    /// Fecha o envio (Event::Sent ou cancelamento): atualiza a linha e grava se entregue.
    fn finish(&mut self, id: u64, err: Option<String>) {
        if let Some((name, task)) = self.sending.remove(&id) {
            task.abort();
            self.transfers.remove(&name);
        }
        let Some(line) = self.log.iter_mut().rev().find(|l| l.id == id) else {
            return;
        };
        let Kind::Mine { state, text, .. } = &mut line.kind else {
            return;
        };
        match err {
            None => {
                self.status = format!("entregue: {text}");
                *state = Sent::Ok;
                persist(line, "");
            }
            Some(e) => {
                self.status = format!("não entregue: {e}");
                *state = Sent::Failed(e);
            }
        }
    }

    fn incoming(&mut self, from: String, fp: String, text: String, focused: bool) {
        self.contacts.insert(fp.clone(), from.clone());
        if self.selected.is_none() {
            self.selected = Some(fp.clone());
        }
        if !(focused && self.selected.as_deref() == Some(fp.as_str())) {
            *self.unread.entry(fp.clone()).or_default() += 1;
        }
        let line = Line {
            id: 0,
            t: now_secs(),
            peer: fp,
            kind: Kind::From(text),
        };
        persist(&line, &from);
        self.log.push(line);
    }

    fn rename(&mut self, new: &str) {
        let new = new.trim();
        if !crate::session::valid_name(new) || new.chars().count() > 32 {
            self.status = "nome inválido: use de 1 a 32 caracteres".into();
            return;
        }
        match crate::identity::rename(new) {
            Ok(()) => {
                *self.ctx.name.write().unwrap() = new.to_string();
                self.status = format!("agora você aparece como '{new}'");
            }
            Err(e) => self.status = format!("não consegui salvar o nome: {e:#}"),
        }
    }

    fn add_ip(&mut self) {
        match self.ip_input.trim().parse::<std::net::Ipv4Addr>() {
            Ok(ip) => {
                discovery::add_manual(ip);
                self.status = format!("procurando o LocalChat em {ip}…");
                self.ip_input.clear();
            }
            Err(_) => self.status = "IP inválido: use o formato 192.168.0.10".into(),
        }
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let Some(m) = self.update.clone() else {
            return;
        };
        egui::Panel::top("update").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Nova versão {} disponível", m.version)).strong());
                if !m.notes.is_empty() {
                    ui.label(egui::RichText::new("o que mudou?").weak())
                        .on_hover_text(&m.notes);
                }
                if self.updating {
                    ui.spinner();
                    ui.label("baixando…");
                } else if ui
                    .button("Atualizar agora")
                    .on_hover_text("Baixa, confere a assinatura e reabre o LocalChat na versão nova")
                    .clicked()
                {
                    self.updating = true;
                    self.status = format!("baixando a versão {}…", m.version);
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(match crate::update::install(&m) {
                            Ok(()) => Event::GuiQuit,
                            Err(e) => Event::UpdateFailed(format!("{e:#}")),
                        });
                        wake_ui();
                    });
                }
            });
            ui.add_space(4.0);
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Abrir pasta")
                    .on_hover_text(format!(
                        "Arquivos recebidos ficam em {}",
                        self.download_dir.display()
                    ))
                    .clicked()
                {
                    open_folder(&self.download_dir);
                }
                // o resto ocupa o que sobrar, da esquerda para a direita
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let mut cancel = None;
                    for (name, (done, total, _)) in &self.transfers {
                        let short: String = name.chars().take(28).collect();
                        ui.label(egui::RichText::new(short).small());
                        ui.add(
                            egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                                .desired_width(120.0)
                                .show_percentage(),
                        );
                        let mine = self.sending.iter().find(|(_, (n, _))| n == name);
                        if let Some((id, _)) = mine {
                            if ui.small_button("Cancelar").clicked() {
                                cancel = Some(*id);
                            }
                        }
                        ui.separator();
                    }
                    if let Some(id) = cancel {
                        self.finish(id, Some("cancelado".into()));
                    }
                    ui.add(egui::Label::new(egui::RichText::new(&self.status).weak()).truncate())
                        .on_hover_text(&self.status);
                });
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("pares")
            .resizable(true)
            .default_size(230.0)
            .min_size(190.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("Conversas");
                ui.separator();

                // online primeiro, depois por nome
                let mut rows: Vec<(bool, String, String)> = self
                    .contacts
                    .iter()
                    .map(|(fp, name)| (!self.online.contains_key(fp), name.to_lowercase(), fp.clone()))
                    .collect();
                rows.sort();
                if rows.is_empty() {
                    ui.label("Procurando na rede…");
                    ui.label(
                        egui::RichText::new("Abra o LocalChat nos outros computadores.").weak(),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 150.0).max(60.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (offline, _, fp) in rows {
                            let unread = self.unread.get(&fp).copied().unwrap_or(0);
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                let color = if offline {
                                    ui.visuals().weak_text_color()
                                } else {
                                    GREEN
                                };
                                ui.painter().circle_filled(dot.center(), 4.0, color);
                                let mut name = egui::RichText::new(self.name_of(&fp));
                                if unread > 0 {
                                    name = name.strong();
                                }
                                if offline {
                                    name = name.weak();
                                }
                                let selected = self.selected.as_deref() == Some(fp.as_str());
                                let old = self
                                    .online
                                    .get(&fp)
                                    .is_some_and(|p| p.v != crate::protocol::PROTO_VERSION);
                                let hint = match (offline, old) {
                                    (true, _) => "offline",
                                    (false, true) => "outra versão do LocalChat",
                                    (false, false) => "online",
                                };
                                // a bolinha e o selo sao so visuais: o leitor de tela le isto
                                let spoken = match unread {
                                    0 => format!("{}, {hint}", self.name_of(&fp)),
                                    n => format!("{}, {hint}, {n} não lida(s)", self.name_of(&fp)),
                                };
                                if ui
                                    .selectable_label(selected, name)
                                    .on_hover_text(hint)
                                    .accessible_name(spoken)
                                    .clicked()
                                {
                                    self.selected = Some(fp.clone());
                                }
                                if unread > 0 {
                                    ui.label(
                                        egui::RichText::new(format!(" {unread} "))
                                            .small()
                                            .strong()
                                            .color(egui::Color32::WHITE)
                                            .background_color(RED),
                                    );
                                }
                            });
                        }
                    });

                ui.separator();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(mut buf) = self.renaming.take() {
                    let mut done = false;
                    ui.horizontal(|ui| {
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut buf).desired_width(130.0))
                            .accessible_name("Seu nome");
                        done = ui.button("OK").clicked() || (r.lost_focus() && enter);
                        if !done {
                            r.request_focus();
                        }
                    });
                    if done {
                        self.rename(&buf);
                    } else if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.renaming = Some(buf);
                    }
                } else {
                    let me = self.ctx.name();
                    if ui
                        .link(format!("Você: {me}"))
                        .on_hover_text("Clique para mudar o nome que os outros veem")
                        .clicked()
                    {
                        self.renaming = Some(me);
                    }
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Não achou o outro computador?").small().weak());
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.ip_input)
                            .hint_text("IP dele")
                            .desired_width(110.0),
                    )
                    .accessible_name("IP do outro computador");
                    if ui.button("Conectar").clicked() || (r.lost_focus() && enter) {
                        self.add_ip();
                    }
                });
                if !self.my_ip.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("O IP deste computador: {}", self.my_ip))
                            .small()
                            .weak(),
                    );
                }

                ui.add_space(8.0);
                if ui.small_button("Sair").clicked() {
                    self.quit = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    }

    fn approval_window(&mut self, ctx: &egui::Context) {
        // aprovacao de par novo: so confia depois do "Confiar"
        let Some((id, name, code)) = self.approvals_ui.front().cloned() else {
            return;
        };
        egui::Window::new("Novo computador quer conversar")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
            .show(ctx, |ui| {
                ui.label(format!("'{name}' quer conversar com você."));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(format!("Código de pareamento: {code}")).strong());
                ui.label(
                    egui::RichText::new(
                        "Confira se o mesmo código aparece no outro computador antes de confiar.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Confiar").clicked() {
                        self.ctx.respond_approval(id, true);
                        self.approvals_ui.pop_front();
                    }
                    if ui.button("Recusar").clicked() {
                        self.ctx.respond_approval(id, false);
                        self.approvals_ui.pop_front();
                    }
                });
            });
    }

    fn conversation(&mut self, ui: &mut egui::Ui) {
        let sel = self.selected.clone();
        let name = sel.as_deref().map(|fp| self.name_of(fp)).unwrap_or_default();
        let on = sel.as_ref().and_then(|fp| self.online.get(fp)).cloned();
        let compatible = on.as_ref().is_some_and(|p| p.v == crate::protocol::PROTO_VERSION);

        if let Some(fp) = &sel {
            egui::Panel::top("cabecalho").show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading(&name);
                    let code = pairing_code(&self.fp, fp);
                    let info = match &on {
                        Some(p) if compatible => format!("online · {} · código {code}", p.ip),
                        Some(_) => format!("outra versão do LocalChat — atualize os dois · código {code}"),
                        None => format!("offline · código {code}"),
                    };
                    ui.label(egui::RichText::new(info).weak()).on_hover_text(
                        "O código de pareamento é o mesmo nos dois computadores.\n\
                         Se aparecer diferente, não confie na conexão.",
                    );
                });
                ui.add_space(6.0);
            });
        }

        egui::Panel::bottom("entrada").show(ui, |ui| {
            ui.add_space(8.0);
            let hint = match (&sel, &on) {
                (None, _) => "Selecione uma conversa".to_string(),
                (Some(_), None) => format!("{name} está offline"),
                (Some(_), Some(_)) if !compatible => format!("{name} usa outra versão do LocalChat"),
                (Some(_), Some(_)) => format!("Mensagem para {name} — Enter envia"),
            };
            ui.add_enabled_ui(compatible, |ui| {
                // botoes a direita; o campo ocupa o que sobrar. Altura fixa: um
                // layout centralizado sem limite cresce ate ocupar o painel todo
                let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 6.0);
                ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let file = ui
                        .button("Arquivo…")
                        .on_hover_text("Ou arraste o arquivo para a janela")
                        .clicked();
                    let send = ui.button("Enviar").clicked();
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.input)
                                .hint_text(hint)
                                .desired_width(f32::INFINITY),
                        )
                        .accessible_name("Mensagem");
                    // o TextEdit solta o foco no proprio frame do Enter: e lost_focus, nao has_focus
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if enter || send {
                        self.submit();
                        resp.request_focus();
                    }
                    if file {
                        self.pick_file();
                    }
                });
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(fp) = &sel else {
                let msg = if self.contacts.is_empty() {
                    "Procurando outros computadores na rede…\n\n\
                     Abra o LocalChat neles. Se não aparecerem em alguns segundos,\n\
                     digite o IP do outro computador em \"Conectar\", à esquerda."
                } else {
                    "Selecione uma conversa à esquerda."
                };
                ui.centered_and_justified(|ui| ui.label(egui::RichText::new(msg).weak()));
                return;
            };
            if !self.log.iter().any(|l| &l.peer == fp) {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Nenhuma mensagem ainda.\n\
                             Escreva abaixo ou arraste um arquivo para esta janela.",
                        )
                        .weak(),
                    )
                });
                return;
            }
            let mut retry = None;
            egui::ScrollArea::vertical()
                .id_salt(fp)
                .stick_to_bottom(true)
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    for (i, line) in self.log.iter().enumerate().filter(|(_, l)| &l.peer == fp) {
                        if bubble(ui, line) {
                            retry = Some(i);
                        }
                    }
                    ui.add_space(6.0);
                });
            // offline: a bolha falha continua ali (target() so avisa na barra de status)
            if let Some(i) = retry.filter(|_| self.target().is_some()) {
                if let Kind::Mine { text, file, .. } = self.log.remove(i).kind {
                    match file {
                        Some(path) => self.send_path(path),
                        None => self.send_text(text),
                    }
                }
            }
        });
    }
}

/// Desenha uma mensagem. Devolve true se clicaram em "tentar de novo".
fn bubble(ui: &mut egui::Ui, line: &Line) -> bool {
    const PAD: f32 = 10.0;
    let time = fmt_time(line.t);
    let v = ui.visuals();
    let (mine, text, meta, fail) = match &line.kind {
        Kind::From(text) => (false, text, time, None),
        Kind::Mine { text, state, .. } => match state {
            Sent::Sending => (true, text, "enviando…".to_string(), None),
            Sent::Ok => (true, text, format!("{time} · entregue"), None),
            Sent::Failed(e) => (
                true,
                text,
                "⚠ não entregue · tentar de novo".to_string(),
                Some(e.as_str()),
            ),
        },
    };
    let (fill, fg) = if mine {
        (v.selection.bg_fill, v.selection.stroke.color)
    } else {
        (v.widgets.inactive.bg_fill, v.text_color())
    };
    // vermelho sobre o azul do balao fica ilegivel: a falha usa a cor cheia do texto + ⚠
    let meta_color = if fail.is_some() { fg } else { fg.gamma_multiply(0.7) };

    // largura exata do balao: o texto quebra em ~70% da area e o balao abraca o conteudo
    let avail = ui.available_width();
    let max_w = (avail * 0.7 - 2.0 * PAD).max(120.0);
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let text_w = ui.painter().layout(text.clone(), body, fg, max_w).size().x;
    let meta_w = ui.painter().layout_no_wrap(meta.clone(), small, fg).size().x;
    let w = text_w.max(meta_w).ceil() + 1.0;

    let mut retry = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if mine {
            ui.add_space((avail - w - 2.0 * PAD).max(0.0));
        }
        egui::Frame::new()
            .fill(fill)
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(PAD as i8, 6))
            .show(ui, |ui| {
                ui.set_max_width(w);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new(text).color(fg));
                    let mut meta = egui::RichText::new(meta).small().color(meta_color);
                    if fail.is_some() {
                        meta = meta.strong();
                    }
                    match fail {
                        Some(err) => {
                            retry = ui
                                .add(egui::Label::new(meta).sense(egui::Sense::click()))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(err)
                                .clicked();
                        }
                        None => {
                            ui.label(meta);
                        }
                    }
                });
            });
    });
    ui.add_space(4.0);
    retry
}

impl eframe::App for GuiApp {
    /// Roda mesmo com a janela escondida: eventos da bandeja e do canal.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut g) = egui_ctx_cell().lock() {
            *g = Some(ctx.clone());
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Chat { from, fp, text } => self.incoming(from, fp, text, focused),
                Event::FileReceived { from, fp, path, size } => {
                    let name = Path::new(&path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("arquivo");
                    let text = format!("Arquivo recebido: {name} ({})", fmt_size(size));
                    self.status = format!("{name} salvo em {}", self.download_dir.display());
                    self.incoming(from, fp, text, focused);
                }
                Event::Notice { text } => self.status = text,
                Event::Sent { id, err } => self.finish(id, err),
                Event::ApprovalRequest { id, name, code } => {
                    self.approvals_ui.push_back((id, name, code));
                    show_from_anywhere();
                }
                Event::Progress { name, done, total } => {
                    if done >= total {
                        self.transfers.remove(&name);
                    } else {
                        self.transfers.insert(name, (done, total, Instant::now()));
                    }
                }
                Event::GuiShow => show_window(ctx),
                Event::UpdateAvailable(m) => self.update = Some(m),
                Event::UpdateFailed(e) => {
                    self.updating = false;
                    self.status = format!("atualização falhou: {e}");
                }
                Event::GuiQuit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // recebimento que caiu no meio nao avisa: a barra some sozinha
        self.transfers
            .retain(|_, t| t.2.elapsed() < Duration::from_secs(10));

        // nao lidas: titulo, bandeja e pisca na barra de tarefas
        let total: usize = self.unread.values().sum();
        if total != self.badge {
            self.badge = total;
            let (title, tip) = if total > 0 {
                (
                    format!("({total}) LocalChat"),
                    format!("LocalChat — {total} não lida(s)"),
                )
            } else {
                ("LocalChat".to_string(), TOOLTIP.to_string())
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
            if let Some(tray) = &self.tray {
                let _ = tray.set_icon(Some(tray_image(total > 0)));
                let _ = tray.set_tooltip(Some(tip));
            }
            if total > 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
            }
        }

        // mantem o loop de logica vivo mesmo com a janela escondida (senao
        // os eventos da bandeja nunca sao lidos)
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // X na janela = continuar rodando em segundo plano (sair: bandeja ou botao "Sair")
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.quit && self.tray.is_some()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if cfg!(windows) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                return;
            }
            // Linux: esconder nao existe no Wayland e a bandeja pode nao estar
            // visivel (GNOME sem extensao); minimizado sempre da para reabrir
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // quem esta na rede agora; nome novo de um par conhecido vale na hora
        self.online = discovery::online(&self.table)
            .into_iter()
            .map(|p| (p.fp.clone(), p))
            .collect();
        for p in self.online.values() {
            self.contacts.insert(p.fp.clone(), p.name.clone());
        }
        // offline e sem conversa sai da lista: anuncios falsos na LAN nao a enchem
        let (online, log, sel) = (&self.online, &self.log, &self.selected);
        self.contacts.retain(|fp, _| {
            online.contains_key(fp) || sel.as_ref() == Some(fp) || log.iter().any(|l| &l.peer == fp)
        });
        // conversa aberta com a janela em foco = lida
        if ui.ctx().input(|i| i.viewport().focused.unwrap_or(false)) {
            if let Some(fp) = &self.selected {
                self.unread.remove(fp);
            }
        }
        // arquivos arrastados para a janela vao para a conversa aberta
        for f in ui.ctx().input(|i| i.raw.dropped_files.clone()) {
            self.send_path(f.path().to_path_buf());
        }

        let ctx = ui.ctx().clone();
        self.update_banner(ui);
        self.status_bar(ui);
        self.side_panel(ui);
        self.approval_window(&ctx);
        self.conversation(ui);

        ctx.request_repaint_after(Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tamanhos_legiveis() {
        assert_eq!(fmt_size(1), "1 KB");
        assert_eq!(fmt_size(1536 * 1024), "1.5 MB");
        assert_eq!(fmt_size(3 << 30), "3.0 GB");
    }

    #[test]
    fn icone_tem_balao_e_selo() {
        let px = |rgba: &[u8], x: usize, y: usize| rgba[(y * 32 + x) * 4..][..4].to_vec();
        let plain = icon_rgba(32, false);
        assert_eq!(plain.len(), 32 * 32 * 4);
        assert_eq!(px(&plain, 0, 0)[3], 0, "canto transparente");
        assert_eq!(px(&plain, 6, 20), [37, 99, 235, 255], "corpo azul");
        assert_eq!(px(&icon_rgba(32, true), 24, 7), [RED.r(), RED.g(), RED.b(), 255]);
    }
}
