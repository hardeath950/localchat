use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Identity {
    pub name: String,
    pub private_hex: String,
    pub public_hex: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct TrustStore {
    pub peers: HashMap<String, TrustEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrustEntry {
    pub name: String,
    pub last_seen: u64,
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LOCALCHAT_HOME") {
        return PathBuf::from(d);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".localchat")
}

pub fn fingerprint(public: &[u8]) -> String {
    hex::encode(Sha256::digest(public))
}

/// Codigo de pareamento de 6 digitos, derivado dos DOIS fingerprints.
/// Como e simetrico, os dois lados mostram o mesmo numero — se um invasor
/// estiver no meio, os fingerprints nao batem e o codigo aparece diferente.
pub fn pairing_code(fp_a: &str, fp_b: &str) -> String {
    let (lo, hi) = if fp_a <= fp_b { (fp_a, fp_b) } else { (fp_b, fp_a) };
    let mut h = Sha256::new();
    h.update(b"localchat-pair");
    h.update(lo.as_bytes());
    h.update(hi.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Carrega a identidade local ou cria uma nova (chave X25519).
pub fn load_or_init(name: Option<String>) -> Result<(Identity, Vec<u8>, Vec<u8>)> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("criar {}", dir.display()))?;
    // a chave privada e o historico ficam aqui: so o dono le (vale para instalacoes antigas)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("identity.json");
    if path.exists() {
        let id: Identity = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let private = hex::decode(&id.private_hex).context("identity.json corrompido")?;
        let public = hex::decode(&id.public_hex).context("identity.json corrompido")?;
        return Ok((id, private, public));
    }
    let params: snow::params::NoiseParams = NOISE_PARAMS.parse()?;
    let kp = snow::Builder::new(params).generate_keypair()?;
    let name = name.unwrap_or_else(default_name);
    let id = Identity {
        name: name.clone(),
        private_hex: hex::encode(&kp.private),
        public_hex: hex::encode(&kp.public),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&id)?)?;
    println!("Identidade '{name}' criada em {}", path.display());
    Ok((id, kp.private, kp.public))
}

/// Nome da maquina, nao do usuario: o mesmo usuario em dois PCs daria nomes iguais.
fn default_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "eu".into())
}

/// Troca o nome mostrado aos outros (a chave, e portanto a confianca, nao muda).
pub fn rename(new_name: &str) -> Result<()> {
    let path = config_dir().join("identity.json");
    let mut id: Identity = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    id.name = new_name.to_string();
    std::fs::write(&path, serde_json::to_string_pretty(&id)?)?;
    Ok(())
}

pub fn trust_path() -> PathBuf {
    config_dir().join("known_peers.json")
}

pub fn load_trust() -> TrustStore {
    std::fs::read_to_string(trust_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_trust(store: &TrustStore) -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(trust_path(), serde_json::to_string_pretty(store)?)?;
    Ok(())
}

/// Consulta sem gravar: Ok(true) = primeira vez com esta chave (par novo).
/// Err = nome ja existe com outra chave (possivel sequestro).
pub fn trust_status(fp: &str, name: &str) -> std::result::Result<bool, String> {
    let store = load_trust();
    for (k, e) in store.peers.iter() {
        if e.name.eq_ignore_ascii_case(name) && k != fp {
            return Err(format!(
                "seguranca: o nome '{name}' ja pertence a outra chave (fingerprint {k}). \
                 Se for reinstalacao do app na maquina certa, rode: localchat trust --forget {k}"
            ));
        }
    }
    Ok(!store.peers.contains_key(fp))
}

/// Grava (ou atualiza) o par como confiavel.
pub fn trust_save(fp: &str, name: &str) {
    let mut store = load_trust();
    store.peers.insert(
        fp.to_string(),
        TrustEntry {
            name: name.to_string(),
            last_seen: now(),
        },
    );
    let _ = save_trust(&store);
}

/// TOFU direto (CLI/envio iniciado pelo usuario): consulta e grava de uma vez.
pub fn trust_check(fp: &str, name: &str) -> std::result::Result<bool, String> {
    let is_new = trust_status(fp, name)?;
    trust_save(fp, name);
    Ok(is_new)
}

pub fn trust_forget(query: &str) -> usize {
    let mut store = load_trust();
    let before = store.peers.len();
    store
        .peers
        .retain(|fp, e| !(fp.starts_with(query) || e.name.eq_ignore_ascii_case(query)));
    let removed = before - store.peers.len();
    if removed > 0 {
        let _ = save_trust(&store);
    }
    removed
}
