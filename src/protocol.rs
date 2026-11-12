use serde::{Deserialize, Serialize};

/// v2: o destino confirma cada mensagem de chat com `ChatAck`.
pub const PROTO_VERSION: u32 = 2;
/// Porta UDP de descoberta (broadcast na LAN).
pub const DISCOVERY_PORT: u16 = 47820;
/// Limite de mensagem do Noise (65535 bytes de plaintext).
pub const MAX_FRAME: usize = 65535;
/// Tamanho de cada pedaco de arquivo.
pub const CHUNK_SIZE: usize = 32 * 1024;

pub const KIND_CONTROL: u8 = 0;
pub const KIND_CHUNK: u8 = 1;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Control {
    Hello {
        name: String,
        fingerprint: String,
        version: u32,
    },
    Chat {
        text: String,
    },
    /// O destino recebeu (e mostrou) a mensagem de chat.
    ChatAck,
    FileOffer {
        file_id: String,
        name: String,
        size: u64,
    },
    FileAccept {
        file_id: String,
        /// Retomada: a partir de qual byte o destino quer receber (0 = inicio).
        offset: u64,
    },
    FileReject {
        file_id: String,
        reason: String,
    },
    /// Fim do stream de chunks, com hash para verificacao.
    FileDone {
        file_id: String,
        sha256: String,
    },
    /// Confirmacao final de quem recebeu.
    FileAck {
        file_id: String,
        ok: bool,
        note: String,
    },
    Bye {
        reason: String,
    },
}

/// Beacon UDP de descobertura na LAN.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Beacon {
    pub app: String,
    pub v: u32,
    /// "announce" | "query" | "answer" | "show" (so de 127.0.0.1: mostra a janela)
    pub kind: String,
    pub name: String,
    /// Porta TCP do app.
    pub port: u16,
    /// Fingerprint da chave estatica (hex).
    pub fp: String,
}
