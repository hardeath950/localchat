# localchat

Chat e transferencia de arquivos entre os computadores da sua casa, direto na LAN —
sem servidor, sem nuvem, tudo criptografado de ponta a ponta.

## Como funciona

- **Descoberta**: cada no anuncia a cada 3s via broadcast UDP (porta 47820) e responde
  a consultas. `localchat peers` mostra quem esta online.
- **Conexao**: TCP (porta 47821 por padrao) com handshake **Noise XX**
  (X25519 + ChaCha20-Poly1305 + BLAKE2s). Todo trafego e autenticado e criptografado.
- **Identidade**: cada maquina gera uma chave X25519 propria (`~/.localchat/identity.json`).
  O fingerprint (SHA-256 da chave publica) identifica o par.
- **Confianca**: primeiro contato exige aprovacao ("Confiar / Recusar") na janela,
  com codigo de pareamento para conferir nos dois aparelhos; se depois o mesmo
  nome aparecer com outra chave, a conexao e recusada (possivel sequestro de nome).
- **Arquivos**: chunks de 32 KiB com SHA-256 do arquivo completo conferido no destino;
  transferencias interrompidas sao retomadas automaticamente (arquivos `.part`) e o
  progresso aparece na barra de status da GUI.

## Baixar

Versoes prontas ficam em **Releases** no GitHub: `LocalChat-Setup-<versao>.exe` (Windows)
e `localchat-linux-x86_64` (Linux; roda no Zorin 16/17 e Ubuntu 20.04+). Para conferir o
download: `SHA256SUMS` (assinado em `SHA256SUMS.minisig`) e
`minisign -Vm SHA256SUMS -P RWT6nq8IoZiQV4sY38od1xqByP+SmV25dMPjk8QaYNWGrtILydUBXyvG`.

## Atualizacoes

O app procura versao nova ao abrir e a cada 6 horas. Quando acha, aparece uma faixa
"Nova versao X disponivel — Atualizar agora": ele baixa, confere a assinatura e reabre
na versao nova (no Windows o instalador pede permissao de administrador).

O app so aceita um `latest.json` assinado com a chave do projeto e so instala o
arquivo cujo SHA-256 bate com esse manifesto; versao mais velha nunca e instalada.
No Linux o binario precisa estar numa pasta onde voce pode escrever (ex.: `~/Apps`).

## Publicar uma versao

1. Commits no formato `feat: ...` (novidade), `fix: ...` (correcao) — eles viram o CHANGELOG.
2. A cada push na `main`, o release-plz abre/atualiza um PR "release" com a versao nova
   e o CHANGELOG. Revise e faca merge.
3. O merge cria a tag e o Release; o workflow compila Windows e Linux, gera
   `SHA256SUMS` e `latest.json` assinados e anexa tudo. Os apps instalados acham sozinhos.

A chave privada de assinatura fica no secret `MINISIGN_SECRET_KEY` do repositorio (e uma
copia fora do repo, em `~/.localchat-release/`). Sem ela nao ha como publicar update; se
ela vazar, quem a tiver consegue publicar update malicioso — guarde com cuidado.

## Compilar

```bash
# Windows (Rust + MSYS2 com mingw-w64-x86_64-gcc + Inno Setup 6): binario e instalador
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
# Linux
./scripts/build-linux.sh
```

## Linux (Zorin OS, Ubuntu e derivados)

No computador Linux, baixe/clone este projeto e rode:

```bash
./scripts/build-linux.sh
./target/x86_64-unknown-linux-gnu/release/localchat gui
```

O script instala as dependencias (GTK), o Rust e compila. No Linux o X da janela
minimiza (o app continua recebendo) e "Sair" encerra. O protocolo
e o mesmo do Windows — **Windows e Linux conversam entre si na mesma rede** (a
descoberta e o pareamento funcionam entre os dois).

Para so copiar o binario pronto (`dist/localchat-linux-x86_64`) sem compilar:
`chmod +x localchat-linux-x86_64 && ./localchat-linux-x86_64 gui`. Se ele fechar na hora
reclamando de `libxkbcommon-x11.so`, instale: `sudo apt install libxkbcommon-x11-0`.

Se usar firewall (ufw): `sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp`.
Para lancar pelo menu de aplicativos, crie `~/.local/share/applications/localchat.desktop`
apontando para o binario com o argumento `gui`.

## Instalador (Windows)

`scripts\build-windows.ps1` gera `dist\LocalChat-Setup-<versao>.exe`. Rode o exe nas maquinas: instala em `C:\Program Files\LocalChat`, cria atalho
no menu iniciar (e opcionalmente na area de trabalho), libera o firewall e oferece
iniciar com o Windows. Desinstalar pelo Painel de Controle remove tudo (inclusive a
regra do firewall).

## Uso

### Interface grafica (recomendado)

```bash
localchat gui
```

Abre a janela do LocalChat:

- **Conversas** (esquerda): cada computador tem a sua conversa. Bolinha verde = online,
  cinza = offline; o numero vermelho conta as mensagens nao lidas (tambem aparecem no
  titulo da janela e no icone da bandeja).
- **Mensagem**: digite e pressione Enter. Cada mensagem mostra a hora e "entregue" so
  depois que o outro computador confirmou; se falhar, clique em "tentar de novo".
- **Arquivos**: botao "Arquivo..." ou arraste o arquivo para a janela. A barra de
  progresso fica embaixo, com "Cancelar"; "Abrir pasta" abre os recebidos.
- **Seu nome**: clique em "Voce: ..." para mudar o nome que os outros veem (o padrao e
  o nome do computador).
- **Nao achou o outro computador?** Digite o IP dele em "Conectar" (o IP deste
  computador aparece logo abaixo). Util quando o broadcast nao passa (VPN, varias
  placas de rede); o IP fica salvo em `~/.localchat/manual_peers.txt`.

Fechar com o X deixa o app rodando e recebendo: no Windows vai para a bandeja (clique
no icone para reabrir), no Linux minimiza. "Sair" (na janela ou no menu da bandeja)
encerra. Abrir o LocalChat de novo enquanto ele ja roda so traz a janela de volta.

### Linha de comando

```bash
# 1) na primeira vez, cria a identidade (uma vez por maquina)
localchat init --name notebook-sala

# 2) roda o no (fica ouvindo e recebendo)
localchat serve                      # aceita em 0.0.0.0:47821, salva em ~/Downloads/LocalChat
localchat serve --listen 0.0.0.0:47821 --download-dir D:/recebidos

# 3) em outra maquina, ver quem esta online
localchat peers

# envio direto (sem abrir o serve interativo)
localchat msg 192.168.0.10 "cheguei"
localchat send 192.168.0.10 ./fotos.zip

# dentro do serve interativo
/peers
/msg notebook-sala ola
/send notebook-sala C:/Users/voce/Documents/planilha.xlsx
/quit
```

Segredos/estado ficam em `~/.localchat` (ou `LOCALCHAT_HOME` para testar com varias identidades).

## Windows Firewall

Na primeira execucao o Windows vai pedir liberacao — marque **Redes privadas**.
A descoberta usa UDP 47820 e o trafego TCP 47821.

## Seguranca

- Criptografia e autenticacao por sessao Noise XX — sem metadados em claro na rede.
- Identidade persistente por maquina; fingerprint para conferencia manual
  (`localchat init` mostra o seu; compare por telefone na primeira conexao, opcional).
- **Codigo de pareamento de 6 digitos**: derivado dos dois fingerprints, os dois
  aparelhos mostram o mesmo numero — basta conferir os digitos, nao o fingerprint.
  Aparece no primeiro contato e no cabecalho de cada conversa. Contra alguem passivo
  ou desatento na rede, codigos iguais garantem que nao ha ninguem no meio.
  **Limite conhecido:** um atacante ativo na mesma rede, no momento do primeiro
  pareamento, consegue fabricar uma chave que gera o mesmo codigo de 6 digitos nos
  dois lados (leva segundos). Em rede de casa o risco e baixo; em rede publica ou de
  escritorio, confira o fingerprint completo (`localchat init`) por telefone.
- Tempo limite no handshake, teto de conexoes e de pedidos de aprovacao simultaneos;
  anuncios falsos na rede nao enchem a lista.
- Updates so com manifesto assinado (minisign) e SHA-256 conferido; ver "Atualizacoes".
- Primeiro contato pede aprovacao explicita na janela ("Confiar / Recusar") com o
  codigo de pareamento; nada do par novo e entregue antes disso.
- Nome duplicado com chave diferente e recusado. Gerenciar com `localchat trust --list` /
  `localchat trust --forget <fingerprint>`.
- SHA-256 conferido no recebimento; arquivo corrompido nao e confirmado.

## Limitacoes

- Recebimento de arquivo e automatico (`--no-auto-accept` recusa tudo).
- Historico de conversas vale para a GUI (`~/.localchat/history.jsonl`, ultimas 1000 linhas); a CLI e so console.
- A versao 0.2 usa o protocolo 2 (confirmacao de entrega): atualize **todos** os
  computadores juntos, a 0.1 nao conversa com a 0.2. Do historico da 0.1 so as mensagens
  recebidas reaparecem (as enviadas nao guardavam o destinatario).

## Testar em casa (duas maquinas)

1. Instale/compile nas duas maquinas e rode `localchat gui` em cada uma.
2. Na lista "Conversas" a outra maquina aparece sozinha, com bolinha verde.
3. Primeiro contato: ao receber a primeira mensagem, a janela "Novo computador quer
   conversar" mostra o codigo — confira que e o mesmo do cabecalho da conversa no
   outro computador e clique **Confiar**.
4. Troque uma mensagem e um arquivo; fechar a transferencia no meio e reenviar
   deve retomar de onde parou.

## Roadmap

- Multiplas transferencias paralelas e progresso por arquivo na lista.
- Pareamento via PAKE para redes hostis.
- Pacote .deb para Linux.
- Codigo de pareamento resistente a ataque ativo (hoje 6 digitos; ver Seguranca).

## Licenca

MIT (ver `LICENSE`): use, modifique e redistribua como quiser, mantendo o aviso de copyright.
