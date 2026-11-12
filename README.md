# LocalChat

Converse e mande arquivos entre os computadores da sua casa ou escritório, direto
pela rede local — sem internet, sem servidor, sem cadastro. Tudo criptografado.
Funciona no **Windows 10/11** e no **Linux** (Zorin OS, Ubuntu e parecidos), e um
conversa com o outro.

## Instalar

### Windows

1. Baixe o instalador: **[LocalChat-Setup.exe](https://github.com/hardeath950/localchat/releases/latest/download/LocalChat-Setup.exe)**
2. Abra o arquivo baixado. Se o Windows mostrar "O Windows protegeu o computador",
   clique em **Mais informações → Executar assim mesmo** (o aviso aparece porque o
   instalador ainda não tem assinatura paga da Microsoft).
3. Avance até o fim. O LocalChat abre sozinho e fica no menu Iniciar.

### Linux (Zorin OS, Ubuntu)

Abra o **Terminal** e cole este comando:

```bash
wget -qO- https://raw.githubusercontent.com/hardeath950/localchat/main/scripts/install-linux.sh | sh
```

Ele baixa a versão mais nova, confere o arquivo, cria o ícone no menu de aplicativos
e pergunta se o LocalChat deve abrir junto com o computador.

## Primeiro uso

1. Instale nos computadores que vão conversar e abra o LocalChat em todos.
2. Em alguns segundos os outros aparecem na lista **Conversas**, com bolinha verde.
3. Clique em um deles e mande uma mensagem. No outro computador aparece
   **"Novo computador quer conversar"** com um código de 6 números: confira que é o
   mesmo código mostrado no topo da conversa do primeiro computador e clique **Confiar**.
   Isso só acontece uma vez por computador.

## No dia a dia

- **Mensagem:** digite e aperte Enter (Shift+Enter quebra a linha). ✓✓ aparece quando o
  outro recebeu. Botão direito numa mensagem copia o texto.
- **Arquivo:** botão **+** ou arraste para a janela (vários de uma vez). Arquivos grandes
  mostram a porcentagem e quanto tempo falta; o **✕** cancela. Os recebidos ficam na pasta
  **Downloads/LocalChat**: clique no arquivo para abrir (botão direito mostra na pasta).
- **Seu perfil:** clique na sua foto, no topo à esquerda, para mudar nome, recado e foto.
  Todos os computadores da rede veem o seu perfil.
- **Dados de um contato:** clique no nome dele no topo da conversa (foto, recado e código
  de segurança).
- **Tema claro ou escuro:** menu **⋮ → Tema**.
- **Fechar no X** deixa o LocalChat rodando para continuar recebendo (no Windows ele fica
  perto do relógio; no Linux, minimizado). Para fechar de vez: **Sair**.
- **Atualizações:** quando sair versão nova, aparece **"Atualizar agora"** no topo da
  janela. Um clique e pronto. Atualize todos os computadores — versões diferentes avisam
  "outra versão do LocalChat" e não conversam.

## Problemas comuns

**O outro computador não aparece na lista**
- Os dois precisam estar na mesma rede (mesmo Wi-Fi/roteador) e com o LocalChat aberto.
- No Windows, se perguntar sobre o firewall, permita em **Redes privadas**. Se a rede
  do Windows estiver como "Pública", mude para "Privada" (Configurações → Rede).
- No Linux com firewall ligado: `sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp`.
- Ainda não apareceu? Digite o IP do outro computador em **Conectar**, à esquerda (o IP
  de cada um aparece logo abaixo desse campo).

**No Linux, o ícone perto do relógio não aparece**
Algumas áreas de trabalho escondem esses ícones. O LocalChat continua funcionando:
abra pelo menu de aplicativos.

## Desinstalar

- **Windows:** Configurações → Aplicativos → LocalChat → Desinstalar.
- **Linux:** `rm -rf ~/.local/share/localchat ~/.local/share/applications/localchat.desktop ~/.config/autostart/localchat.desktop`

Suas conversas e a identidade do computador ficam em `~/.localchat` (apague se quiser
remover tudo).

## Segurança, em resumo

- Tudo que passa pela rede é criptografado; só os computadores que você confirmou
  (com o código de 6 números) conseguem conversar com o seu.
- Atualizações só são instaladas se vierem assinadas pelo projeto.
- Em rede pública ou de escritório, confira o código com atenção no primeiro contato.
  Detalhes e limites em [DESENVOLVIMENTO.md](DESENVOLVIMENTO.md#segurança).

---

Para quem quer compilar, publicar versões ou entender o protocolo:
[DESENVOLVIMENTO.md](DESENVOLVIMENTO.md). Licença: [MIT](LICENSE).
