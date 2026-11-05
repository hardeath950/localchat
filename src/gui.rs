use crate::discovery::{self, PeerTable};
use crate::identity::{fingerprint, load_or_init};
use crate::session::{Ctx, Event};
use anyhow::{Context as _, Result};
use eframe::egui;
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

    let app = GuiApp {
        rt,
        ctx,
        table,
        events: rx,
        input: String::new(),
        log: vec![Line::Notice(format!(
            "Bem-vindo ao LocalChat! Voce e '{}'. Selecione um par na lista ao lado.",
            id.name
        ))],
        selected: None,
        status: "pronto".into(),
        me: id.name.clone(),
        fp,
        download_dir,
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
        self.log.push(Line::Mine(text.clone()));
        self.input.clear();
        self.status = format!("enviando para {name}...");

        let ctx = self.ctx.clone();
        let tx = self.ctx.notify.clone();
        self.rt.spawn(async move {
            if let Err(e) = crate::session::send_chat(&addr, &ctx, &text).await {
                if let Some(tx) = tx {
                    let _ = tx.send(Event::Notice {
                        text: format!("falha ao enviar para {name}: {e:#}"),
                    });
                }
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
        self.log.push(Line::Mine(format!("[arquivo] {label}")));
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Chat { from, text } => self.log.push(Line::From(from, text)),
                Event::FileReceived { from, path, size } => {
                    let kb = size / 1024;
                    self.log.push(Line::Notice(format!(
                        "{from} enviou um arquivo ({kb} KiB): {path}"
                    )));
                }
                Event::Notice { text } => {
                    self.status = text.clone();
                    self.log.push(Line::Notice(text));
                }
            }
        }

        egui::Panel::left("pares")
            .resizable(true)
            .default_size(210.0)
            .show(ui, |ui| {
                ui.heading("Na rede");
                ui.separator();
                let mut peers: Vec<(String, String)> = {
                    let t = self.table.lock().unwrap();
                    t.values()
                        .map(|p| (p.name.clone(), format!("{}:{}", p.ip, p.port)))
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
                for (name, addr) in peers {
                    let selected = self.selected.as_deref() == Some(name.as_str());
                    if ui.selectable_label(selected, &name).clicked() {
                        self.selected = Some(name.clone());
                        self.status = format!("conversando com {name} ({addr})");
                    }
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
