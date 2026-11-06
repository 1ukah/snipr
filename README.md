# Snipr

Caçador de preços com agentes de IA. Você digita o produto ("cadeira DT3 N10 XL")
e agentes controlam um Chrome real para pesquisar em Mercado Livre, OLX,
Facebook Marketplace, Amazon, KaBuM!, Terabyte, Pichau, Enjoei, Magazine Luiza
e Shopee. O resultado é uma lista ordenada por preço, separando novos e usados.

App nativo em Rust + Tauri (webview do sistema, sem Electron).

## Rodar

**Pré-requisitos**

- [Rust](https://rustup.rs) (stable)
- [Node.js](https://nodejs.org) 22 ou mais recente (o build compila a interface em `ui/`)
- Google Chrome (ou Edge/Chromium)
- Para usar o login de uma conta de IA: o CLI do provedor instalado (veja abaixo)
- Linux: dependências do Tauri (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`) e `libdbus-1-dev` para o cofre de segredos

```sh
cargo run -p snipr-desktop --release
```

O primeiro build roda `npm ci` e `npm run build` em `ui/` automaticamente. Para
compilar só o Rust (por exemplo, no CI), use `SNIPR_SKIP_UI_BUILD=1`.

Na primeira vez, abra **Ajustes** e configure a IA.

## Provedores de IA

Três formas de conectar, em **Ajustes > IA**:

| Forma | Provedores | Como funciona |
|---|---|---|
| **Login** (padrão) | Anthropic, OpenAI, Google | Usa a conta já logada no CLI oficial: [Claude Code](https://claude.com/claude-code) (`claude`), [Codex](https://developers.openai.com/codex/cli) (`codex`) ou [Antigravity CLI](https://antigravity.google/docs/cli) (`agy`). O botão **Entrar** abre o login do CLI num terminal. |
| **API key** | Anthropic, OpenAI, Google Gemini, xAI, OpenRouter, Ollama (local) e qualquer endpoint compatível com OpenAI | Chamadas HTTP diretas. A lista de modelos vem da API do provedor. |
| **OAuth** | Anthropic | Token de `claude setup-token`, usado pelo Claude Code. |

No modo Login e OAuth, o Snipr executa o CLI em modo não interativo e entrega
as ferramentas do navegador por um servidor **MCP** local (`127.0.0.1`, com
token). As ferramentas embutidas dos CLIs (shell, arquivos, web) ficam
desligadas. Para o Antigravity CLI, o Snipr adiciona a regra `mcp(snipr/*)` em
`~/.gemini/antigravity-cli/settings.json`: sem ela, o modo headless bloqueia as
ferramentas do Snipr.

Antes dos agentes, uma etapa de **planejamento** transforma a frase do usuário
em um plano: produto, alternativas ("gigabyte ou msi"), requisitos, condição,
preço máximo e termos de busca. O plano aparece na interface e guia os agentes.

Os segredos (tokens e API keys) ficam no cofre do sistema
(Credential Manager / Keychain / Secret Service). A interface só consegue
gravar, trocar ou remover um segredo, nunca ler. Também dá para usar `.env`
(veja `.env.example`).

## Arquitetura

```
crates/
  snipr-browser   CDP próprio, Chrome real, isolated worlds, input humano, anti-desafio
  snipr-llm       ChatSession: Anthropic (HTTP), OpenAI-compatível
  snipr-core      Engine (orquestrador), planejamento, CLIs de IA, ferramentas, servidor MCP, lojas, cofre, ajustes
apps/
  desktop         Tauri: ponte fina entre a UI e o Engine
ui/               TypeScript + Vite, sem framework. Tipos gerados do Rust em ui/src/bindings
```

**Integrações futuras** (Telegram, Discord, WhatsApp, API de SaaS) só precisam
de `Engine::start_search(SearchRequest, sink)` e consumir o fluxo de
`SearchEvent` (serializável em JSON). O `Engine` pode ser chamado de qualquer
thread, e nada nele depende da interface. `crates/snipr-core/examples/search.rs`
é um frontend de linha de comando completo.

### Navegador e anti-bot

- **Um único processo Chrome** com perfil persistente próprio. Cada agente ganha
  uma janela nesse processo; é por isso que o multi-agente não multiplica a RAM.
- **Modo oculto** (padrão): Chrome com janela real fora da tela, sem nenhum
  sinal de headless.
- **Sem `Runtime.enable`**, sem `--enable-automation`, `navigator.webdriver`
  falso. Os scripts do agente rodam num *isolated world* invisível para a página.
- **Mouse e teclado** via `Input.dispatch*`: os eventos chegam ao site como
  `isTrusted`, com trajetórias em curva e cadência variável.
- **Desafios**: o Snipr espera a verificação automática do Cloudflare e clica no
  Turnstile se ele aparecer. Se não passar, mostra a janela e pede ajuda.
  Bloqueios 403/429 na borda (ex.: Akamai) levam a uma espera e uma nova
  tentativa; se persistirem, o site é pulado com aviso.
- **Economia de memória**: bloqueio de imagens, fontes, mídia e redes de
  anúncio/rastreamento; composição de GPU em software (o WebGL continua na GPU
  real); fechamento do Chrome após um tempo ocioso. Cerca de 1,2 GB com
  4 agentes em paralelo, contra cerca de 2 GB de um Chrome comum.
- **O que não desligar**: `--disable-gpu` (o WebGL vira "Microsoft Basic Render
  Driver") e `--disable-site-isolation-trials` (o sensor do Akamai bloqueia)
  economizam RAM, mas denunciam a automação.

### Logins nas lojas

- **Contas > Entrar**: abre a loja numa janela visível. Você faz login uma vez
  (inclusive 2FA) e a sessão fica salva no perfil.
- Se uma loja pedir login durante uma busca, o Snipr detecta a tela de login,
  mostra a janela e pede para você entrar. O agente nunca digita senhas.
- Lojas novas (em **Lojas > +**) passam por uma verificação: o Snipr abre a
  página, detecta se ela requer login e procura o campo de busca.

### Textos da interface

A interface segue um português técnico simplificado, inspirado no ASD-STE100.
Veja [docs/escrita.md](docs/escrita.md).

## Desenvolvimento

```sh
cargo clippy --workspace          # lint
cargo test --workspace            # testes rápidos
cargo test -p snipr-core -- --ignored   # ponta a ponta: Chrome real + MCP + CLIs de IA (usa a IA)
cargo test -p snipr-core export_bindings  # regenera os tipos TypeScript em ui/src/bindings
npm --prefix ui run check               # checagem de tipos da interface
```

Ferramentas de diagnóstico (sem IA, sem custo):

| Comando | Para quê |
|---|---|
| `cargo run -p snipr-browser --example probe --release -- "<url>" [hidden\|headless\|visible]` | Abre a URL, mostra desafio, sinais de automação, anúncios extraídos e snapshot |
| `cargo run -p snipr-browser --example debug --release -- "<url>" "<js>" ...` | Avalia expressões JS no isolated world da página |
| `cargo run -p snipr-browser --example ram --release` | Mede a RAM do Chrome com 4 abas em paralelo (Windows) |
| `cargo run -p snipr-browser --example webgl --release` | Mostra o que os sites enxergam do WebGL |
| `cargo run -p snipr-core --example search -- "<produto>" site1,site2` | Busca completa pelo `Engine`, com o provedor e os segredos configurados (**usa a IA**) |
| `cargo run -p snipr-core --example plan -- "<frase>" [login\|api_key\|oauth] [provedor]` | Mostra o plano de busca gerado pela IA, sem abrir o navegador (**usa a IA**) |

Variáveis de ambiente para experimentos com o Chrome:

- `SNIPR_CHROME_ARGS="--flag1 --flag2"`: adiciona flags.
- `SNIPR_CHROME_BARE=1`: remove todas as flags opcionais, útil para bisseccionar detecções.

Arquivos do app em execução:

| O quê | Windows | Onde fica |
|---|---|---|
| Ajustes | `%APPDATA%\snipr\settings.json` | `dirs::config_dir()/snipr` |
| Perfil do Chrome | `%LOCALAPPDATA%\snipr\chrome-profile` | `dirs::data_local_dir()/snipr` |
| Log de crash | `%LOCALAPPDATA%\snipr\crash.log` | idem |
