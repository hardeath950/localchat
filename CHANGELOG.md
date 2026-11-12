# Changelog

As versões novas são escritas pelo release-plz a partir dos commits (`feat:`, `fix:`...).

## [0.2.4](https://github.com/hardeath950/localchat/compare/v0.2.3...v0.2.4) - 2026-11-12

### Added

- servidor MCP para agentes de IA (localchat mcp)

### Other

- como conectar agentes de IA via MCP no README

## [0.2.3](https://github.com/hardeath950/localchat/compare/v0.2.2...v0.2.3) - 2026-11-12

### Added

- compartilhar pasta com um contato por tempo determinado

### Fixed

- *(gui)* janela de pastas compartilhadas encolhe ao abrir pasta menor

### Other

- pasta compartilhada e hash durante o envio no DESENVOLVIMENTO
- transferencia de arquivo 2,4x mais rapida (hash durante o envio)

## [0.2.2](https://github.com/hardeath950/localchat/compare/v0.2.1...v0.2.2) - 2026-11-12

### Added

- visual estilo WhatsApp Web, perfil com foto e recado, progresso de arquivos grandes

## [0.2.1](https://github.com/hardeath950/localchat/compare/v0.2.0...v0.2.1) - 2026-11-12

### Added

- instalacao sem compilar - link fixo no Windows e instalador de um comando no Linux

### Fixed

- tira gtk/glib 0.18 do Cargo.lock (alerta GHSA-wrw7-89jp-8q8g)
- build Windows sem rust-toolchain.toml (rustup novo recusa o host no canal)

## [0.2.0]

### Novo
- Uma conversa por computador, com bolinha online/offline, contador de não lidas
  (lista, título da janela e ícone da bandeja) e hora em cada mensagem.
- "Entregue" só depois da confirmação do outro lado; falhas mostram "tentar de novo".
- Arquivos: arrastar para a janela, barra de progresso com "Cancelar", "Abrir pasta".
- Mudar o próprio nome na janela (o padrão agora é o nome do computador).
- "Conectar por IP" para redes onde o broadcast não passa.
- Abrir o LocalChat de novo traz a janela que já estava aberta.
- Linux: compila e roda (Zorin/Ubuntu), bandeja via D-Bus, X minimiza.
- Update automático assinado (minisign) a partir do GitHub Releases.
- Aviso quando o outro computador usa uma versão incompatível.

### Correções
- Enter não enviava a mensagem.
- Um `.part` antigo travava para sempre o recebimento de arquivos com o mesmo nome.
- Janela de console preta ao abrir pelo atalho no Windows.

### Segurança
- Tempo limite no handshake, limite de conexões e de pedidos de aprovação simultâneos.
- Nomes de arquivo reservados do Windows (CON, NUL, COM1...) e caracteres de controle.
- Pasta `~/.localchat` (chave privada) só legível pelo dono no Linux.
- A conexão confere se a chave do par é a mesma anunciada na rede.

### Incompatível
- Protocolo 2: a 0.1 não conversa com a 0.2. Atualize todos os computadores juntos.
