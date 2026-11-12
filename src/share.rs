//! Pastas compartilhadas por tempo determinado: uma por contato, so leitura.
use crate::identity::now;
use crate::protocol::ShareEntry;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Share {
    pub path: PathBuf,
    /// Segundos desde 1970; depois disso todo pedido e recusado.
    pub expires: u64,
}

impl Share {
    /// Nome da pasta para mostrar na tela (uma unidade inteira nao tem nome: usa o caminho).
    pub fn name(&self) -> String {
        match self.path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => self.path.display().to_string(),
        }
    }
}

fn file() -> PathBuf {
    crate::identity::config_dir().join("shares.json")
}

/// Fingerprint do contato -> pasta. Lido do disco a cada pedido (como o known_peers).
fn load() -> HashMap<String, Share> {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Compartilha `share` com o contato `fp` (troca a pasta anterior); `None` para de compartilhar.
pub fn set(fp: &str, share: Option<Share>) -> Result<()> {
    let mut all = load();
    let now = now();
    all.retain(|_, s| s.expires > now);
    match share {
        Some(s) => all.insert(fp.to_string(), s),
        None => all.remove(fp),
    };
    std::fs::create_dir_all(crate::identity::config_dir())?;
    std::fs::write(file(), serde_json::to_string_pretty(&all)?)?;
    Ok(())
}

/// O compartilhamento ainda valido para o contato `fp`.
pub fn active(fp: &str) -> Option<Share> {
    load().remove(fp).filter(|s| s.expires > now())
}

/// Caminho real de `rel` ("a/b/c"; "" = raiz) dentro de `root`. Recusa tudo que
/// saia da pasta: "..", caminho absoluto e link/atalho que aponte para fora.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        let mut c = Path::new(part).components();
        if !matches!((c.next(), c.next()), (Some(Component::Normal(_)), None)) {
            bail!("caminho inválido");
        }
        p.push(part);
    }
    // o canonicalize segue os links: e ele que garante que o destino esta dentro
    let root = root.canonicalize().context("a pasta compartilhada não existe mais")?;
    let real = p.canonicalize().context("não encontrado")?;
    if !real.starts_with(&root) {
        bail!("caminho inválido");
    }
    Ok(real)
}

/// Arquivos e subpastas de `dir`, pastas primeiro. Links e atalhos ficam de fora.
pub fn list(dir: &Path) -> Result<Vec<ShareEntry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).context("não consegui abrir a pasta")?.flatten() {
        // metadata do DirEntry nao segue link: link nao e arquivo nem pasta aqui
        let (Ok(meta), Ok(name)) = (e.metadata(), e.file_name().into_string()) else {
            continue;
        };
        if meta.is_dir() || meta.is_file() {
            out.push(ShareEntry {
                name,
                size: if meta.is_file() { meta.len() } else { 0 },
                dir: meta.is_dir(),
            });
        }
    }
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nao_sai_da_pasta() {
        let tmp = std::env::temp_dir().join(format!("localchat-share-{}", std::process::id()));
        let root = tmp.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), "b").unwrap();
        std::fs::write(root.join("a.txt"), "aa").unwrap();
        std::fs::write(tmp.join("segredo.txt"), "x").unwrap();

        assert!(resolve(&root, "").is_ok());
        assert!(resolve(&root, "sub/b.txt").unwrap().ends_with("b.txt"));
        for bad in ["..", "../segredo.txt", "sub/../../segredo.txt", "./a.txt", "nao-existe"] {
            assert!(resolve(&root, bad).is_err(), "{bad}");
        }
        if cfg!(windows) {
            for bad in [r"..\segredo.txt", r"C:\Windows\win.ini", "C:", "NUL"] {
                assert!(resolve(&root, bad).is_err(), "{bad}");
            }
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.join("segredo.txt"), root.join("link")).unwrap();
            assert!(resolve(&root, "link").is_err());
        }

        let names: Vec<_> = list(&root).unwrap().into_iter().map(|e| (e.name, e.size, e.dir)).collect();
        assert_eq!(names, [("sub".to_string(), 0, true), ("a.txt".to_string(), 2, false)]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
