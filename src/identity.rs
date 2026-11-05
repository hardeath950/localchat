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

fn default_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "eu".into())
        .to_lowercase()
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

/// TOFU: aceita a chave do par na primeira vez que aparece.
/// Recusa se o nome ja existia gravado com outra chave (possivel sequestro de nome).
pub fn trust_check(fp: &str, name: &str) -> std::result::Result<(), String> {
    let mut store = load_trust();
    for (k, e) in store.peers.iter() {
        if e.name.eq_ignore_ascii_case(name) && k != fp {
            return Err(format!(
                "seguranca: o nome '{name}' ja pertence a outra chave (fingerprint {k}). \
                 Se for reinstalacao do app na maquina certa, rode: localchat trust --forget {k}"
            ));
        }
    }
    store.peers.insert(
        fp.to_string(),
        TrustEntry {
            name: name.to_string(),
            last_seen: now(),
        },
    );
    let _ = save_trust(&store);
    Ok(())
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
