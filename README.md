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

## Compilar

```bash
cargo build --release
# Windows: binario em target/x86_64-pc-windows-gnu/release/localchat.exe
# Linux:   binario em target/release/localchat
```

## Linux (Zorin OS, Ubuntu e derivados)

No computador Linux, baixe/clone este projeto e rode:

```bash
./scripts/build-linux.sh
./target/release/localchat gui
```

O script instala as dependencias (GTK, appindicator), o Rust e compila. O protocolo
e o mesmo do Windows — **Windows e Linux conversam entre si na mesma rede** (a
descoberta e o pareamento funcionam entre os dois).

Se usar firewall (ufw): `sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp`.
Para lancar pelo menu de aplicativos, crie `~/.local/share/applications/localchat.desktop`
apontando para o binario com o argumento `gui`.

## Instalador (para as maquinas de casa)

```bash
cargo build --release
"%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" setup.iss
# gera dist\LocalChat-Setup-0.1.0.exe
```

Rode o exe gerado nas maquinas: instala em `C:\Program Files\LocalChat`, cria atalho
no menu iniciar (e opcionalmente na area de trabalho), libera o firewall e oferece
iniciar com o Windows. Desinstalar pelo Painel de Controle remove tudo (inclusive a
regra do firewall).

## Uso

### Interface grafica (recomendado)

```bash
localchat gui
```

Abre a janela do LocalChat: veja quem esta na rede na lista da esquerda, clique no
par, digite a mensagem e pressione Enter. "Enviar arquivo" abre o seletor de
arquivos do Windows. Tudo que chega aparece na conversa.

A janela tem icone na bandeja do Windows: fechar com o X esconde para a bandeja
(o app continua rodando e recebendo); clique no icone para reabrir e o menu com
botao direito tem "Abrir LocalChat" e "Sair".

### Linha de comando

```bash
# 1) na primeira vez, cria a identidade (uma vez por maquina)
localchat init --name notebook-raul

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
/msg notebook-raul ola
/send notebook-raul C:/Users/Raul/Documents/planilha.xlsx
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
  Aparece no primeiro contato e ao lado de cada par na lista. Se um invasor estiver
  no meio, os codigos aparecem diferentes nos dois lados.
- Primeiro contato pede aprovacao explicita na janela ("Confiar / Recusar") com o
  codigo de pareamento; nada do par novo e entregue antes disso.
- Nome duplicado com chave diferente e recusado. Gerenciar com `localchat trust --list` /
  `localchat trust --forget <fingerprint>`.
- SHA-256 conferido no recebimento; arquivo corrompido nao e confirmado.

## Limitacoes v0

- Recebimento de arquivo e automatico (`--no-auto-accept` recusa tudo).
- Historico de conversas vale para a GUI (`~/.localchat/history.jsonl`, ultimas 1000 linhas); a CLI e so console.

## Testar em casa (duas maquinas)

1. Instale/compile nas duas maquinas e rode `localchat gui` em cada uma.
2. Na lista "Na rede" a outra maquina aparece sozinha (com o codigo de pareamento).
3. Primeiro contato: a janela "Novo par quer conectar" mostra o codigo — confira
   que e o mesmo nos dois aparelhos e clique **Confiar** nos dois.
4. Troque uma mensagem e um arquivo; fechar a transferencia no meio e reenviar
   deve retomar de onde parou.

## Roadmap

- Multiplas transferencias paralelas e progresso por arquivo na lista.
- Pareamento via PAKE para redes hostis.
- Pacote .deb para Linux e atualizacoes automaticas.
