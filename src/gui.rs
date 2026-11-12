use crate::discovery::{self, PeerInfo, PeerTable};
use crate::identity::{fingerprint, load_or_init, pairing_code};
use crate::session::{Ctx, Event};
use anyhow::{Context as _, Result};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

const TOOLTIP: &str = "LocalChat — chat na rede local";

// ---------------------------------------------------------------- aparencia

/// Cores do app. Layout no estilo dos mensageiros web, com a cor do LocalChat.
#[derive(Clone, Copy)]
struct Pal {
    side: Color32,
    head: Color32,
    hover: Color32,
    sel: Color32,
    line: Color32,
    text: Color32,
    weak: Color32,
    wall: Color32,
    bin: Color32,
    bout: Color32,
    field: Color32,
    accent: Color32,
    chip: Color32,
    online: Color32,
    danger: Color32,
}

const LIGHT: Pal = Pal {
    side: Color32::from_rgb(255, 255, 255),
    head: Color32::from_rgb(240, 242, 245),
    hover: Color32::from_rgb(245, 246, 246),
    sel: Color32::from_rgb(232, 237, 244),
    line: Color32::from_rgb(233, 237, 239),
    text: Color32::from_rgb(17, 27, 33),
    weak: Color32::from_rgb(102, 119, 129),
    wall: Color32::from_rgb(232, 237, 243),
    bin: Color32::from_rgb(255, 255, 255),
    bout: Color32::from_rgb(214, 231, 255),
    field: Color32::from_rgb(255, 255, 255),
    accent: Color32::from_rgb(37, 99, 235),
    chip: Color32::from_rgb(255, 255, 255),
    online: Color32::from_rgb(34, 197, 94),
    danger: Color32::from_rgb(220, 38, 38),
};

const DARK: Pal = Pal {
    side: Color32::from_rgb(17, 27, 33),
    head: Color32::from_rgb(32, 44, 51),
    hover: Color32::from_rgb(32, 44, 51),
    sel: Color32::from_rgb(42, 57, 66),
    line: Color32::from_rgb(34, 45, 52),
    text: Color32::from_rgb(233, 237, 239),
    weak: Color32::from_rgb(134, 150, 160),
    wall: Color32::from_rgb(11, 20, 26),
    bin: Color32::from_rgb(32, 44, 51),
    bout: Color32::from_rgb(25, 58, 107),
    field: Color32::from_rgb(42, 57, 66),
    accent: Color32::from_rgb(96, 150, 255),
    chip: Color32::from_rgb(24, 34, 41),
    online: Color32::from_rgb(52, 211, 113),
    danger: Color32::from_rgb(248, 113, 113),
};

fn pal(ui: &egui::Ui) -> Pal {
    if ui.visuals().dark_mode {
        DARK
    } else {
        LIGHT
    }
}

/// Cores dos avatares (uma por computador, sempre a mesma para o mesmo par).
const AVATAR: [Color32; 8] = [
    Color32::from_rgb(37, 99, 235),
    Color32::from_rgb(13, 148, 136),
    Color32::from_rgb(217, 119, 6),
    Color32::from_rgb(147, 51, 234),
    Color32::from_rgb(219, 39, 119),
    Color32::from_rgb(22, 163, 74),
    Color32::from_rgb(234, 88, 12),
    Color32::from_rgb(71, 85, 105),
];

fn avatar_color(fp: &str) -> Color32 {
    let n = fp.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    AVATAR[n as usize % AVATAR.len()]
}

/// "Ana (notebook)" -> "AN"; "desktop-abc" -> "DE".
fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let s: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("forte".into()))
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Fonte do sistema (Segoe UI no Windows; Inter/Ubuntu/Noto no Linux), com os
/// emojis do egui de reserva. Sem nenhuma, fica a fonte padrao do egui.
fn setup_fonts(ctx: &egui::Context) {
    const REGULAR: &[&str] = &[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
        "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    const BOLD: &[&str] = &[
        "C:\\Windows\\Fonts\\seguisb.ttf",
        "C:\\Windows\\Fonts\\segoeuib.ttf",
        "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
        "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ];
    let read = |paths: &[&str]| paths.iter().find_map(|p| std::fs::read(p).ok());
    let mut fonts = egui::FontDefinitions::default();
    if let Some(data) = read(REGULAR) {
        fonts
            .font_data
            .insert("sistema".into(), Arc::new(egui::FontData::from_owned(data)));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "sistema".into());
        }
    }
    let mut forte: Vec<String> = Vec::new();
    if let Some(data) = read(BOLD) {
        fonts
            .font_data
            .insert("sistema-forte".into(), Arc::new(egui::FontData::from_owned(data)));
        forte.push("sistema-forte".into());
    }
    forte.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("forte".into()), forte);
    ctx.set_fonts(fonts);
}

fn theme_path() -> PathBuf {
    crate::identity::config_dir().join("tema.txt")
}

/// Tema escolhido no menu: "claro", "escuro" ou automatico (segue o sistema).
fn load_theme() -> egui::ThemePreference {
    match std::fs::read_to_string(theme_path()).unwrap_or_default().trim() {
        "claro" => egui::ThemePreference::Light,
        "escuro" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    }
}

fn save_theme(t: egui::ThemePreference) {
    let s = match t {
        egui::ThemePreference::Light => "claro",
        egui::ThemePreference::Dark => "escuro",
        egui::ThemePreference::System => "automatico",
    };
    let _ = std::fs::write(theme_path(), s);
}

fn setup_style(ctx: &egui::Context) {
    setup_fonts(ctx);
    ctx.set_theme(load_theme());
    for (theme, p) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        ctx.style_mut_of(theme, |s| {
            use egui::TextStyle;
            s.text_styles = [
                (TextStyle::Small, font(11.5)),
                (TextStyle::Body, font(14.5)),
                (TextStyle::Button, font(14.0)),
                (TextStyle::Heading, bold(18.0)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();
            s.spacing.button_padding = vec2(12.0, 6.0);
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.interact_size.y = 30.0;
            s.spacing.menu_margin = egui::Margin::same(6);
            let v = &mut s.visuals;
            v.panel_fill = p.side;
            v.window_fill = p.side;
            // fundo dos campos de texto: no claro, branco sobre a janela branca sumia
            v.extreme_bg_color = p.head;
            v.faint_bg_color = p.head;
            v.hyperlink_color = p.accent;
            v.selection.bg_fill = p.accent.gamma_multiply(0.35);
            v.selection.stroke = Stroke::new(1.0, p.accent);
            v.window_corner_radius = CornerRadius::same(12);
            v.menu_corner_radius = CornerRadius::same(10);
            v.window_stroke = Stroke::new(1.0, p.line);
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CornerRadius::same(8);
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
            v.widgets.inactive.weak_bg_fill = p.head;
            v.widgets.inactive.bg_fill = p.head;
            v.widgets.hovered.weak_bg_fill = p.sel;
            v.widgets.hovered.bg_fill = p.sel;
            v.widgets.hovered.bg_stroke = Stroke::NONE;
            v.widgets.active.weak_bg_fill = p.sel;
        });
    }
}

// ---------------------------------------------------------------- icones

/// Botao redondo so com icone desenhado (nao depende de glifo na fonte).
fn icon_button(
    ui: &mut egui::Ui,
    size: f32,
    tip: &str,
    enabled: bool,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let p = pal(ui);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    if enabled && resp.hovered() {
        ui.painter().circle_filled(rect.center(), size / 2.0, p.sel);
    }
    let color = if enabled { p.weak } else { p.weak.gamma_multiply(0.4) };
    paint(ui.painter(), rect, color);
    let resp = resp.on_hover_text(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tip));
    resp
}

fn paint_dots(pa: &egui::Painter, r: Rect, c: Color32) {
    for dy in [-6.0, 0.0, 6.0] {
        pa.circle_filled(r.center() + vec2(0.0, dy), 2.0, c);
    }
}

fn paint_plus(pa: &egui::Painter, r: Rect, c: Color32) {
    let (m, s) = (r.center(), r.width() * 0.22);
    let st = Stroke::new(2.2, c);
    pa.line_segment([m - vec2(s, 0.0), m + vec2(s, 0.0)], st);
    pa.line_segment([m - vec2(0.0, s), m + vec2(0.0, s)], st);
}

fn paint_search(pa: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center() - vec2(1.5, 1.5);
    pa.circle_stroke(m, 5.5, Stroke::new(1.8, c));
    pa.line_segment([m + vec2(4.0, 4.0), m + vec2(8.0, 8.0)], Stroke::new(2.0, c));
}

fn paint_lock(pa: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    let body = Rect::from_center_size(m + vec2(0.0, 3.0), vec2(13.0, 10.0));
    pa.rect_filled(body, 2.0, c);
    let arc: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
            m + vec2(4.0 * a.cos(), -2.0 + 5.0 * a.sin())
        })
        .collect();
    pa.add(egui::Shape::line(arc, Stroke::new(1.8, c)));
}

fn paint_folder(pa: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    // aba + corpo
    pa.rect_filled(Rect::from_min_size(m + vec2(-8.0, -7.0), vec2(8.0, 5.0)), 1.5, c);
    pa.rect_filled(Rect::from_center_size(m + vec2(0.0, 1.0), vec2(16.0, 11.0)), 2.0, c);
}

/// Aviao de papel (enviar), branco sobre o circulo de destaque.
fn paint_send(pa: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    let pts = vec![
        m + vec2(-7.0, -7.0),
        m + vec2(8.0, 0.0),
        m + vec2(-7.0, 7.0),
        m + vec2(-4.0, 0.0),
    ];
    pa.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
}

/// ✓✓ desenhado (a fonte pode nao ter o glifo).
fn paint_ticks(pa: &egui::Painter, at: Pos2, c: Color32) {
    let st = Stroke::new(1.5, c);
    for dx in [0.0, 4.5] {
        let o = at + vec2(dx, 0.0);
        pa.line_segment([o + vec2(0.0, 4.0), o + vec2(3.0, 7.0)], st);
        pa.line_segment([o + vec2(3.0, 7.0), o + vec2(9.0, 0.0)], st);
    }
}

/// Relogio (enviando).
fn paint_clock(pa: &egui::Painter, at: Pos2, c: Color32) {
    let m = at + vec2(6.0, 4.0);
    pa.circle_stroke(m, 4.5, Stroke::new(1.2, c));
    pa.line_segment([m, m + vec2(0.0, -3.0)], Stroke::new(1.2, c));
    pa.line_segment([m, m + vec2(2.2, 0.0)], Stroke::new(1.2, c));
}

/// Foto redonda do perfil, ou as iniciais sobre a cor do par.
fn paint_avatar(
    pa: &egui::Painter,
    center: Pos2,
    radius: f32,
    name: &str,
    fp: &str,
    photo: Option<&egui::TextureHandle>,
) {
    if let Some(tex) = photo {
        let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        pa.add(
            egui::epaint::RectShape::filled(rect, CornerRadius::same(radius.min(255.0) as u8), Color32::WHITE)
                .with_texture(tex.id(), uv),
        );
        return;
    }
    pa.circle_filled(center, radius, avatar_color(fp));
    let g = pa.layout_no_wrap(initials(name), bold(radius * 0.72), Color32::WHITE);
    pa.galley(center - g.size() / 2.0, g, Color32::WHITE);
}

/// Texto numa linha so, cortado com "…" se nao couber.
fn elided(
    pa: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    pa.layout_job(job)
}

// ---------------------------------------------------------------- dados

#[derive(Clone, PartialEq)]
enum Sent {
    Sending,
    Ok,
    Failed(String),
}

enum Body {
    Text(String),
    File { name: String, size: u64, path: PathBuf },
}

/// Uma mensagem. `peer` e o fingerprint do outro lado.
struct Line {
    /// Liga o resultado do envio (Event::Sent) a linha; 0 = historico/recebida.
    id: u64,
    t: u64,
    peer: String,
    mine: bool,
    body: Body,
    state: Sent,
}

impl Line {
    /// Texto curto para a lista de conversas.
    fn preview(&self) -> String {
        match &self.body {
            Body::Text(t) => t.lines().next().unwrap_or("").to_string(),
            Body::File { name, .. } => format!("📄 {name}"),
        }
    }
}

/// Linha do historico persistido (JSONL em ~/.localchat/history.jsonl).
#[derive(Serialize, Deserialize)]
struct HEntry {
    t: u64,
    /// "from" | "mine" (texto) | "file_in" | "file_out"
    k: String,
    who: String,
    text: String,
    #[serde(default)]
    peer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    path: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    size: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
        let (mine, body) = match e.k.as_str() {
            "from" => (false, Body::Text(e.text)),
            "mine" => (true, Body::Text(e.text)),
            "file_in" | "file_out" => (
                e.k == "file_out",
                Body::File {
                    name: e.text,
                    size: e.size,
                    path: PathBuf::from(e.path),
                },
            ),
            _ => continue,
        };
        out.push(Line {
            id: 0,
            t: e.t,
            peer: e.peer,
            mine,
            body,
            state: Sent::Ok,
        });
    }
    const MAX: usize = 1000;
    let skip = out.len().saturating_sub(MAX);
    out.split_off(skip)
}

/// Grava o que foi de fato trocado: recebidas e enviadas com confirmacao.
fn persist(line: &Line, who: &str) {
    let (k, text, path, size) = match &line.body {
        Body::Text(t) => (if line.mine { "mine" } else { "from" }, t.clone(), String::new(), 0),
        Body::File { name, size, path } => (
            if line.mine { "file_out" } else { "file_in" },
            name.clone(),
            path.display().to_string(),
            *size,
        ),
    };
    let e = HEntry {
        t: line.t,
        k: k.into(),
        who: who.into(),
        text,
        peer: line.peer.clone(),
        path,
        size,
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

fn local(t: u64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(t as i64, 0).single()
}

fn hhmm(t: u64) -> String {
    local(t).map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

/// "Hoje", "Ontem" ou a data: separador de dia na conversa.
fn day_label(t: u64) -> String {
    let Some(d) = local(t) else {
        return String::new();
    };
    let today = chrono::Local::now().date_naive();
    match (today - d.date_naive()).num_days() {
        0 => "Hoje".into(),
        1 => "Ontem".into(),
        _ => d.format("%d/%m/%Y").to_string(),
    }
}

/// Hora na lista de conversas: hoje = 14:02, ontem = "Ontem", antes = data.
fn list_time(t: u64) -> String {
    match day_label(t).as_str() {
        "Hoje" => hhmm(t),
        other => other.to_string(),
    }
}

/// Uma transferencia em andamento (envio ou recebimento).
#[derive(Clone, Copy)]
struct Xfer {
    done: u64,
    total: u64,
    updated: Instant,
    /// Relogio da velocidade: comeca quando os bytes comecam a andar.
    started: Instant,
    start_done: u64,
}

impl Xfer {
    fn pct(&self) -> u64 {
        self.done * 100 / self.total.max(1)
    }

    /// Bytes por segundo, depois de 1 s de amostra.
    fn speed(&self) -> Option<f64> {
        let secs = self.started.elapsed().as_secs_f64();
        (secs >= 1.0 && self.done > self.start_done)
            .then(|| (self.done - self.start_done) as f64 / secs)
    }

    /// "45% · 230 MB de 1.2 GB · faltam 12 s"
    fn line(&self) -> String {
        if self.done == 0 {
            // conectando, esperando o destino aprovar ou relendo o trecho de uma retomada
            return format!("preparando… · {}", fmt_size(self.total));
        }
        let mut s = format!("{}% · {} de {}", self.pct(), fmt_size(self.done), fmt_size(self.total));
        if let Some(sp) = self.speed() {
            let left = (self.total.saturating_sub(self.done) as f64 / sp) as u64;
            s += &format!(" · faltam {}", fmt_eta(left));
        }
        s
    }

    fn speed_text(&self) -> String {
        match self.speed() {
            Some(sp) => format!("{}/s", fmt_size(sp as u64)),
            None => "medindo a velocidade…".into(),
        }
    }
}

fn fmt_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{} s", secs.max(1)),
        60..=3599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
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

/// Abre o arquivo no programa padrao, ou a pasta com ele selecionado.
fn open_path(path: &Path, in_folder: bool) {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("explorer");
        if in_folder {
            c.arg(format!("/select,{}", path.display()));
        } else {
            c.arg(path);
        }
        let _ = c.spawn();
    }
    #[cfg(not(windows))]
    {
        let target = if in_folder {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

/// IP desta maquina na rede local (o da rota padrao; nada e enviado).
fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// Janela por cima de tudo (uma por vez).
enum Dialog {
    None,
    /// Editar o meu perfil.
    Profile(ProfileEdit),
    ConnectIp(String),
    /// Dados de outro computador (foto, recado, codigo de seguranca).
    Contact(String),
    /// Pastas compartilhadas com um contato (a minha para ele e a dele para mim).
    Share(ShareView),
    About,
}

struct ShareView {
    fp: String,
    /// Prazo escolhido para compartilhar, em horas.
    hours: u64,
    /// A pasta que eu compartilho com este contato (lida ao abrir e ao mudar).
    mine: Option<crate::share::Share>,
    /// Pasta do outro lado que esta na tela ("" = raiz).
    path: String,
    /// None = carregando.
    listing: Option<Result<crate::session::Listing, String>>,
}

/// "13/11 às 14:02": ate quando um compartilhamento vale.
fn until(t: u64) -> String {
    local(t).map(|d| d.format("%d/%m às %H:%M").to_string()).unwrap_or_default()
}

/// Rascunho do "Meu perfil" (so vale ao clicar em Salvar).
struct ProfileEdit {
    name: String,
    about: String,
    photo: Vec<u8>,
    preview: Option<egui::TextureHandle>,
}

/// Perfil de outro computador, como chegou pela rede (ou do cache em disco).
#[derive(Default)]
struct PeerProfile {
    rev: String,
    about: String,
    photo: Option<egui::TextureHandle>,
    fetching: bool,
    last_try: Option<Instant>,
}

/// JPEG -> textura para desenhar o avatar.
fn photo_texture(ctx: &egui::Context, key: &str, jpeg: &[u8]) -> Option<egui::TextureHandle> {
    if jpeg.is_empty() {
        return None;
    }
    let img = image::load_from_memory(jpeg).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(key, color, egui::TextureOptions::LINEAR))
}

struct GuiApp {
    rt: tokio::runtime::Runtime,
    ctx: Arc<Ctx>,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    table: PeerTable,
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    input: String,
    search: String,
    log: Vec<Line>,
    next_id: u64,
    /// fingerprint -> nome: pares confiados + quem apareceu na rede.
    contacts: BTreeMap<String, String>,
    /// Quem esta na rede neste frame, por fingerprint.
    online: HashMap<String, PeerInfo>,
    /// Ultima vez que cada par foi visto online (so nesta execucao).
    last_seen: HashMap<String, u64>,
    unread: HashMap<String, usize>,
    /// Total de nao lidas ja refletido no titulo e na bandeja.
    badge: usize,
    /// Fingerprint da conversa aberta.
    selected: Option<String>,
    /// Avisos flutuantes (somem sozinhos).
    toasts: Vec<(String, Instant)>,
    fp: String,
    my_ip: String,
    download_dir: PathBuf,
    /// (fingerprint, arquivo) -> andamento.
    transfers: HashMap<(String, String), Xfer>,
    /// Perfis dos outros computadores, por fingerprint.
    profiles: HashMap<String, PeerProfile>,
    /// Minha foto (textura), refeita quando o perfil muda.
    my_photo: Option<egui::TextureHandle>,
    my_photo_dirty: bool,
    /// Envios de arquivo em andamento: id da linha -> (nome, tarefa), para cancelar.
    sending: HashMap<u64, (String, tokio::task::JoinHandle<()>)>,
    dialog: Dialog,
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
    let red = LIGHT.danger;
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
                [red.r(), red.g(), red.b(), 255]
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
        profile: std::sync::RwLock::new(crate::profile::load_own()),
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
            eprintln!("sem ícone na bandeja: {e:#}");
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
        search: String::new(),
        log: load_history(&contacts),
        next_id: 0,
        contacts,
        online: HashMap::new(),
        last_seen: HashMap::new(),
        unread: HashMap::new(),
        badge: 0,
        selected: None,
        toasts: Vec::new(),
        fp,
        my_ip: local_ip().map(|ip| ip.to_string()).unwrap_or_default(),
        download_dir,
        transfers: HashMap::new(),
        profiles: HashMap::new(),
        my_photo: None,
        my_photo_dirty: true,
        sending: HashMap::new(),
        dialog: Dialog::None,
        tray,
        quit: false,
        approvals_ui: VecDeque::new(),
        update: None,
        updating: false,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LocalChat")
            .with_app_id("localchat")
            .with_icon(egui::IconData {
                rgba: icon_rgba(64, false),
                width: 64,
                height: 64,
            })
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LocalChat",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("falha ao abrir a janela: {e}"))
}

// ---------------------------------------------------------------- logica

impl GuiApp {
    fn name_of(&self, fp: &str) -> String {
        self.contacts.get(fp).cloned().unwrap_or_default()
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push((text.into(), Instant::now()));
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    /// Conversa aberta, se o par estiver online: (fingerprint, nome, ip:porta).
    fn target(&mut self) -> Option<(String, String, String)> {
        let Some(fp) = self.selected.clone() else {
            self.toast("Abra uma conversa na lista da esquerda");
            return None;
        };
        let name = self.name_of(&fp);
        let Some(p) = self.online.get(&fp) else {
            self.toast(format!("{name} está offline"));
            return None;
        };
        if p.v != crate::protocol::PROTO_VERSION {
            self.toast(format!(
                "{name} usa outra versão do LocalChat: atualize os dois computadores"
            ));
            return None;
        }
        let addr = format!("{}:{}", p.ip, p.port);
        Some((fp, name, addr))
    }

    fn push_mine(&mut self, fp: &str, body: Body) -> u64 {
        self.next_id += 1;
        self.log.push(Line {
            id: self.next_id,
            t: now_secs(),
            peer: fp.to_string(),
            mine: true,
            body,
            state: Sent::Sending,
        });
        self.next_id
    }

    fn send_text(&mut self, text: String) {
        let Some((fp, _, addr)) = self.target() else {
            return;
        };
        let id = self.push_mine(&fp, Body::Text(text.clone()));
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
        let Some((fp, _, addr)) = self.target() else {
            return;
        };
        if path.is_dir() {
            self.toast("Pastas não podem ser enviadas: compacte em .zip antes");
            return;
        }
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("arquivo")
            .to_string();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let id = self.push_mine(
            &fp,
            Body::File {
                name: label.clone(),
                size,
                path: path.clone(),
            },
        );
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
        if let Some(paths) = rfd::FileDialog::new()
            .set_title("Escolher arquivos para enviar")
            .pick_files()
        {
            for p in paths {
                self.send_path(p);
            }
        }
    }

    /// ip:porta do par, se estiver online numa versao compativel.
    fn addr_of(&self, fp: &str) -> Option<String> {
        self.online
            .get(fp)
            .filter(|p| p.v == crate::protocol::PROTO_VERSION)
            .map(|p| format!("{}:{}", p.ip, p.port))
    }

    fn open_share(&mut self, fp: String) {
        self.dialog = Dialog::Share(ShareView {
            fp: fp.clone(),
            hours: 24,
            mine: crate::share::active(&fp),
            path: String::new(),
            listing: None,
        });
        self.share_browse(fp, String::new());
    }

    /// Pede a listagem de `path` na pasta que o par compartilhou (resposta: Event::ShareListed).
    fn share_browse(&mut self, fp: String, path: String) {
        let addr = self.addr_of(&fp);
        let Dialog::Share(v) = &mut self.dialog else {
            return;
        };
        v.path = path.clone();
        v.listing = None;
        let Some(addr) = addr else {
            v.listing = Some(Err("O outro computador está offline ou usa outra versão do LocalChat.".into()));
            return;
        };
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        self.rt.spawn(async move {
            let res = crate::session::share_list(&addr, &ctx, &fp, &path)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::ShareListed { fp, path, res });
            wake_ui();
        });
    }

    /// Baixa um arquivo da pasta do par: chega como arquivo recebido na conversa.
    fn share_download(&mut self, fp: &str, path: String) {
        let Some(addr) = self.addr_of(fp) else {
            self.toast("O outro computador está offline");
            return;
        };
        let (ctx, tx, fp) = (self.ctx.clone(), self.tx.clone(), fp.to_string());
        self.rt.spawn(async move {
            if let Err(e) = crate::session::share_get(&addr, &ctx, &fp, &path).await {
                let _ = tx.send(Event::ShareFailed(format!("{e:#}")));
                wake_ui();
            }
        });
        self.toast("Baixando: o arquivo aparece na conversa");
    }

    /// Compartilha `share` com o par (None = para de compartilhar) e atualiza a janela.
    fn share_set(&mut self, fp: &str, share: Option<crate::share::Share>) -> bool {
        if let Err(e) = crate::share::set(fp, share.clone()) {
            self.toast(format!("Não deu para salvar o compartilhamento: {e:#}"));
            return false;
        }
        if let Dialog::Share(v) = &mut self.dialog {
            v.mine = share;
        }
        true
    }

    fn share_pick(&mut self, fp: &str) {
        let Dialog::Share(v) = &self.dialog else {
            return;
        };
        let hours = v.hours;
        let Some(path) = rfd::FileDialog::new()
            .set_title("Escolher a pasta para compartilhar")
            .pick_folder()
        else {
            return;
        };
        let share = crate::share::Share {
            path,
            expires: now_secs() + hours * 3600,
        };
        let folder = share.name();
        if !self.share_set(fp, Some(share)) {
            return;
        }
        // o outro lado fica sabendo pela propria conversa (se estiver online)
        if self.addr_of(fp).is_some() && self.selected.as_deref() == Some(fp) {
            let prazo = match hours {
                1 => "1 hora".to_string(),
                168 => "7 dias".to_string(),
                h => format!("{h} horas"),
            };
            self.send_text(format!(
                "Compartilhei a pasta \"{folder}\" com você por {prazo}. Abra pelo botão de pasta no topo desta conversa."
            ));
        }
    }

    /// Fecha o envio (Event::Sent ou cancelamento): atualiza a linha e grava se entregue.
    fn finish(&mut self, id: u64, err: Option<String>) {
        if let Some((name, task)) = self.sending.remove(&id) {
            task.abort();
            self.transfers.retain(|(_, n), _| n != &name);
        }
        let Some(line) = self.log.iter_mut().rev().find(|l| l.id == id) else {
            return;
        };
        match err {
            None => {
                line.state = Sent::Ok;
                persist(line, "");
            }
            Some(e) => line.state = Sent::Failed(e),
        }
    }

    fn incoming(&mut self, from: String, fp: String, body: Body, focused: bool) {
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
            mine: false,
            body,
            state: Sent::Ok,
        };
        persist(&line, &from);
        self.log.push(line);
    }

    fn rename(&mut self, new: &str) -> bool {
        let new = new.trim();
        if !crate::session::valid_name(new) || new.chars().count() > 32 {
            self.toast("Nome inválido: use de 1 a 32 caracteres");
            return false;
        }
        match crate::identity::rename(new) {
            Ok(()) => {
                *self.ctx.name.write().unwrap() = new.to_string();
                self.toast(format!("Agora você aparece como \"{new}\""));
                true
            }
            Err(e) => {
                self.toast(format!("Não consegui salvar o nome: {e:#}"));
                false
            }
        }
    }

    fn add_ip(&mut self, ip: &str) -> bool {
        match ip.trim().parse::<std::net::Ipv4Addr>() {
            Ok(ip) => {
                discovery::add_manual(ip);
                self.toast(format!("Procurando o LocalChat em {ip}…"));
                true
            }
            Err(_) => {
                self.toast("IP inválido: use o formato 192.168.0.10");
                false
            }
        }
    }

    fn start_update(&mut self, m: crate::update::Manifest) {
        self.updating = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(match crate::update::install(&m) {
                Ok(()) => Event::GuiQuit,
                Err(e) => Event::UpdateFailed(format!("{e:#}")),
            });
            wake_ui();
        });
    }

    /// Fotos e recados: o meu (quando muda), o guardado em disco de cada contato
    /// e, de quem anunciou versao nova do perfil, busca pela rede.
    fn sync_profiles(&mut self, ctx: &egui::Context) {
        if self.my_photo_dirty {
            self.my_photo_dirty = false;
            let p = self.ctx.profile.read().unwrap().clone();
            let key = format!("minha-foto-{}", crate::profile::rev("", &p));
            self.my_photo = photo_texture(ctx, &key, &p.photo);
        }
        for fp in self.contacts.keys() {
            if !self.profiles.contains_key(fp) {
                let mut e = PeerProfile::default();
                if let Some((c, photo)) = crate::profile::load_cached(fp) {
                    e.photo = photo_texture(ctx, &format!("foto-{fp}-{}", c.rev), &photo);
                    e.rev = c.rev;
                    e.about = c.about;
                }
                self.profiles.insert(fp.clone(), e);
            }
        }
        for p in self.online.values() {
            // versao antiga nao anuncia perfil nem sabe responder o pedido
            if p.rev.is_empty() || p.v != crate::protocol::PROTO_VERSION {
                continue;
            }
            let e = self.profiles.entry(p.fp.clone()).or_default();
            let retry_ok = e.last_try.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
            if e.rev == p.rev || e.fetching || !retry_ok {
                continue;
            }
            e.fetching = true;
            let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
            let (fp, rev, addr) = (p.fp.clone(), p.rev.clone(), format!("{}:{}", p.ip, p.port));
            self.rt.spawn(async move {
                let got = tokio::time::timeout(
                    Duration::from_secs(15),
                    crate::session::fetch_profile(&addr, &ctx, &fp),
                )
                .await;
                let ev = match got {
                    Ok(Ok((about, photo))) => {
                        // a foto vem de outro computador: so entra re-codificada por nos
                        let photo = if photo.is_empty() {
                            photo
                        } else {
                            crate::profile::normalize_photo(&photo).unwrap_or_default()
                        };
                        let cached = crate::profile::Cached { rev: rev.clone(), about: about.clone() };
                        let _ = crate::profile::save_cached(&fp, &cached, &photo);
                        Event::ProfileFetched { fp, rev, about, photo }
                    }
                    _ => Event::ProfileFetched { fp, rev: String::new(), about: String::new(), photo: Vec::new() },
                };
                let _ = tx.send(ev);
                wake_ui();
            });
        }
    }

    fn open_profile(&mut self) {
        let p = self.ctx.profile.read().unwrap().clone();
        self.dialog = Dialog::Profile(ProfileEdit {
            name: self.ctx.name(),
            about: p.about,
            photo: p.photo,
            preview: None,
        });
    }

    fn photo_of(&self, fp: &str) -> Option<&egui::TextureHandle> {
        self.profiles.get(fp).and_then(|p| p.photo.as_ref())
    }

    fn about_of(&self, fp: &str) -> String {
        self.profiles.get(fp).map(|p| p.about.clone()).unwrap_or_default()
    }

    /// Salva o "Meu perfil": nome, recado e foto; o anuncio leva a versao nova.
    fn save_profile(&mut self, edit: &ProfileEdit) -> bool {
        if edit.name.trim() != self.ctx.name() && !self.rename(&edit.name) {
            return false;
        }
        let p = crate::profile::Profile {
            about: crate::profile::clean_about(&edit.about),
            photo: edit.photo.clone(),
        };
        if let Err(e) = crate::profile::save_own(&p) {
            self.toast(format!("Não consegui salvar o perfil: {e:#}"));
            return false;
        }
        *self.ctx.profile.write().unwrap() = p;
        self.my_photo_dirty = true;
        self.toast("Perfil salvo");
        true
    }

    /// "online", "visto por último hoje às 14:02", "offline"...
    fn presence(&self, fp: &str) -> (String, bool) {
        if let Some(p) = self.online.get(fp) {
            if p.v != crate::protocol::PROTO_VERSION {
                return ("usa outra versão do LocalChat — atualize os dois".into(), false);
            }
            return ("online".into(), true);
        }
        match self.last_seen.get(fp) {
            Some(&t) => {
                let day = day_label(t);
                let when = match day.as_str() {
                    "Hoje" => format!("hoje às {}", hhmm(t)),
                    "Ontem" => format!("ontem às {}", hhmm(t)),
                    d => format!("em {d}"),
                };
                (format!("visto por último {when}"), false)
            }
            None => ("offline".into(), false),
        }
    }
}

// ---------------------------------------------------------------- telas

impl GuiApp {
    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let Some(m) = self.update.clone() else {
            return;
        };
        let p = pal(ui);
        egui::Panel::top("update")
            .frame(
                egui::Frame::new()
                    .fill(p.accent.gamma_multiply(if ui.visuals().dark_mode { 0.35 } else { 0.12 }))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Nova versão {} disponível", m.version))
                            .font(bold(14.0))
                            .color(p.text),
                    );
                    if !m.notes.is_empty() {
                        ui.label(egui::RichText::new("o que mudou?").color(p.weak))
                            .on_hover_text(&m.notes);
                    }
                    if self.updating {
                        ui.spinner();
                        ui.label(egui::RichText::new("baixando e conferindo…").color(p.weak));
                    } else if primary_button(ui, p, "Atualizar agora")
                        .on_hover_text("Baixa, confere a assinatura e reabre o LocalChat na versão nova")
                        .clicked()
                    {
                        self.start_update(m);
                    }
                });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        egui::Panel::left("conversas")
            .resizable(true)
            .default_size(370.0)
            .min_size(300.0)
            .max_size(520.0)
            .frame(egui::Frame::new().fill(p.side).stroke(Stroke::new(1.0, p.line)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                self.sidebar_header(ui, p);
                self.search_box(ui, p);
                self.contact_list(ui, p);
            });
    }

    fn sidebar_header(&mut self, ui: &mut egui::Ui, p: Pal) {
        let me = self.ctx.name();
        egui::Frame::new()
            .fill(p.head)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                ui.set_min_height(40.0);
                fixed_row(ui, 40.0, |ui| {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
                    paint_avatar(ui.painter(), r.center(), 20.0, &me, &self.fp, self.my_photo.as_ref());
                    if resp
                        .on_hover_text("Meu perfil: nome, recado e foto")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.open_profile();
                    }
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&me).font(bold(15.5)).color(p.text));
                        let sub = if self.my_ip.is_empty() {
                            "Você".to_string()
                        } else {
                            format!("Você · {}", self.my_ip)
                        };
                        ui.label(egui::RichText::new(sub).font(font(12.5)).color(p.weak))
                            .on_hover_text("O IP deste computador (para \"Conectar por IP\" no outro)");
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let menu = icon_button(ui, 40.0, "Menu", true, paint_dots);
                        egui::Popup::menu(&menu).show(|ui| {
                            ui.set_min_width(220.0);
                            if ui.button("Meu perfil").clicked() {
                                self.open_profile();
                            }
                            if ui.button("Conectar por IP…").clicked() {
                                self.dialog = Dialog::ConnectIp(String::new());
                            }
                            if ui.button("Abrir pasta de recebidos").clicked() {
                                open_path(&self.download_dir, false);
                            }
                            ui.menu_button("Tema", |ui| {
                                let cur = ui.ctx().options(|o| o.theme_preference);
                                for (t, label) in [
                                    (egui::ThemePreference::System, "Automático (do sistema)"),
                                    (egui::ThemePreference::Light, "Claro"),
                                    (egui::ThemePreference::Dark, "Escuro"),
                                ] {
                                    if ui.radio(cur == t, label).clicked() {
                                        ui.ctx().set_theme(t);
                                        save_theme(t);
                                    }
                                }
                            });
                            if ui.button("Sobre o LocalChat").clicked() {
                                self.dialog = Dialog::About;
                            }
                            ui.separator();
                            if ui.button("Sair").clicked() {
                                self.quit = true;
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
                });
            });
    }

    fn search_box(&mut self, ui: &mut egui::Ui, p: Pal) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(p.head)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(24.0, 30.0), Sense::hover());
                            paint_search(ui.painter(), r, p.weak);
                            ui.add_space(6.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text("Pesquisar conversa")
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            )
                            .accessible_name("Pesquisar conversa");
                        });
                    });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.line),
        );
    }

    fn contact_list(&mut self, ui: &mut egui::Ui, p: Pal) {
        // ultima mensagem de cada conversa (para previa, hora e ordem)
        let mut last: HashMap<&str, &Line> = HashMap::new();
        for l in &self.log {
            last.insert(l.peer.as_str(), l);
        }
        let q = self.search.trim().to_lowercase();
        let mut rows: Vec<(String, String)> = self
            .contacts
            .iter()
            .filter(|(_, n)| q.is_empty() || n.to_lowercase().contains(&q))
            .map(|(fp, n)| (fp.clone(), n.clone()))
            .collect();
        // mais recente primeiro; sem conversa, online antes, depois por nome
        rows.sort_by_key(|(fp, n)| {
            (
                std::cmp::Reverse(last.get(fp.as_str()).map(|l| l.t).unwrap_or(0)),
                !self.online.contains_key(fp),
                n.to_lowercase(),
            )
        });
        let rows: Vec<(String, String, Option<(u64, String, bool, Sent)>)> = rows
            .into_iter()
            .map(|(fp, n)| {
                let l = last
                    .get(fp.as_str())
                    .map(|l| (l.t, l.preview(), l.mine, l.state.clone()));
                (fp, n, l)
            })
            .collect();

        if rows.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                if q.is_empty() {
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Procurando computadores na rede…").color(p.text));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Abra o LocalChat nos outros computadores.")
                            .font(font(13.0))
                            .color(p.weak),
                    );
                    ui.add_space(14.0);
                    if ui.link("Conectar por IP").clicked() {
                        self.dialog = Dialog::ConnectIp(String::new());
                    }
                } else {
                    ui.label(egui::RichText::new("Nenhuma conversa encontrada").color(p.weak));
                }
            });
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (fp, name, last) in rows {
                    if self.contact_row(ui, p, &fp, &name, last) {
                        self.selected = Some(fp);
                    }
                }
            });
    }

    /// Uma linha da lista. Devolve true se foi clicada.
    fn contact_row(
        &self,
        ui: &mut egui::Ui,
        p: Pal,
        fp: &str,
        name: &str,
        last: Option<(u64, String, bool, Sent)>,
    ) -> bool {
        const H: f32 = 72.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::click());
        let selected = self.selected.as_deref() == Some(fp);
        let unread = self.unread.get(fp).copied().unwrap_or(0);
        let (presence, online) = self.presence(fp);
        let pa = ui.painter();
        if selected {
            pa.rect_filled(rect, 0.0, p.sel);
        } else if resp.hovered() {
            pa.rect_filled(rect, 0.0, p.hover);
        }
        let av = pos2(rect.left() + 16.0 + 24.5, rect.center().y);
        paint_avatar(pa, av, 24.5, name, fp, self.photo_of(fp));
        if online {
            let dot = av + vec2(17.0, 17.0);
            pa.circle_filled(dot, 6.5, if selected { p.sel } else { p.side });
            pa.circle_filled(dot, 4.5, p.online);
        }
        let x0 = rect.left() + 16.0 + 49.0 + 14.0;
        let x1 = rect.right() - 16.0;
        // hora da ultima mensagem
        let mut right_w = 0.0;
        if let Some((t, ..)) = &last {
            let col = if unread > 0 { p.accent } else { p.weak };
            let g = pa.layout_no_wrap(list_time(*t), font(12.0), col);
            right_w = g.size().x + 8.0;
            pa.galley(pos2(x1 - g.size().x, rect.top() + 16.0), g, col);
        }
        let g = elided(pa, name, bold(16.0), p.text, x1 - x0 - right_w);
        pa.galley(pos2(x0, rect.top() + 13.0), g, p.text);
        // segunda linha: ✓✓ + previa, ou presenca
        let mut x = x0;
        let y2 = rect.top() + 40.0;
        let mut badge_w = 0.0;
        if unread > 0 {
            let txt = if unread > 99 { "99+".to_string() } else { unread.to_string() };
            let g = pa.layout_no_wrap(txt, bold(11.5), Color32::WHITE);
            let w = (g.size().x + 12.0).max(20.0);
            let r = Rect::from_min_size(pos2(x1 - w, y2), vec2(w, 20.0));
            pa.rect_filled(r, 10.0, p.accent);
            pa.galley(r.center() - g.size() / 2.0, g, Color32::WHITE);
            badge_w = w + 8.0;
        }
        // arquivo indo ou vindo agora: o andamento toma o lugar da previa
        let activity = self.transfers.iter().find(|((f, _), _)| f == fp).map(|((_, n), x)| {
            if x.done == 0 {
                return format!("Preparando {n}…");
            }
            let dir = if self.sending.values().any(|(sn, _)| sn == n) { "Enviando" } else { "Recebendo" };
            format!("{dir} {n} · {}%", x.pct())
        });
        let about = self.about_of(fp);
        let (text, color) = match (&last, activity) {
            (_, Some(a)) => (a, p.accent),
            (Some((_, prev, mine, state)), None) => {
                if *mine {
                    match state {
                        Sent::Ok => paint_ticks(pa, pos2(x, y2 + 5.0), p.accent),
                        Sent::Sending => paint_clock(pa, pos2(x, y2 + 5.0), p.weak),
                        Sent::Failed(_) => {
                            pa.circle_filled(pos2(x + 6.0, y2 + 9.5), 6.0, p.danger);
                        }
                    }
                    x += 18.0;
                }
                (prev.clone(), p.weak)
            }
            // sem conversa ainda: o recado do perfil, ou se esta online
            (None, None) if !about.is_empty() => (about, p.weak),
            (None, None) => (presence, if online { p.accent } else { p.weak }),
        };
        let g = elided(pa, &text, font(14.0), color, x1 - x - badge_w);
        pa.galley(pos2(x, y2), g, color);
        pa.hline(x0..=rect.right(), rect.bottom() - 0.5, Stroke::new(1.0, p.line));

        let hint = match unread {
            0 => format!("{name}, {}", self.presence(fp).0),
            n => format!("{name}, {n} não lida(s)"),
        };
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &hint));
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }

    fn chat(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let Some(fp) = self.selected.clone() else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(p.head))
                .show(ui, |ui| self.welcome(ui, p));
            return;
        };
        let name = self.name_of(&fp);
        let (presence, compatible) = self.presence(&fp);

        // cabecalho
        egui::Panel::top("chat-topo")
            .frame(
                egui::Frame::new()
                    .fill(p.head)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(Stroke::new(1.0, p.line)),
            )
            .show(ui, |ui| {
                ui.set_min_height(40.0);
                fixed_row(ui, 40.0, |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    paint_avatar(ui.painter(), r.center(), 20.0, &name, &fp, self.photo_of(&fp));
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(egui::RichText::new(&name).font(bold(16.0)).color(p.text));
                        let col = if compatible { p.accent } else { p.weak };
                        ui.label(egui::RichText::new(&presence).font(font(12.5)).color(col));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, 40.0, "Dados do contato e código de segurança", true, paint_lock).clicked() {
                            self.dialog = Dialog::Contact(fp.clone());
                        }
                        if icon_button(ui, 40.0, "Pastas compartilhadas", true, paint_folder).clicked() {
                            self.open_share(fp.clone());
                        }
                    });
                    // a faixa inteira (foto + nome) abre os dados do contato, como no WhatsApp
                    let whole = ui.min_rect();
                    let info = Rect::from_min_max(whole.min, pos2(whole.max.x - 96.0, whole.max.y));
                    if ui
                        .interact(info, ui.scope_id().with("dados-contato"), Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Dados do contato")
                        .clicked()
                    {
                        self.dialog = Dialog::Contact(fp.clone());
                    }
                });
            });

        // barra de digitacao
        egui::Panel::bottom("chat-entrada")
            .frame(
                egui::Frame::new()
                    .fill(p.head)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| self.composer(ui, p, &name, compatible, &presence));

        // mensagens
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.wall))
            .show(ui, |ui| {
                let area = ui.max_rect();
                self.messages(ui, p, &fp);
                // arrastando arquivo por cima: mostra para onde vai
                let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
                if hovering {
                    let pa = ui.painter();
                    pa.rect_filled(area, 0.0, p.wall.gamma_multiply(0.92));
                    let inner = area.shrink(24.0);
                    pa.rect_stroke(
                        inner,
                        16.0,
                        Stroke::new(2.0, p.accent),
                        egui::StrokeKind::Inside,
                    );
                    let msg = if compatible {
                        format!("Solte para enviar para {name}")
                    } else {
                        format!("{name} não pode receber agora")
                    };
                    let g = pa.layout_no_wrap(msg, bold(20.0), p.text);
                    pa.galley(inner.center() - g.size() / 2.0, g, p.text);
                }
            });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, p: Pal) {
        let r = ui.max_rect();
        let pa = ui.painter();
        let c = r.center() - vec2(0.0, 60.0);
        pa.circle_filled(c, 54.0, p.accent.gamma_multiply(0.12));
        let icon = Rect::from_center_size(c, Vec2::splat(56.0));
        // o mesmo balao do icone do app
        pa.rect_filled(
            Rect::from_min_max(icon.min + vec2(0.0, 6.0), icon.max - vec2(0.0, 8.0)),
            12.0,
            p.accent,
        );
        for dx in [-12.0, 0.0, 12.0] {
            pa.circle_filled(c + vec2(dx, -1.0), 3.5, Color32::WHITE);
        }
        let title = pa.layout_no_wrap("LocalChat".into(), bold(28.0), p.text);
        pa.galley(pos2(c.x - title.size().x / 2.0, c.y + 74.0), title, p.text);
        let lines = [
            "Mande mensagens e arquivos para os computadores da sua rede.",
            "Sem internet, sem cadastro. Escolha uma conversa à esquerda.",
        ];
        let mut y = c.y + 120.0;
        for l in lines {
            let g = pa.layout_no_wrap(l.into(), font(14.5), p.weak);
            pa.galley(pos2(c.x - g.size().x / 2.0, y), g, p.weak);
            y += 22.0;
        }
        let foot = pa.layout_no_wrap(
            "Criptografado de ponta a ponta".into(),
            font(12.5),
            p.weak,
        );
        let fpos = pos2(r.center().x - foot.size().x / 2.0 + 10.0, r.bottom() - 40.0);
        paint_lock(pa, Rect::from_center_size(fpos + vec2(-14.0, 8.0), Vec2::splat(16.0)), p.weak);
        pa.galley(fpos, foot, p.weak);
    }

    fn composer(&mut self, ui: &mut egui::Ui, p: Pal, name: &str, compatible: bool, presence: &str) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if icon_button(ui, 42.0, "Enviar arquivo (ou arraste para a janela)", compatible, paint_plus)
                .clicked()
            {
                self.pick_file();
            }
            let send_w = 46.0;
            let field_w = ui.available_width() - send_w - 8.0;
            let hint = if compatible {
                "Digite uma mensagem".to_string()
            } else if presence == "offline" || presence.starts_with("visto") {
                format!("{name} está offline")
            } else {
                format!("{name} usa outra versão do LocalChat")
            };
            let mut send = false;
            ui.add_enabled_ui(compatible, |ui| {
                egui::Frame::new()
                    .fill(p.field)
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.set_width(field_w - 28.0);
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            // o padrao (64 px) deixava o campo vazio alto demais
                            .min_scrolled_height(0.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                let resp = ui
                                    .add(
                                        egui::TextEdit::multiline(&mut self.input)
                                            .id(egui::Id::unique("mensagem"))
                                            .hint_text(hint)
                                            .frame(egui::Frame::NONE)
                                            .desired_rows(1)
                                            .desired_width(f32::INFINITY)
                                            // Shift+Enter quebra a linha; Enter envia
                                            .return_key(egui::KeyboardShortcut::new(
                                                egui::Modifiers::SHIFT,
                                                egui::Key::Enter,
                                            )),
                                    )
                                    .accessible_name("Mensagem");
                                let enter = ui.input(|i| {
                                    i.key_pressed(egui::Key::Enter) && !i.modifiers.shift
                                });
                                if resp.has_focus() && enter {
                                    send = true;
                                }
                            });
                    });
            });
            let can = compatible && !self.input.trim().is_empty();
            let (r, resp) = ui.allocate_exact_size(vec2(send_w, send_w), if can { Sense::click() } else { Sense::hover() });
            let bg = if can { p.accent } else { p.accent.gamma_multiply(0.35) };
            ui.painter().circle_filled(r.center(), send_w / 2.0 - 2.0, bg);
            paint_send(ui.painter(), r.translate(vec2(1.5, 0.0)), Color32::WHITE);
            let resp = resp.on_hover_text("Enviar (Enter)");
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, can, "Enviar"));
            if can && resp.clicked() {
                send = true;
            }
            if send {
                self.submit();
                ui.memory_mut(|m| m.request_focus(egui::Id::unique("mensagem")));
            }
        });
    }

    fn messages(&mut self, ui: &mut egui::Ui, p: Pal, fp: &str) {
        let mut retry = None;
        let mut cancel = None;
        let mut open: Option<(PathBuf, bool)> = None;
        let mut copy = None;
        egui::ScrollArea::vertical()
            .id_salt(fp)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let full = ui.available_width();
                // margem lateral da conversa (8%), como nos mensageiros web
                let side = (full * 0.06).clamp(16.0, 70.0);
                ui.add_space(14.0);
                notice_chip(
                    ui,
                    p,
                    "As mensagens e os arquivos vão direto para o outro computador, criptografados. Ninguém mais vê o conteúdo.",
                );
                let mut prev: Option<(bool, u64)> = None;
                let mut day = String::new();
                for (i, line) in self.log.iter().enumerate().filter(|(_, l)| l.peer == fp) {
                    let d = day_label(line.t);
                    if d != day {
                        ui.add_space(10.0);
                        date_chip(ui, p, &d);
                        day = d;
                        prev = None;
                    }
                    // mensagens seguidas do mesmo lado ficam juntas (sem rabicho)
                    let first = prev.is_none_or(|(m, t)| m != line.mine || line.t.saturating_sub(t) > 300);
                    ui.add_space(if first { 8.0 } else { 2.0 });
                    let progress = line.mine.then(|| self.sending.get(&line.id)).flatten().and_then(
                        |(name, _)| self.transfers.get(&(fp.to_string(), name.clone())).copied(),
                    );
                    let act = bubble(ui, p, line, first, side, progress);
                    match act {
                        Some(BubbleAction::Retry) => retry = Some(i),
                        Some(BubbleAction::Cancel) => cancel = Some(line.id),
                        Some(BubbleAction::Open(path, folder)) => open = Some((path, folder)),
                        Some(BubbleAction::Copy(t)) => copy = Some(t),
                        None => {}
                    }
                    prev = Some((line.mine, line.t));
                }
                // arquivos chegando agora deste par
                let mut incoming: Vec<_> = self
                    .transfers
                    .iter()
                    .filter(|((f, n), _)| f == fp && !self.sending.values().any(|(sn, _)| sn == n))
                    .map(|((_, n), x)| (n.clone(), *x))
                    .collect();
                incoming.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, x) in incoming {
                    ui.add_space(8.0);
                    receiving_bubble(ui, p, side, &name, &x);
                }
                ui.add_space(16.0);
            });
        if let Some(i) = retry.filter(|_| self.target().is_some()) {
            let line = self.log.remove(i);
            match line.body {
                Body::File { path, .. } => self.send_path(path),
                Body::Text(t) => self.send_text(t),
            }
        }
        if let Some(id) = cancel {
            self.finish(id, Some("cancelado".into()));
        }
        if let Some((path, folder)) = open {
            if path.exists() {
                open_path(&path, folder);
            } else {
                self.toast("Arquivo não encontrado (foi movido ou apagado?)");
            }
        }
        if let Some(t) = copy {
            ui.ctx().copy_text(t);
            self.toast("Mensagem copiada");
        }
    }

    fn approval(&mut self, ctx: &egui::Context) {
        // aprovacao de par novo: so confia depois do "Confiar"
        let Some((id, name, code)) = self.approvals_ui.front().cloned() else {
            return;
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("aprovar")).show(ctx, |ui| {
            let p = pal(ui);
            ui.set_width(380.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(egui::RichText::new("Novo computador quer conversar").font(bold(18.0)).color(p.text));
            ui.label(egui::RichText::new(format!("\"{name}\" quer conversar com você.")).color(p.text));
            code_box(ui, p, &code);
            ui.label(
                egui::RichText::new(
                    "Confira se este mesmo código aparece no outro computador (no topo da conversa) antes de confiar.",
                )
                .font(font(13.0))
                .color(p.weak),
            );
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, p, "Confiar").clicked() {
                    answer = Some(true);
                }
                if ui.button("Recusar").clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(ok) = answer {
            self.ctx.respond_approval(id, ok);
            self.approvals_ui.pop_front();
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut action: Option<Box<dyn FnOnce(&mut GuiApp) -> bool>> = None;
        match &mut self.dialog {
            Dialog::None => return,
            Dialog::Profile(edit) => {
                if edit.preview.is_none() && !edit.photo.is_empty() {
                    edit.preview = photo_texture(ctx, &format!("previa-{}", edit.photo.len()), &edit.photo);
                }
                let my_fp = self.fp.clone();
                let mut pick = false;
                let r = egui::Modal::new(egui::Id::unique("perfil")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(380.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(egui::RichText::new("Meu perfil").font(bold(18.0)).color(p.text));
                    ui.label(
                        egui::RichText::new("Os outros computadores da rede veem seu nome, recado e foto.")
                            .font(font(13.0))
                            .color(p.weak),
                    );
                    ui.add_space(6.0);
                    ui.vertical_centered(|ui| {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(112.0), Sense::click());
                        paint_avatar(ui.painter(), r.center(), 56.0, &edit.name, &my_fp, edit.preview.as_ref());
                        if resp.hovered() {
                            ui.painter().circle_filled(r.center(), 56.0, Color32::from_black_alpha(90));
                            let g = ui.painter().layout_no_wrap("Trocar foto".into(), bold(13.0), Color32::WHITE);
                            ui.painter().galley(r.center() - g.size() / 2.0, g, Color32::WHITE);
                        }
                        pick |= resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                        ui.horizontal(|ui| {
                            // centraliza os dois botoes sob a foto
                            let w = if edit.photo.is_empty() { 120.0 } else { 250.0 };
                            ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                            pick |= ui.button("Escolher foto…").clicked();
                            if !edit.photo.is_empty() && ui.button("Remover foto").clicked() {
                                edit.photo.clear();
                                edit.preview = None;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Nome").font(font(12.5)).color(p.accent));
                    ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(f32::INFINITY).char_limit(32))
                        .accessible_name("Nome");
                    ui.label(egui::RichText::new("Recado").font(font(12.5)).color(p.accent));
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.about)
                            .hint_text("Ex.: No escritório até as 18h")
                            .desired_width(f32::INFINITY)
                            .char_limit(crate::profile::MAX_ABOUT),
                    )
                    .accessible_name("Recado");
                    ui.label(
                        egui::RichText::new(format!(
                            "{}/{}",
                            edit.about.chars().count(),
                            crate::profile::MAX_ABOUT
                        ))
                        .font(font(11.5))
                        .color(p.weak),
                    );
                    let mut ok = false;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ok = primary_button(ui, p, "Salvar").clicked();
                        if ui.button("Cancelar").clicked() {
                            close = true;
                        }
                    });
                    ok
                });
                if pick {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Escolher foto do perfil")
                        .add_filter("Imagens", &["png", "jpg", "jpeg"])
                        .pick_file()
                    {
                        match std::fs::read(&path)
                            .map_err(anyhow::Error::from)
                            .and_then(|b| crate::profile::normalize_photo(&b))
                        {
                            Ok(jpg) => {
                                edit.preview = photo_texture(ctx, &format!("previa-{}", path.display()), &jpg);
                                edit.photo = jpg;
                            }
                            Err(e) => {
                                let msg = format!("Não deu para usar essa imagem: {e:#}");
                                action = Some(Box::new(move |app: &mut GuiApp| {
                                    app.toast(msg);
                                    false
                                }));
                            }
                        }
                    }
                }
                if r.inner {
                    let snapshot = ProfileEdit {
                        name: edit.name.clone(),
                        about: edit.about.clone(),
                        photo: edit.photo.clone(),
                        preview: None,
                    };
                    action = Some(Box::new(move |app: &mut GuiApp| app.save_profile(&snapshot)));
                }
                close |= r.should_close();
            }
            Dialog::ConnectIp(buf) => {
                let my_ip = self.my_ip.clone();
                let r = egui::Modal::new(egui::Id::unique("conectar")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(380.0);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    ui.label(egui::RichText::new("Conectar por IP").font(bold(18.0)).color(p.text));
                    ui.label(
                        egui::RichText::new(
                            "Use quando o outro computador não aparece sozinho na lista (VPN, várias placas de rede).",
                        )
                        .color(p.weak),
                    );
                    let te = ui.add(
                        egui::TextEdit::singleline(buf)
                            .hint_text("IP do outro computador, ex.: 192.168.0.10")
                            .desired_width(f32::INFINITY),
                    );
                    te.request_focus();
                    if !my_ip.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("O IP deste computador é {my_ip}"))
                                .font(font(12.5))
                                .color(p.weak),
                        );
                    }
                    let mut ok = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ok |= primary_button(ui, p, "Conectar").clicked();
                        if ui.button("Cancelar").clicked() {
                            close = true;
                        }
                    });
                    ok
                });
                if r.inner {
                    let ip = buf.clone();
                    action = Some(Box::new(move |app: &mut GuiApp| app.add_ip(&ip)));
                }
                close |= r.should_close();
            }
            Dialog::Contact(fp) => {
                let fp = fp.clone();
                let name = self.contacts.get(fp.as_str()).cloned().unwrap_or_default();
                let code = pairing_code(&self.fp, &fp);
                let (presence, _) = self.presence(&fp);
                let prof = self.profiles.get(&fp);
                let about = prof.map(|p| p.about.clone()).unwrap_or_default();
                let photo = prof.and_then(|p| p.photo.clone());
                let r = egui::Modal::new(egui::Id::unique("contato")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(400.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.vertical_centered(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(112.0), Sense::hover());
                        paint_avatar(ui.painter(), r.center(), 56.0, &name, &fp, photo.as_ref());
                        ui.label(egui::RichText::new(&name).font(bold(20.0)).color(p.text));
                        ui.label(egui::RichText::new(&presence).font(font(13.0)).color(p.weak));
                    });
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Recado").font(font(12.5)).color(p.accent));
                    let about = if about.is_empty() { "—".to_string() } else { about.clone() };
                    ui.label(egui::RichText::new(about).color(p.text));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Código de segurança").font(font(12.5)).color(p.accent));
                    ui.label(
                        egui::RichText::new(format!(
                            "Você e {name} veem o mesmo código. Se for diferente no outro computador, alguém pode estar no meio da conexão."
                        ))
                        .font(font(13.0))
                        .color(p.weak),
                    );
                    code_box(ui, p, &code);
                    ui.label(
                        egui::RichText::new(format!("Chave de {name}: {}…", &fp[..16]))
                            .font(font(12.0))
                            .color(p.weak),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Fechar").clicked() {
                            close = true;
                        }
                    });
                });
                close |= r.should_close();
            }
            Dialog::Share(v) => {
                enum Act {
                    Stop,
                    Pick,
                    Browse(String),
                    Get(String),
                }
                let fp = v.fp.clone();
                let name = self.contacts.get(&fp).cloned().unwrap_or_default();
                let mine = v.mine.clone().filter(|s| s.expires > now_secs());
                let mut act = None;
                let r = egui::Modal::new(egui::Id::unique("pastas")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(460.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let weak = |t: String| egui::RichText::new(t).font(font(13.0)).color(p.weak);
                    ui.label(egui::RichText::new("Pastas compartilhadas").font(bold(18.0)).color(p.text));

                    ui.label(egui::RichText::new(format!("Sua pasta para {name}")).font(font(12.5)).color(p.accent));
                    match &mine {
                        Some(s) => {
                            ui.label(egui::RichText::new(s.path.display().to_string()).color(p.text));
                            ui.label(weak(format!(
                                "{name} pode ver e baixar os arquivos até {}.",
                                until(s.expires)
                            )));
                            if ui.button("Parar de compartilhar").clicked() {
                                act = Some(Act::Stop);
                            }
                        }
                        None => {
                            ui.label(weak(format!(
                                "{name} poderá ver e baixar tudo o que estiver na pasta (sem alterar nada) até o prazo acabar."
                            )));
                            ui.horizontal(|ui| {
                                for (h, label) in [(1, "1 hora"), (24, "24 horas"), (168, "7 dias")] {
                                    ui.radio_value(&mut v.hours, h, label);
                                }
                            });
                            if ui.button("Escolher pasta…").clicked() {
                                act = Some(Act::Pick);
                            }
                        }
                    }
                    ui.separator();

                    ui.label(egui::RichText::new(format!("Pasta de {name}")).font(font(12.5)).color(p.accent));
                    match &v.listing {
                        None => {
                            ui.label(weak("Carregando…".into()));
                        }
                        Some(Err(e)) => {
                            ui.label(weak(e.clone()));
                            if ui.button("Tentar de novo").clicked() {
                                act = Some(Act::Browse(v.path.clone()));
                            }
                        }
                        Some(Ok((root, expires, items))) => {
                            ui.horizontal(|ui| {
                                if !v.path.is_empty() && ui.button("Voltar").clicked() {
                                    let up = v.path.rsplit_once('/').map_or("", |(a, _)| a);
                                    act = Some(Act::Browse(up.to_string()));
                                }
                                let here = if v.path.is_empty() {
                                    root.clone()
                                } else {
                                    format!("{root}/{}", v.path)
                                };
                                ui.add(
                                    egui::Label::new(egui::RichText::new(here).font(bold(14.0)).color(p.text))
                                        .truncate(),
                                );
                            });
                            ui.label(weak(format!(
                                "Disponível até {}. Clique num arquivo para baixar.",
                                until(*expires)
                            )));
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    if items.is_empty() {
                                        ui.label(weak("Pasta vazia".into()));
                                    }
                                    for it in items {
                                        if share_row(ui, p, it) {
                                            let full = if v.path.is_empty() {
                                                it.name.clone()
                                            } else {
                                                format!("{}/{}", v.path, it.name)
                                            };
                                            act = Some(if it.dir { Act::Browse(full) } else { Act::Get(full) });
                                        }
                                    }
                                });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Fechar").clicked() {
                            close = true;
                        }
                    });
                });
                if let Some(a) = act {
                    action = Some(Box::new(move |app: &mut GuiApp| {
                        match a {
                            Act::Stop => {
                                app.share_set(&fp, None);
                            }
                            Act::Pick => app.share_pick(&fp),
                            Act::Browse(path) => app.share_browse(fp, path),
                            Act::Get(path) => app.share_download(&fp, path),
                        }
                        false
                    }));
                }
                close |= r.should_close();
            }
            Dialog::About => {
                let (fp, dir) = (self.fp.clone(), self.download_dir.display().to_string());
                let r = egui::Modal::new(egui::Id::unique("sobre")).show(ctx, |ui| {
                    let p = pal(ui);
                    ui.set_width(400.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(egui::RichText::new("LocalChat").font(bold(20.0)).color(p.text));
                    ui.label(egui::RichText::new(format!("Versão {}", env!("CARGO_PKG_VERSION"))).color(p.weak));
                    ui.label(egui::RichText::new(format!("Arquivos recebidos: {dir}")).font(font(13.0)).color(p.weak));
                    ui.label(egui::RichText::new(format!("Sua chave: {}…", &fp[..16])).font(font(13.0)).color(p.weak));
                    ui.hyperlink_to("Página do projeto", "https://github.com/hardeath950/localchat");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Fechar").clicked() {
                            close = true;
                        }
                    });
                });
                close |= r.should_close();
            }
        }
        if let Some(f) = action {
            close |= f(self);
        }
        if close {
            self.dialog = Dialog::None;
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|(_, t)| t.elapsed() < Duration::from_secs(4));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("avisos"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -86.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for (text, _) in &self.toasts {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(38, 50, 56))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(egui::RichText::new(text).color(Color32::WHITE));
                        });
                }
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

/// Botao principal (fundo da cor de destaque, texto branco).
fn primary_button(ui: &mut egui::Ui, p: Pal, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).font(bold(14.0)).color(Color32::WHITE))
            .fill(p.accent),
    )
}

/// Linha de altura fixa, itens centralizados na vertical (o `horizontal_centered`
/// do egui ocupa toda a altura que sobra e empurra o resto do painel).
fn fixed_row<R>(ui: &mut egui::Ui, h: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = vec2(ui.available_width(), h);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), add)
        .inner
}

/// Linha da listagem de uma pasta compartilhada; true = clicada.
fn share_row(ui: &mut egui::Ui, p: Pal, it: &crate::protocol::ShareEntry) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let pa = ui.painter();
    if resp.hovered() {
        pa.rect_filled(rect, 6.0, p.hover);
    }
    let (x0, x1, y) = (rect.left() + 8.0, rect.right() - 8.0, rect.center().y);
    let mut right_w = 0.0;
    if it.dir {
        paint_folder(pa, Rect::from_center_size(pos2(x0 + 9.0, y), Vec2::splat(18.0)), p.weak);
    } else {
        let g = pa.layout_no_wrap(fmt_size(it.size), font(12.0), p.weak);
        right_w = g.size().x + 12.0;
        pa.galley(pos2(x1 - g.size().x, y - g.size().y / 2.0), g, p.weak);
    }
    let g = elided(pa, &it.name, font(14.0), p.text, x1 - x0 - 28.0 - right_w);
    pa.galley(pos2(x0 + 28.0, y - g.size().y / 2.0), g, p.text);
    let hint = if it.dir {
        format!("Abrir a pasta {}", it.name)
    } else {
        format!("Baixar {} ({})", it.name, fmt_size(it.size))
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &hint));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Faixa centralizada (aviso de criptografia), com cadeado desenhado.
fn notice_chip(ui: &mut egui::Ui, p: Pal, text: &str) {
    let max_w = (ui.available_width() - 48.0).min(440.0);
    let g = ui.painter().layout(text.to_string(), font(12.5), p.weak, max_w - 44.0);
    let size = g.size() + vec2(44.0, 16.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), size.y), Sense::hover());
    let r = Rect::from_center_size(row.center(), size);
    ui.painter().rect_filled(r, 8.0, p.chip);
    paint_lock(
        ui.painter(),
        Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), Vec2::splat(16.0)),
        p.weak,
    );
    ui.painter().galley(r.min + vec2(34.0, 8.0), g, p.weak);
}

/// Pilula centralizada com o dia ("Hoje", "Ontem", 10/11/2026).
fn date_chip(ui: &mut egui::Ui, p: Pal, text: &str) {
    let g = ui.painter().layout_no_wrap(text.to_string(), font(12.5), p.weak);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 12.0), Sense::hover());
    let pill = Rect::from_center_size(row.center(), g.size() + vec2(24.0, 12.0));
    ui.painter().rect_filled(pill, 8.0, p.chip);
    ui.painter().galley(pill.center() - g.size() / 2.0, g, p.weak);
}

/// Codigo de pareamento em destaque: "707 577".
fn code_box(ui: &mut egui::Ui, p: Pal, code: &str) {
    let spaced = format!("{} {}", &code[..3], &code[3..]);
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(p.head)
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(24, 12))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(spaced).font(bold(30.0)).color(p.text));
            });
    });
}

enum BubbleAction {
    Retry,
    Cancel,
    Open(PathBuf, bool),
    Copy(String),
}

const BUBBLE_PAD: Vec2 = vec2(10.0, 7.0);

/// Desenha uma mensagem (balao com rabicho, hora e ✓✓ no canto).
fn bubble(
    ui: &mut egui::Ui,
    p: Pal,
    line: &Line,
    first: bool,
    side: f32,
    progress: Option<Xfer>,
) -> Option<BubbleAction> {
    let full = ui.available_width();
    let max_w = ((full - 2.0 * side) * 0.72).clamp(160.0, 620.0);
    let pa = ui.painter().clone();
    let fill = if line.mine { p.bout } else { p.bin };
    let failed = matches!(line.state, Sent::Failed(_));

    // canto: hora + status
    let meta_txt = if failed {
        "Não entregue · toque para tentar de novo".to_string()
    } else {
        hhmm(line.t)
    };
    let meta_col = if failed { p.danger } else { p.weak };
    let meta = pa.layout_no_wrap(meta_txt, font(11.0), meta_col);
    let status_w = if line.mine && !failed { 18.0 } else { 0.0 };
    let meta_w = meta.size().x + status_w;

    // conteudo
    let (content, inline) = match &line.body {
        Body::Text(t) => {
            let g = pa.layout(t.clone(), font(14.5), p.text, max_w - 2.0 * BUBBLE_PAD.x);
            let last_w = g.rows.last().map(|r| r.rect().width()).unwrap_or(0.0);
            // a hora cabe ao lado da ultima linha? entao nao gasta linha extra
            let inline = last_w + 10.0 + meta_w <= max_w - 2.0 * BUBBLE_PAD.x;
            let w = if inline { g.size().x.max(last_w + 10.0 + meta_w) } else { g.size().x.max(meta_w) };
            (Content::Text(g, vec2(w, 0.0)), inline)
        }
        Body::File { name, size, .. } => (Content::File(name.clone(), *size), false),
    };
    let body_size = match &content {
        Content::Text(g, w) => vec2(w.x, g.size().y),
        Content::File(..) => vec2((max_w - 2.0 * BUBBLE_PAD.x).min(360.0), 48.0 + if progress.is_some() { 14.0 } else { 0.0 }),
    };
    let h = body_size.y + if inline { 0.0 } else { meta.size().y + 2.0 } + 2.0 * BUBBLE_PAD.y;
    let w = body_size.x + 2.0 * BUBBLE_PAD.x;

    let (row, _) = ui.allocate_exact_size(vec2(full, h), Sense::hover());
    let x = if line.mine { row.right() - side - w } else { row.left() + side };
    let rect = Rect::from_min_size(pos2(x, row.top()), vec2(w, h));
    let resp = ui.interact(rect, ui.scope_id().with(("balao", line.t, line.id, rect.left() as i32)), Sense::click());

    // fundo + rabicho no primeiro balao do grupo
    let mut cr = CornerRadius::same(8);
    if first {
        if line.mine {
            cr.ne = 0;
        } else {
            cr.nw = 0;
        }
    }
    pa.rect_filled(rect, cr, fill);
    if first {
        let (a, b, c) = if line.mine {
            (rect.right_top(), rect.right_top() + vec2(8.0, 0.0), rect.right_top() + vec2(0.0, 10.0))
        } else {
            (rect.left_top(), rect.left_top() - vec2(8.0, 0.0), rect.left_top() + vec2(0.0, 10.0))
        };
        pa.add(egui::Shape::convex_polygon(vec![a, b, c], fill, Stroke::NONE));
    }

    let inner = rect.shrink2(BUBBLE_PAD);
    let mut action = None;
    match content {
        Content::Text(g, _) => {
            pa.galley(inner.min, g.clone(), p.text);
            if resp.secondary_clicked() || resp.double_clicked() {
                // copiar sem selecionar: botao direito ou duplo clique
                if let Body::Text(t) = &line.body {
                    action = Some(BubbleAction::Copy(t.clone()));
                }
            }
        }
        Content::File(name, size) => {
            let icon = Rect::from_min_size(inner.min, vec2(40.0, 48.0));
            pa.rect_filled(icon, 6.0, p.accent.gamma_multiply(0.85));
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_uppercase();
            let ext: String = ext.chars().take(4).collect();
            let g = pa.layout_no_wrap(if ext.is_empty() { "ARQ".into() } else { ext }, bold(10.5), Color32::WHITE);
            pa.galley(icon.center() - g.size() / 2.0, g, Color32::WHITE);
            let tx = icon.right() + 10.0;
            let tw = inner.right() - tx;
            let g = elided(&pa, &name, bold(14.0), p.text, tw);
            pa.galley(pos2(tx, inner.top() + 5.0), g, p.text);
            let sub = match (&progress, line.mine) {
                (Some(x), _) => x.line(),
                (None, true) => fmt_size(size),
                (None, false) => format!("{} · clique para abrir", fmt_size(size)),
            };
            let g = elided(&pa, &sub, font(12.0), p.weak, tw - if progress.is_some() { 24.0 } else { 0.0 });
            pa.galley(pos2(tx, inner.top() + 27.0), g, p.weak);
            if let Some(x) = progress {
                let bar = Rect::from_min_size(pos2(inner.left(), inner.top() + 54.0), vec2(inner.width(), 4.0));
                paint_bar(&pa, p, bar, &x);
                ui.interact(bar.expand(4.0), ui.scope_id().with(("barra", line.id)), Sense::hover())
                    .on_hover_text(x.speed_text());
                // X para cancelar
                let xr = Rect::from_center_size(pos2(inner.right() - 9.0, inner.top() + 33.0), Vec2::splat(18.0));
                let xresp = ui.interact(xr, ui.scope_id().with(("cancelar", line.id)), Sense::click());
                let col = if xresp.hovered() { p.danger } else { p.weak };
                pa.line_segment([xr.left_top() + vec2(4.0, 4.0), xr.right_bottom() - vec2(4.0, 4.0)], Stroke::new(1.8, col));
                pa.line_segment([xr.right_top() + vec2(-4.0, 4.0), xr.left_bottom() + vec2(4.0, -4.0)], Stroke::new(1.8, col));
                if xresp.on_hover_text("Cancelar envio").clicked() {
                    action = Some(BubbleAction::Cancel);
                }
            }
            if let Body::File { path, .. } = &line.body {
                if !line.mine && resp.clicked() {
                    action = Some(BubbleAction::Open(path.clone(), false));
                }
                if resp.secondary_clicked() {
                    action = Some(BubbleAction::Open(path.clone(), true));
                }
            }
        }
    }

    // hora + ✓✓ no canto inferior direito
    let my = rect.bottom() - BUBBLE_PAD.y - meta.size().y + 1.0;
    let mx = rect.right() - BUBBLE_PAD.x - meta_w;
    pa.galley(pos2(mx, my), meta, meta_col);
    if line.mine && !failed {
        let at = pos2(rect.right() - BUBBLE_PAD.x - 14.0, my + 2.0);
        match line.state {
            Sent::Ok => paint_ticks(&pa, at, p.accent),
            _ => paint_clock(&pa, at, p.weak),
        }
    }

    let hover = match (&line.body, line.mine, failed) {
        (_, _, true) => match &line.state {
            Sent::Failed(e) => format!("Não entregue: {e}"),
            _ => String::new(),
        },
        (Body::File { .. }, false, _) => "Clique para abrir · botão direito: mostrar na pasta".into(),
        (Body::File { .. }, true, _) => "Botão direito: mostrar na pasta".into(),
        (Body::Text(_), _, _) => "Botão direito: copiar".into(),
    };
    let mut resp = resp.on_hover_text(hover);
    if failed || (!line.mine && matches!(line.body, Body::File { .. })) {
        resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    if failed && resp.clicked() {
        action = Some(BubbleAction::Retry);
    }
    let spoken = match &line.body {
        Body::Text(t) => format!("{}: {t}", if line.mine { "Você" } else { "Recebida" }),
        Body::File { name, .. } => format!("Arquivo {name}"),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &spoken));
    action
}

enum Content {
    Text(Arc<egui::Galley>, Vec2),
    File(String, u64),
}

/// Arquivo que esta chegando agora (some quando termina).
fn receiving_bubble(ui: &mut egui::Ui, p: Pal, side: f32, name: &str, x: &Xfer) {
    let full = ui.available_width();
    let w = 360.0_f32.min(full - 2.0 * side);
    let (row, _) = ui.allocate_exact_size(vec2(full, 62.0), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.left() + side, row.top()), vec2(w, 62.0));
    let pa = ui.painter().clone();
    pa.rect_filled(rect, 8.0, p.bin);
    let inner = rect.shrink2(BUBBLE_PAD);
    let g = elided(&pa, &format!("Recebendo {name}"), bold(14.0), p.text, inner.width());
    pa.galley(inner.min, g, p.text);
    let g = elided(&pa, &x.line(), font(12.0), p.weak, inner.width());
    pa.galley(inner.min + vec2(0.0, 22.0), g, p.weak);
    let bar = Rect::from_min_size(pos2(inner.left(), inner.bottom() - 4.0), vec2(inner.width(), 4.0));
    paint_bar(&pa, p, bar, x);
    let resp = ui
        .interact(rect, ui.scope_id().with(("recebendo", name)), Sense::hover())
        .on_hover_text(x.speed_text());
    let spoken = format!("Recebendo {name}, {}", x.line());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, &spoken));
}

/// Barra de progresso fina, como nos mensageiros.
fn paint_bar(pa: &egui::Painter, p: Pal, bar: Rect, x: &Xfer) {
    pa.rect_filled(bar, 2.0, p.weak.gamma_multiply(0.3));
    let frac = x.done as f32 / x.total.max(1) as f32;
    pa.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), 2.0, p.accent);
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
                Event::Chat { from, fp, text } => self.incoming(from, fp, Body::Text(text), focused),
                Event::FileReceived { from, fp, path, size } => {
                    let path = PathBuf::from(path);
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("arquivo")
                        .to_string();
                    self.incoming(from, fp, Body::File { name, size, path }, focused);
                }
                // avisos tecnicos so interessam no terminal; o resto aparece na conversa
                Event::Notice { text } => eprintln!("{text}"),
                Event::Sent { id, err } => {
                    if let Some(e) = &err {
                        if e != "cancelado" {
                            self.toast(format!("Não entregue: {e}"));
                        }
                    }
                    self.finish(id, err);
                }
                Event::ApprovalRequest { id, name, code } => {
                    self.approvals_ui.push_back((id, name, code));
                    show_from_anywhere();
                }
                Event::Progress { fp, name, done, total } => {
                    let key = (fp, name);
                    if done >= total {
                        self.transfers.remove(&key);
                    } else {
                        let now = Instant::now();
                        let x = self.transfers.entry(key).or_insert(Xfer {
                            done,
                            total,
                            updated: now,
                            started: now,
                            start_done: done,
                        });
                        // o relogio da velocidade comeca quando os bytes andam (a conexao vem antes)
                        if x.done == 0 && done > 0 {
                            x.started = now;
                            x.start_done = done;
                        }
                        x.done = done;
                        x.total = total;
                        x.updated = now;
                    }
                }
                Event::ProfileFetched { fp, rev, about, photo } => {
                    let photo = photo_texture(ctx, &format!("foto-{fp}-{rev}"), &photo);
                    let e = self.profiles.entry(fp).or_default();
                    e.fetching = false;
                    // rev vazio = a busca falhou: mantem o que ja tinha e so tenta de
                    // novo em 1 min; com sucesso, a proxima versao e buscada na hora
                    if rev.is_empty() {
                        e.last_try = Some(Instant::now());
                    } else {
                        e.last_try = None;
                        e.rev = rev;
                        e.about = about;
                        e.photo = photo;
                    }
                }
                Event::ShareListed { fp, path, res } => {
                    // resposta de uma pasta que nao esta mais na tela e descartada
                    if let Dialog::Share(v) = &mut self.dialog {
                        if v.fp == fp && v.path == path {
                            v.listing = Some(res);
                        }
                    }
                }
                Event::ShareFailed(e) => self.toast(format!("Não deu para baixar: {e}")),
                Event::GuiShow => show_window(ctx),
                Event::UpdateAvailable(m) => self.update = Some(m),
                Event::UpdateFailed(e) => {
                    self.updating = false;
                    self.toast(format!("Atualização falhou: {e}"));
                }
                Event::GuiQuit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // recebimento que caiu no meio nao avisa: a barra some sozinha. Os meus
        // envios ficam (a retomada de arquivo grande passa de 10 s sem progresso)
        let mine: Vec<String> = self.sending.values().map(|(n, _)| n.clone()).collect();
        self.transfers
            .retain(|(_, n), t| mine.contains(n) || t.updated.elapsed() < Duration::from_secs(10));

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
        // X na janela = continuar rodando em segundo plano (sair: bandeja ou menu "Sair")
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
        let now = now_secs();
        for p in self.online.values() {
            self.contacts.insert(p.fp.clone(), p.name.clone());
            self.last_seen.insert(p.fp.clone(), now);
        }
        // offline e sem conversa sai da lista: anuncios falsos na LAN nao a enchem
        let (online, log, sel) = (&self.online, &self.log, &self.selected);
        self.contacts.retain(|fp, _| {
            online.contains_key(fp) || sel.as_ref() == Some(fp) || log.iter().any(|l| &l.peer == fp)
        });
        self.sync_profiles(ui.ctx());
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
        // Esc fecha a conversa (se nenhuma janela estiver aberta por cima)
        if matches!(self.dialog, Dialog::None)
            && self.approvals_ui.is_empty()
            && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.selected = None;
        }

        let ctx = ui.ctx().clone();
        self.update_banner(ui);
        self.sidebar(ui);
        self.chat(ui);
        self.approval(&ctx);
        self.dialogs(&ctx);
        self.toasts(&ctx);

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
    fn andamento_da_transferencia() {
        let now = Instant::now();
        let x = Xfer { done: 0, total: 1 << 30, updated: now, started: now, start_done: 0 };
        assert!(x.line().starts_with("preparando"), "hash antes de enviar");
        // metade em 4 s: falta mais 4 s
        let started = now - Duration::from_secs(4);
        let half = Xfer { done: 512 << 20, started, ..x };
        assert_eq!(half.line(), "50% · 512.0 MB de 1.0 GB · faltam 4 s");
        assert_eq!(half.speed_text(), "128.0 MB/s");
        assert_eq!(fmt_eta(90), "2 min");
        assert_eq!(fmt_eta(3700), "1 h 1 min");
    }

    #[test]
    fn iniciais_do_avatar() {
        assert_eq!(initials("Ana (notebook)"), "AN");
        assert_eq!(initials("desktop-abc123"), "DA");
        assert_eq!(initials("bruno"), "BR");
        assert_eq!(initials("  "), "?");
        assert_eq!(avatar_color("abc"), avatar_color("abc"));
    }

    #[test]
    fn rotulos_de_dia() {
        let now = now_secs();
        assert_eq!(day_label(now), "Hoje");
        assert_eq!(day_label(now - 86_400), "Ontem");
        assert_eq!(list_time(now).len(), 5, "hoje mostra HH:MM");
    }

    #[test]
    fn icone_tem_balao_e_selo() {
        let px = |rgba: &[u8], x: usize, y: usize| rgba[(y * 32 + x) * 4..][..4].to_vec();
        let plain = icon_rgba(32, false);
        assert_eq!(plain.len(), 32 * 32 * 4);
        assert_eq!(px(&plain, 0, 0)[3], 0, "canto transparente");
        assert_eq!(px(&plain, 6, 20), [37, 99, 235, 255], "corpo azul");
        let red = LIGHT.danger;
        assert_eq!(px(&icon_rgba(32, true), 24, 7), [red.r(), red.g(), red.b(), 255]);
    }

    #[test]
    fn historico_de_arquivo_ida_e_volta() {
        let line = Line {
            id: 0,
            t: 1,
            peer: "fp".into(),
            mine: false,
            body: Body::File { name: "a.zip".into(), size: 9, path: "/x/a.zip".into() },
            state: Sent::Ok,
        };
        let Body::File { name, size, path } = &line.body else { unreachable!() };
        let e = HEntry {
            t: line.t,
            k: "file_in".into(),
            who: "ana".into(),
            text: name.clone(),
            peer: line.peer.clone(),
            path: path.display().to_string(),
            size: *size,
        };
        let back: HEntry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!((back.k.as_str(), back.size, back.text.as_str()), ("file_in", 9, "a.zip"));
        // linha antiga (sem path/size) continua valida
        let old: HEntry = serde_json::from_str(r#"{"t":1,"k":"from","who":"a","text":"oi","peer":"f"}"#).unwrap();
        assert_eq!((old.size, old.path.as_str()), (0, ""));
    }
}
