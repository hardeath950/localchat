use crate::protocol::{Control, KIND_CHUNK, KIND_CONTROL, MAX_FRAME};
use anyhow::{bail, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_CIPHER: usize = MAX_FRAME + 16;

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    if data.len() > MAX_CIPHER {
        bail!("frame grande demais: {}", data.len());
    }
    // tamanho + dados numa escrita so: com TCP_NODELAY, duas escritas viram
    // duas chamadas ao sistema e dois pacotes por frame
    let mut frame = Vec::with_capacity(4 + data.len());
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(data);
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

/// Le um frame. `None` = o par fechou a conexao limpo (entre frames).
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut lenb = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = r.read(&mut lenb[got..]).await.context("falha de leitura")?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            bail!("conexao truncada");
        }
        got += n;
    }
    let n = u32::from_be_bytes(lenb) as usize;
    if n == 0 || n > MAX_CIPHER {
        bail!("frame invalido: {n}");
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await.context("conexao truncada")?;
    Ok(Some(buf))
}

pub enum Msg {
    Control(Control),
    Chunk { id: Vec<u8>, seq: u32, data: Vec<u8> },
}

/// Canal criptografado: handshake Noise XX + mensagens autenticadas (ChaCha20-Poly1305).
pub struct SecureChannel<S> {
    stream: S,
    noise: snow::TransportState,
    remote_static: Vec<u8>,
}

async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    privkey: &[u8],
    initiator: bool,
) -> Result<(snow::TransportState, Vec<u8>)> {
    let params: snow::params::NoiseParams = crate::identity::NOISE_PARAMS.parse()?;
    let builder = snow::Builder::new(params);
    let mut hs = if initiator {
        builder.local_private_key(privkey).build_initiator()?
    } else {
        builder.local_private_key(privkey).build_responder()?
    };
    let mut buf = vec![0u8; MAX_FRAME];
    if initiator {
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
        let f = read_frame(stream).await?.context("handshake incompleto")?;
        hs.read_message(&f, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
    } else {
        let f = read_frame(stream).await?.context("handshake incompleto")?;
        hs.read_message(&f, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
        let f = read_frame(stream).await?.context("handshake incompleto")?;
        hs.read_message(&f, &mut buf)?;
    }
    let ts = hs.into_transport_mode()?;
    let remote = ts.get_remote_static().map(|s| s.to_vec()).unwrap_or_default();
    Ok((ts, remote))
}

impl<S: AsyncRead + AsyncWrite + Unpin> SecureChannel<S> {
    pub async fn initiator(stream: S, privkey: &[u8]) -> Result<Self> {
        let mut stream = stream;
        let (noise, remote_static) = handshake(&mut stream, privkey, true).await?;
        Ok(Self { stream, noise, remote_static })
    }

    pub async fn responder(stream: S, privkey: &[u8]) -> Result<Self> {
        let mut stream = stream;
        let (noise, remote_static) = handshake(&mut stream, privkey, false).await?;
        Ok(Self { stream, noise, remote_static })
    }

    /// Fingerprint REAL da chave estatica do par (a unica que importa).
    pub fn remote_fingerprint(&self) -> String {
        crate::identity::fingerprint(&self.remote_static)
    }

    pub async fn send_control(&mut self, c: &Control) -> Result<()> {
        let payload = serde_json::to_vec(c)?;
        let mut plain = Vec::with_capacity(payload.len() + 1);
        plain.push(KIND_CONTROL);
        plain.extend_from_slice(&payload);
        self.send_plain(&plain).await
    }

    pub async fn send_chunk(&mut self, id16: &[u8], seq: u32, data: &[u8]) -> Result<()> {
        let mut plain = Vec::with_capacity(data.len() + 21);
        plain.push(KIND_CHUNK);
        plain.extend_from_slice(id16);
        plain.extend_from_slice(&seq.to_be_bytes());
        plain.extend_from_slice(data);
        self.send_plain(&plain).await
    }

    async fn send_plain(&mut self, plain: &[u8]) -> Result<()> {
        let mut cipher = vec![0u8; plain.len() + 16];
        let n = self.noise.write_message(plain, &mut cipher)?;
        write_frame(&mut self.stream, &cipher[..n]).await
    }

    pub async fn recv(&mut self) -> Result<Msg> {
        self.recv_opt()
            .await?
            .ok_or_else(|| anyhow::anyhow!("conexao encerrada pelo par"))
    }

    /// `None` = par fechou a conexao de forma limpa.
    pub async fn recv_opt(&mut self) -> Result<Option<Msg>> {
        let Some(frame) = read_frame(&mut self.stream).await? else {
            return Ok(None);
        };
        let mut plain = vec![0u8; frame.len()];
        let n = self.noise.read_message(&frame, &mut plain)?;
        Ok(Some(decode(&plain[..n])?))
    }

    /// Fecha a conexao de forma graciosa (FIN apos todo o trafego).
    pub async fn shutdown(&mut self) -> Result<()> {
        self.stream.shutdown().await?;
        Ok(())
    }
}

fn decode(plain: &[u8]) -> Result<Msg> {
    if plain.is_empty() {
        bail!("mensagem vazia");
    }
    match plain[0] {
        KIND_CONTROL => Ok(Msg::Control(serde_json::from_slice(&plain[1..])?)),
        KIND_CHUNK => {
            if plain.len() < 21 {
                bail!("chunk curto demais");
            }
            let id = plain[1..17].to_vec();
            let seq = u32::from_be_bytes(plain[17..21].try_into()?);
            Ok(Msg::Chunk { id, seq, data: plain[21..].to_vec() })
        }
        k => bail!("tipo de mensagem desconhecido: {k}"),
    }
}
