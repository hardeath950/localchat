# LocalChat — desenvolvimento

Para usar o app, veja o [README](README.md). Aqui fica como ele funciona, como compilar
e como publicar versões.

## Como funciona

- **Descoberta**: cada nó anuncia a cada 3 s via broadcast UDP (porta 47820) e responde
  a consultas. IPs digitados em "Conectar" recebem anúncio e consulta diretos
  (`~/.localchat/manual_peers.txt`). Quem some por 10 s sai da lista.
- **Conexão**: TCP (porta 47821) com handshake **Noise XX**
  (X25519 + ChaCha20-Poly1305 + BLAKE2s). Todo tráfego é autenticado e criptografado.
- **Identidade**: chave X25519 por máquina (`~/.localchat/identity.json`). O fingerprint
  (SHA-256 da chave pública) identifica o par; o nome padrão é o do computador.
- **Confiança**: primeiro contato pede "Confiar / Recusar" na janela, com código de
  pareamento de 6 dígitos; mesmo nome com outra chave é recusado.
- **Mensagens**: o destino confirma cada uma (`ChatAck`, protocolo 2); só então a GUI
  mostra "entregue". Versões de protocolo diferentes não conversam (a GUI avisa).
- **Arquivos**: chunks de 32 KiB, SHA-256 do arquivo inteiro conferido no destino;
  transferência interrompida é retomada do `.part`. O hash é calculado durante o envio
  (na retomada, os dois lados leem primeiro o trecho já transferido).
- **Pasta compartilhada**: cada pasta vale para um fingerprint, até um prazo
  (`~/.localchat/shares.json`). O par pede `ShareList { path }` (resposta `ShareListing`
  + JSON em chunks) ou `ShareGet { path }` (resposta: `FileOffer` normal). Só depois da
  confiança; o caminho é canonicalizado e precisa ficar dentro da pasta (`src/share.rs`),
  e links não aparecem na listagem. Erros voltam como `Bye { reason }`.
- **Perfil** (nome, recado, foto): o anúncio UDP leva `rev`, a versão do perfil. Quem vê
  um `rev` novo conecta e manda `ProfileRequest`; a resposta é `Profile { about, photo_len }`
  seguido de um `Chunk` com a foto (JPEG 128×128). É público: não grava confiança de
  nenhum lado nem pede aprovação. Versões sem `rev` (≤ 0.2.1) nunca recebem o pedido. A foto
  recebida é decodificada com limites e re-codificada (`src/profile.rs`); cache em
  `~/.localchat/contatos/`.
- **Histórico** (GUI): `~/.localchat/history.jsonl`, últimas 1000 linhas.

## Compilar

```bash
# Windows (Rust + MSYS2 com mingw-w64-x86_64-gcc em C:\msys64 + Inno Setup 6):
# binário, testes e dist\LocalChat-Setup.exe
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1

# Linux (instala dependências com apt e o Rust se faltar)
./scripts/build-linux.sh
```

No Windows o build usa o toolchain `stable-x86_64-pc-windows-gnu` (sem Visual Studio);
o script escolhe por `RUSTUP_TOOLCHAIN`. Para rodar `cargo` direto, exporte essa variável
e ponha `C:\msys64\mingw64\bin` no PATH.

O binário Linux publicado é compilado no Ubuntu 20.04 (glibc 2.31): roda no Zorin 16/17
e Ubuntu 20.04+.

## Linha de comando

```bash
localchat gui                          # a janela (o que o atalho abre)
localchat init --name notebook-sala    # cria/mostra a identidade e o fingerprint
localchat serve                        # nó sem janela: recebe em ~/Downloads/LocalChat
localchat peers                        # quem está na rede
localchat msg 192.168.0.10 "cheguei"
localchat send 192.168.0.10 ./fotos.zip
localchat trust --list                 # pares confiados
localchat trust --forget <fingerprint> # esquecer (ex.: reinstalou o outro PC)
```

`LOCALCHAT_HOME` troca a pasta de estado (útil para testar várias identidades numa máquina).

## Publicar uma versão

1. Commits no formato `feat: …` (novidade) e `fix: …` (correção) — eles viram o CHANGELOG.
2. A cada push na `main`, o [release-plz](https://release-plz.dev) abre/atualiza um PR
   "chore: release vX.Y.Z" com a versão nova e o CHANGELOG. Revise e faça merge.
3. O merge cria a tag e o GitHub Release; o workflow `release.yml` compila Windows e
   Linux, gera `SHA256SUMS` e `latest.json`, assina os dois com minisign e anexa tudo.
   Os apps instalados acham a versão nova sozinhos.

Para refazer os arquivos de uma tag: Actions → Release → Run workflow (informe a tag).

A chave privada de assinatura fica no secret `MINISIGN_SECRET_KEY` (cópia fora do repo
em `~/.localchat-release/` de quem a gerou). Sem ela não há como publicar update; se
vazar, quem a tiver consegue publicar update malicioso.

## Atualização automática

O app baixa `releases/latest/download/latest.json` e `latest.json.minisig`, confere a
assinatura com a chave pública embutida (`src/update.rs`), compara a versão (nunca volta
versão) e, no clique, baixa o arquivo da plataforma e confere o SHA-256 do manifesto.
Windows: roda o instalador em modo silencioso (pede administrador) e ele reabre o app.
Linux: troca o binário no lugar (precisa estar numa pasta do usuário, como faz o
`install-linux.sh`) e reabre.

Para testar com um servidor local: `LOCALCHAT_UPDATE_URL=http://127.0.0.1:8765` (a
assinatura continua obrigatória).

Conferir um download manualmente:
`minisign -Vm SHA256SUMS -P RWT6nq8IoZiQV4sY38od1xqByP+SmV25dMPjk8QaYNWGrtILydUBXyvG`

## Segurança

- Noise XX por sessão; nada em claro na rede além do anúncio (nome, porta, fingerprint).
- **Código de pareamento de 6 dígitos**: derivado dos dois fingerprints. Contra alguém
  passivo na rede, códigos iguais garantem que não há ninguém no meio.
  **Limite conhecido:** um atacante ativo na mesma rede, no momento do primeiro
  pareamento, consegue fabricar uma chave que gera o mesmo código nos dois lados (leva
  segundos, porque os fingerprints são anunciados). Em rede de casa o risco é baixo; em
  rede pública ou de escritório, compare o fingerprint completo (`localchat init`).
- Tempo limite no handshake (10 s), teto de 32 conexões e de 3 pedidos de aprovação
  simultâneos; anúncios falsos não enchem a lista; nomes de arquivo perigosos são tratados.
- Pasta `~/.localchat` só legível pelo dono no Linux.
- Arquivos de pares confiados são aceitos automaticamente (`serve --no-auto-accept` recusa).
- CI roda `cargo audit` (RustSec) a cada push.

## Roadmap

- Código de pareamento resistente a ataque ativo (commit/SAS ou PAKE).
- Assinatura do instalador Windows (tira o aviso do SmartScreen).
- Várias transferências em paralelo, com progresso por arquivo.
- Pacote .deb.
