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
- **Confianca (TOFU)**: a primeira conexao grava o fingerprint do par; se depois o mesmo
  nome aparecer com outra chave, a conexao e recusada (possivel sequestro de nome).
- **Arquivos**: transferidos em chunks de 32 KiB com SHA-256 verificado no destino.

## Compilar

```bash
cargo build --release
# binario em target/release/localchat.exe
```

## Uso

### Interface grafica (recomendado)

```bash
localchat gui
```

Abre a janela do LocalChat: veja quem esta na rede na lista da esquerda, clique no
par, digite a mensagem e pressione Enter. "Enviar arquivo" abre o seletor de
arquivos do Windows. Tudo que chega aparece na conversa.

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
- Nome duplicado com chave diferente e recusado. Gerenciar com `localchat trust --list` /
  `localchat trust --forget <fingerprint>`.
- SHA-256 conferido no recebimento; arquivo corrompido nao e confirmado.

## Limitacoes v0

- Recebimento de arquivo e automatico (`--no-auto-accept` recusa tudo).
- Sem historico persistente de conversas (mensagens aparecem no console).
- Sem retomada de transferencia interrompida.

## Roadmap

- GUI leve (system tray + janela) para uso familiar.
- Emparelhamento por codigo curto (PAKE) em vez de conferencia de fingerprint.
- Historico de conversas local (SQLite) e multiplas transferencias paralelas.
- Empacotamento em instalador (MSI) e release para Windows/Linux.
