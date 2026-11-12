//! Perfil de cada computador: nome (fica no identity.json), recado e foto.
//!
//! O perfil e publico na rede local (como a foto no WhatsApp): quem esta na rede
//! pode pedir, sem precisar ser confiado. Mensagens e arquivos continuam exigindo
//! aprovacao.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Recado ("sobre"), como no WhatsApp.
pub const MAX_ABOUT: usize = 140;
/// Lado da foto publicada (quadrada).
pub const PHOTO_SIDE: u32 = 128;
/// Teto da foto na rede: cabe num unico frame do protocolo (64 KiB).
pub const MAX_PHOTO_BYTES: usize = 60 * 1024;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub about: String,
    /// JPEG 128x128 ja normalizado; vazio = sem foto.
    #[serde(skip)]
    pub photo: Vec<u8>,
}

fn dir() -> PathBuf {
    crate::identity::config_dir()
}

/// Perfil deste computador (~/.localchat/perfil.json + foto.jpg).
pub fn load_own() -> Profile {
    let mut p: Profile = std::fs::read_to_string(dir().join("perfil.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    p.photo = std::fs::read(dir().join("foto.jpg")).unwrap_or_default();
    p
}

pub fn save_own(p: &Profile) -> Result<()> {
    std::fs::write(dir().join("perfil.json"), serde_json::to_string_pretty(p)?)?;
    let foto = dir().join("foto.jpg");
    if p.photo.is_empty() {
        let _ = std::fs::remove_file(foto);
    } else {
        std::fs::write(foto, &p.photo)?;
    }
    Ok(())
}

/// Versao do perfil anunciada na rede: muda quando nome, recado ou foto mudam.
pub fn rev(name: &str, p: &Profile) -> String {
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update([0]);
    h.update(p.about.as_bytes());
    h.update([0]);
    h.update(&p.photo);
    hex::encode(&h.finalize()[..8])
}

/// Recado valido: curto, sem quebra de linha nem caractere de controle.
pub fn clean_about(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_ABOUT)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Qualquer PNG/JPEG -> JPEG quadrado de 128x128. Vale para a foto escolhida
/// aqui e para a recebida de outro computador (dado nao confiavel): decodifica
/// com limite de tamanho e re-codifica, entao so sai daqui uma imagem nossa.
pub fn normalize_photo(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .context("formato de imagem desconhecido")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8000);
    limits.max_image_height = Some(8000);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let img = reader.decode().context("imagem invalida (use PNG ou JPEG)")?;
    let side = img.width().min(img.height());
    if side == 0 {
        bail!("imagem vazia");
    }
    // recorte quadrado no centro, como a foto redonda vai mostrar
    let x = (img.width() - side) / 2;
    let y = (img.height() - side) / 2;
    let square = img
        .crop_imm(x, y, side, side)
        .resize_exact(PHOTO_SIDE, PHOTO_SIDE, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&square)?;
    if out.len() > MAX_PHOTO_BYTES {
        bail!("foto grande demais depois de reduzida");
    }
    Ok(out)
}

/// Perfil de outro computador guardado aqui (~/.localchat/contatos/<fp>.*),
/// para a foto aparecer mesmo com ele offline.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Cached {
    pub rev: String,
    pub about: String,
}

fn cache_dir() -> PathBuf {
    dir().join("contatos")
}

pub fn load_cached(fp: &str) -> Option<(Cached, Vec<u8>)> {
    // o fp vem da rede: so hex, senao vira caminho
    if !fp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let c: Cached = serde_json::from_str(&std::fs::read_to_string(cache_dir().join(format!("{fp}.json"))).ok()?).ok()?;
    let photo = std::fs::read(cache_dir().join(format!("{fp}.jpg"))).unwrap_or_default();
    Some((c, photo))
}

pub fn save_cached(fp: &str, c: &Cached, photo: &[u8]) -> Result<()> {
    if !fp.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("fingerprint invalido");
    }
    std::fs::create_dir_all(cache_dir())?;
    std::fs::write(cache_dir().join(format!("{fp}.json")), serde_json::to_string(c)?)?;
    let jpg = cache_dir().join(format!("{fp}.jpg"));
    if photo.is_empty() {
        let _ = std::fs::remove_file(jpg);
    } else {
        std::fs::write(jpg, photo)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn foto_vira_jpeg_quadrado_pequeno() {
        let jpg = normalize_photo(&png(640, 400)).unwrap();
        assert!(jpg.starts_with(&[0xFF, 0xD8]), "jpeg");
        assert!(jpg.len() <= MAX_PHOTO_BYTES);
        let back = image::load_from_memory(&jpg).unwrap();
        assert_eq!((back.width(), back.height()), (PHOTO_SIDE, PHOTO_SIDE));
        // normalizar de novo (como faz quem recebe) continua valido
        assert!(normalize_photo(&jpg).is_ok());
    }

    #[test]
    fn foto_invalida_ou_gigante_e_recusada() {
        assert!(normalize_photo(b"nao sou imagem").is_err());
        assert!(normalize_photo(&png(9000, 10)).is_err(), "passa do limite de largura");
    }

    #[test]
    fn recado_e_versao() {
        assert_eq!(clean_about("  oi\ntudo bem?  "), "oi tudo bem?");
        assert_eq!(clean_about(&"x".repeat(500)).chars().count(), MAX_ABOUT);
        let p = Profile { about: "a".into(), photo: vec![] };
        let q = Profile { about: "b".into(), photo: vec![] };
        assert_ne!(rev("ana", &p), rev("ana", &q));
        assert_ne!(rev("ana", &p), rev("bia", &p));
        assert_eq!(rev("ana", &p), rev("ana", &p.clone()));
        assert!(load_cached("../../etc/passwd").is_none());
    }
}
