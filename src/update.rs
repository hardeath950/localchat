//! Update automatico pelo GitHub Releases.
//!
//! Cada release publica `latest.json` (versao, notas, URL e SHA-256 de cada
//! plataforma) e `latest.json.minisig`, assinado no CI com a chave privada do
//! projeto. O app so confia no manifesto se a assinatura bater com a chave
//! publica abaixo, e so instala o arquivo cujo SHA-256 bate com o manifesto:
//! quem controlar o download (ou a rede) nao consegue empurrar um binario.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// Chave publica minisign do projeto (a privada fica so nos secrets do CI).
const PUBKEY: &str = "RWT6nq8IoZiQV4sY38od1xqByP+SmV25dMPjk8QaYNWGrtILydUBXyvG";
const DEFAULT_URL: &str = "https://github.com/hardeath950/localchat/releases/latest/download";
/// Teto de download: o binario tem ~35 MB; mais que isso e erro ou ataque.
const MAX_BYTES: u64 = 200 << 20;

#[derive(Deserialize, Debug, Clone)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub windows: Asset,
    pub linux: Asset,
}

impl Manifest {
    fn asset(&self) -> &Asset {
        if cfg!(windows) {
            &self.windows
        } else {
            &self.linux
        }
    }
}

/// `LOCALCHAT_UPDATE_URL` aponta para outro lugar (teste local); a assinatura
/// continua obrigatoria, entao isso nao abre brecha.
fn base_url() -> String {
    std::env::var("LOCALCHAT_UPDATE_URL").unwrap_or_else(|_| DEFAULT_URL.into())
}

fn get(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("localchat/", env!("CARGO_PKG_VERSION")))
        .build();
    let resp = agent.get(url).call().with_context(|| format!("baixar {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("baixar {url}"))?;
    if buf.len() as u64 > MAX_BYTES {
        bail!("download grande demais: {url}");
    }
    Ok(buf)
}

fn verify(body: &[u8], sig: &[u8]) -> Result<()> {
    let pk = minisign_verify::PublicKey::from_base64(PUBKEY).context("chave publica invalida")?;
    let sig = minisign_verify::Signature::decode(std::str::from_utf8(sig)?)
        .context("assinatura do manifesto ilegivel")?;
    pk.verify(body, &sig, false)
        .context("assinatura do manifesto NAO confere: update ignorado")?;
    Ok(())
}

/// "1.2.3" > "1.2.0"? Versao que nao parseia nunca e considerada nova.
fn newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.parse().ok());
        Some((it.next()??, it.next()??, it.next()??))
    }
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

/// Busca o manifesto assinado. `Some` = existe versao mais nova que esta.
pub fn check() -> Result<Option<Manifest>> {
    let base = base_url();
    let body = get(&format!("{base}/latest.json"))?;
    let sig = get(&format!("{base}/latest.json.minisig"))?;
    verify(&body, &sig)?;
    let m: Manifest = serde_json::from_slice(&body).context("manifesto invalido")?;
    Ok(newer(&m.version, env!("CARGO_PKG_VERSION")).then_some(m))
}

/// Baixa, confere o SHA-256 e dispara a instalacao. Quem chama encerra o app
/// em seguida: o instalador (Windows) ou o relancamento (Linux) reabre a nova.
pub fn install(m: &Manifest) -> Result<()> {
    let asset = m.asset();
    let data = get(&asset.url)?;
    let got = hex::encode(Sha256::digest(&data));
    if !got.eq_ignore_ascii_case(asset.sha256.trim()) {
        bail!("arquivo baixado não confere (SHA-256 diferente): update cancelado");
    }
    install_bytes(&m.version, &data)
}

#[cfg(windows)]
fn install_bytes(version: &str, data: &[u8]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let path: PathBuf = std::env::temp_dir().join(format!("LocalChat-Setup-{version}.exe"));
    std::fs::write(&path, data).with_context(|| format!("gravar {}", path.display()))?;
    // `start` passa pelo ShellExecute, que mostra o pedido de administrador (UAC);
    // um CreateProcess direto falharia com "elevacao necessaria"
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("abrir o instalador")?;
    Ok(())
}

#[cfg(not(windows))]
fn install_bytes(_version: &str, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let exe: PathBuf = std::env::current_exe()?;
    let new = exe.with_extension("new");
    std::fs::write(&new, data).with_context(|| {
        format!(
            "sem permissão para atualizar {} — baixe a versão nova manualmente",
            exe.display()
        )
    })?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    // rename por cima do executavel em uso e permitido no Linux (o processo
    // atual segue com o inode antigo)
    std::fs::rename(&new, &exe)?;
    // espera esta instancia soltar a porta antes de abrir a nova
    std::process::Command::new("sh")
        .args(["-c", "sleep 2; exec \"$0\" gui"])
        .arg(&exe)
        .spawn()
        .context("reabrir o LocalChat")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compara_versoes() {
        assert!(newer("0.3.0", "0.2.0"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(newer("0.2.10", "0.2.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"), "nunca volta versao (downgrade)");
        assert!(!newer("lixo", "0.2.0"));
    }

    // assinado com a chave do projeto: `minisign -S -s minisign.key -m t` (conteudo "teste\n")
    const SIG_TESTE: &str = include_str!("../tests/fixtures/teste.minisig");

    #[test]
    fn assinatura_confere_e_rejeita_adulteracao() {
        verify(b"teste\n", SIG_TESTE.as_bytes()).unwrap();
        assert!(verify(b"teste adulterado\n", SIG_TESTE.as_bytes()).is_err());
        assert!(verify(b"teste\n", b"lixo").is_err());
    }
}
