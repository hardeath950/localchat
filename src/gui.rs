use crate::discovery::{self, PeerTable};
use crate::identity::{fingerprint, load_or_init};
use crate::session::{Ctx, Event};
use anyhow::{Context as _, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

enum Line {
    From(String, String),
    Mine(String),
    Notice(String),
}

/// Linha do historico persistido (JSONL em ~/.localchat/history.jsonl).
#[derive(Serialize, Deserialize)]
struct HEntry {
    t: u64,
    k: String,
    who: String,
    text: String,
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

fn load_history() -> Vec<Line> {
    let Ok(content) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for l in content.lines() {
        let Ok(e) = serde_json::from_str::<HEntry>(l) else {
            continue;
        };
        out.push(match e.k.as_str() {
            "from" => Line::From(e.who, e.text),
            "mine" => Line::Mine(e.text),
            _ => Line::Notice(e.text),
        });
    }
    const MAX: usize = 1000;
    let skip = out.len().saturating_sub(MAX);
    out.split_off(skip)
}

fn persist(line: &Line) {
    let e = match line {
        Line::From(who, text) => HEntry {
            t: now_secs(),
            k: "from".into(),
            who: who.clone(),
            text: text.clone(),
        },
        Line::Mine(text) => HEntry {
            t: now_secs(),
            k: "mine".into(),
            who: String::new(),
            text: text.clone(),
        },
        Line::Notice(text) => HEntry {
            t: now_secs(),
            k: "notice".into(),
            who: String::new(),
            text: text.clone(),
        },
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

struct GuiApp {
    rt: tokio::runtime::Runtime,
    ctx: Arc<Ctx>,
    table: PeerTable,
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    input: String,
    log: Vec<Line>,
    selected: Option<String>,
    status: String,
    me: String,
    fp: String,
    download_dir: PathBuf,
    _tray: tray_icon::TrayIcon,
    quit: bool,
    approvals_ui: std::collections::VecDeque<(u64, String, String)>,
}

/// Icone da bandeja: balao de chat azul com tres pontinhos.
fn make_tray_icon() -> tray_icon::Icon {
    const S: usize = 32;
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let fx = x as f32;
            let fy = y as f32;
            // retangulo arredondado (balao)
            let (x0, y0, x1, y1, r) = (2.0, 3.0, 29.0, 25.0, 7.0);
            let in_bubble = {
                if fx < x0 || fx > x1 || fy < y0 || fy > y1 {
                    false
                } else {
                    let cx = fx.clamp(x0 + r, x1 - r);
                    let cy = fy.clamp(y0 + r, y1 - r);
                    (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r
                }
            };
            // rabicho
            let in_tail = fy >= 24.0 && fy <= 29.0 && fx >= 10.0 && fx <= 16.0 && (fx - 10.0) <= (29.0 - fy);
            // pontinhos brancos
            let in_dot = (y == 13 || y == 14)
                && ((10..13).contains(&x) || (15..18).contains(&x) || (20..23).contains(&x));
            let i = (y * S + x) * 4;
            if in_dot {
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if in_bubble || in_tail {
                rgba[i..i + 4].copy_from_slice(&[37, 99, 235, 255]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, S as u32, S as u32).expect("icone valido")
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
        .with_tooltip("LocalChat — chat na rede local")
        .with_icon(make_tray_icon())
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

/// Mostra a janela a partir de qualquer thread (handler da bandeja).
fn show_from_anywhere() {
    if let Ok(g) = egui_ctx_cell().lock() {
        if let Some(c) = g.as_ref() {
            c.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            c.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            c.send_viewport_cmd(egui::ViewportCommand::Focus);
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
    let (id, private, public) = load_or_init(None)?;
    let fp = fingerprint(&public);
    let download_dir = download_dir.unwrap_or_else(crate::default_download_dir);
    std::fs::create_dir_all(&download_dir)?;
    let addr: SocketAddr = listen.parse().context("listen invalido (use ip:porta)")?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = Arc::new(Ctx {
        name: id.name.clone(),
        private,
        public,
        download_dir: download_dir.clone(),
        auto_accept: true,
        notify: Some(tx.clone()),
        approvals: Default::default(),
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let table: PeerTable = Arc::new(Mutex::new(HashMap::new()));
    // from_std precisa do contexto do runtime: fazer o bind dentro dele
    let udp = Arc::new(rt.block_on(async { discovery::bind_udp(crate::protocol::DISCOVERY_PORT) })?);
    rt.spawn(discovery::announce_loop(
        udp.clone(),
        id.name.clone(),
        addr.port(),
        fp.clone(),
    ));
    rt.spawn(discovery::listen_loop(
        udp,
        table.clone(),
        fp.clone(),
        id.name.clone(),
        addr.port(),
    ));

    let listener = rt.block_on(TcpListener::bind(addr))?;
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
                                text: format!("conexao encerrada: {e:#}"),
                            });
                        }
                    });
                }
                Err(e) => {
                    let _ = accept_tx.send(Event::Notice {
                        text: format!("accept: {e}"),
                    });
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    });

    let (tray, menu_open, menu_quit) = build_tray()?;
    wire_tray_events(tx.clone(), menu_open, menu_quit);

    let app = GuiApp {
        rt,
        ctx,
        table,
        events: rx,
        input: String::new(),
        log: {
            let mut log = load_history();
            log.push(Line::Notice(format!(
                "Bem-vindo ao LocalChat! Voce e '{}'. Selecione um par na lista ao lado.",
                id.name
            )));
            log
        },
        selected: None,
        status: "pronto".into(),
        me: id.name.clone(),
        fp,
        download_dir,
        _tray: tray,
        quit: false,
        approvals_ui: std::collections::VecDeque::new(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LocalChat")
            .with_inner_size([920.0, 620.0])
            .with_min_inner_size([680.0, 440.0]),
        ..Default::default()
    };
    eframe::run_native("LocalChat", options, Box::new(move |_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("falha ao abrir a janela: {e}"))
}

impl GuiApp {
    fn show_window(&self, ctx: &egui::Context) {
        // comandos de viewport so sao processados em um pass de pintura
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn log_line(&mut self, line: Line) {
        persist(&line);
        self.log.push(line);
    }

    fn peer_addr(&self, name: &str) -> Option<String> {
        let t = self.table.lock().unwrap();
        t.values()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| format!("{}:{}", p.ip, p.port))
    }

    fn send_message(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(name) = self.selected.clone() else {
            self.status = "selecione um par na lista da esquerda".into();
            return;
        };
        let Some(addr) = self.peer_addr(&name) else {
            self.status = format!("'{name}' saiu da rede");
            return;
        };
        self.log_line(Line::Mine(text.clone()));
        self.input.clear();
        self.status = format!("enviando para {name}...");

        let ctx = self.ctx.clone();
        let tx = self.ctx.notify.clone();
        self.rt.spawn(async move {
            let res = crate::session::send_chat(&addr, &ctx, &text).await;
            if let Some(tx) = tx {
                let _ = tx.send(match res {
                    Ok(_) => Event::Notice {
                        text: format!("mensagem entregue para {name}"),
                    },
                    Err(e) => Event::Notice {
                        text: format!("falha ao enviar para {name}: {e:#}"),
                    },
                });
            }
        });
    }

    fn send_file(&mut self) {
        let Some(name) = self.selected.clone() else {
            self.status = "selecione um par na lista da esquerda".into();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Escolher arquivo para enviar")
            .pick_file()
        else {
            return;
        };
        let Some(addr) = self.peer_addr(&name) else {
            self.status = format!("'{name}' saiu da rede");
            return;
        };
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("arquivo")
            .to_string();
        self.log_line(Line::Mine(format!("[arquivo] {label}")));
        self.status = format!("enviando {label} para {name}...");

        let ctx = self.ctx.clone();
        let tx = self.ctx.notify.clone();
        self.rt.spawn(async move {
            let res = crate::session::send_file(&addr, &ctx, &path).await;
            match (res, tx) {
                (Ok(_), Some(tx)) => {
                    let _ = tx.send(Event::Notice {
                        text: format!("{label} entregue para {name}"),
                    });
                }
                (Err(e), Some(tx)) => {
                    let _ = tx.send(Event::Notice {
                        text: format!("falha ao enviar {label}: {e:#}"),
                    });
                }
                _ => {}
            }
        });
    }
}

impl eframe::App for GuiApp {
    /// Roda mesmo com a janela escondida: eventos da bandeja e do canal.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut g) = egui_ctx_cell().lock() {
            *g = Some(ctx.clone());
        }
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Chat { from, text } => self.log_line(Line::From(from, text)),
                Event::FileReceived { from, path, size } => {
                    let kb = size / 1024;
                    self.log_line(Line::Notice(format!(
                        "{from} enviou um arquivo ({kb} KiB): {path}"
                    )));
                }
                Event::Notice { text } => {
                    self.status = text.clone();
                    self.log_line(Line::Notice(text));
                }
                Event::ApprovalRequest { id, name, code } => {
                    self.approvals_ui.push_back((id, name, code));
                    show_from_anywhere();
                }
                Event::GuiShow => {
                    self.show_window(ctx);
                }
                Event::GuiQuit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // mantem o loop de logica vivo mesmo com a janela escondida (senao
        // os eventos da bandeja nunca sao lidos)
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // X na janela = esconder na bandeja (sair e pelo menu da bandeja)
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.quit {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }

        egui::Panel::left("pares")
            .resizable(true)
            .default_size(210.0)
            .show(ui, |ui| {
                ui.heading("Na rede");
                ui.separator();
                let mut peers: Vec<(String, String, String)> = {
                    let t = self.table.lock().unwrap();
                    t.values()
                        .map(|p| {
                            (
                                p.name.clone(),
                                format!("{}:{}", p.ip, p.port),
                                crate::identity::pairing_code(&self.fp, &p.fp),
                            )
                        })
                        .collect()
                };
                peers.sort();
                if peers.is_empty() {
                    ui.label("Procurando na rede...");
                    ui.label(
                        egui::RichText::new("Abra o LocalChat nos outros computadores.")
                            .weak(),
                    );
                }
                for (name, addr, code) in peers {
                    let selected = self.selected.as_deref() == Some(name.as_str());
                    if ui.selectable_label(selected, &name).clicked() {
                        self.selected = Some(name.clone());
                        self.status = format!("conversando com {name} ({addr})");
                    }
                    ui.label(egui::RichText::new(format!("codigo {code}")).weak().small());
                }
                ui.separator();
                ui.label(egui::RichText::new(format!("voce: {}", self.me)).weak());
            });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("fp {}...", &self.fp[..12])).weak(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "arquivos em: {}",
                            self.download_dir.display()
                        ))
                        .weak(),
                    );
                });
            });
        });

        // aprovacao de par novo: so confia depois do "Confiar"
        if let Some((id, name, code)) = self.approvals_ui.front().cloned() {
            egui::Window::new("Novo par quer conectar")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!("'{name}' quer conversar com voce."));
                    ui.add_space(6.0);
                    ui.label(format!("Codigo de pareamento: {code}"));
                    ui.label(
                        egui::RichText::new(
                            "Confira se o mesmo codigo aparece no aparelho dele antes de confiar.",
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

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink(false)
                .show(ui, |ui| {
                    for line in &self.log {
                        match line {
                            Line::From(who, text) => {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(egui::RichText::new(format!("{who}:")).strong());
                                    ui.label(text);
                                });
                            }
                            Line::Mine(text) => {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(egui::RichText::new("voce:").strong());
                                    ui.label(
                                        egui::RichText::new(text)
                                            .color(ui.visuals().selection.bg_fill),
                                    );
                                });
                            }
                            Line::Notice(text) => {
                                ui.label(egui::RichText::new(format!("— {text}")).weak());
                            }
                        }
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Digite uma mensagem e pressione Enter")
                        .desired_width(f32::INFINITY),
                );
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (enter && resp.has_focus()) || ui.button("Enviar").clicked() {
                    self.send_message();
                }
                if ui.button("Enviar arquivo").clicked() {
                    self.send_file();
                }
            });
        });

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(300));
    }
}
