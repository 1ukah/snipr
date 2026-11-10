# Snipr

Caçador de preços com IA. Você digita o produto ("cadeira DT3 N10 XL") e o Snipr
pesquisa em Mercado Livre, OLX, Facebook Marketplace, Amazon, KaBuM!, Terabyte,
Pichau, Enjoei, Magazine Luiza, Shopee e em qualquer loja que você adicionar. O
resultado é uma lista ordenada por preço, separando novos e usados.

Cada loja é lida da forma mais leve que funciona: por HTTP quando possível, e
num Chrome real só quando a loja exige um navegador. A IA não navega: ela monta
o plano de busca e filtra os anúncios em chamadas curtas.

App nativo em Rust + Tauri (webview do sistema, sem Electron).

## Rodar

**Pré-requisitos**

- [Rust](https://rustup.rs) (stable)
- [Node.js](https://nodejs.org) 22 ou mais recente (o build compila a interface em `ui/`)
- Um navegador baseado no Chromium: Google Chrome (recomendado), Chromium,
  Microsoft Edge ou Brave. Sem navegador, o Snipr lê só as lojas das etapas A e B.
- Para usar o login de uma conta de IA: o CLI do provedor instalado (veja abaixo)
- Ferramentas para compilar o BoringSSL (usado pelo cliente HTTP da etapa B):
  CMake, Perl e um compilador C. No macOS, `brew install cmake` (o Xcode
  Command Line Tools traz o resto). No Linux, `build-essential cmake perl clang libclang-dev`.
  No Windows, CMake, NASM e Perl (ex.: Strawberry Perl), além do Visual Studio Build Tools.
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

### Modelos

O Snipr usa dois modelos, os dois em **Ajustes > IA**:

- **Modelo principal**: opera o agent nas lojas que o Snipr não consegue ler
  sozinho, e repete o filtro quando a resposta do modelo de busca vem fora do
  formato.
- **Modelo de busca**: monta o plano de busca e filtra os anúncios. São tarefas
  curtas de classificação com resposta em JSON. Em **Automático**, o Snipr usa
  a versão mais nova da família pequena do provedor, entre os modelos que a
  conta lista: Haiku (Anthropic), mini (OpenAI), Flash (Google), fast/mini (xAI).
  No login do Claude Code, o apelido `haiku`. No Codex, no Antigravity, no
  Ollama e em endpoints personalizados, o modelo principal.

Os dois campos e o reasoning listam só as opções do provedor escolhido. No
login, a lista de modelos vem do próprio CLI (`codex debug models`,
`agy models`; no Claude Code, os apelidos `opus`, `sonnet` e `haiku`).

A família pequena muda bem menos que os nomes de versão, por isso a escolha
automática não depende de uma lista fixa de modelos. Para comparar modelos na
prática, rode a mesma busca com cada um e compare os anúncios encontrados.

No modo Login e OAuth, o Snipr executa o CLI em modo não interativo e entrega
as ferramentas do navegador por um servidor **MCP** local (`127.0.0.1`, com
token). As ferramentas embutidas dos CLIs (shell, arquivos, web) ficam
desligadas. Para o Antigravity CLI, o Snipr adiciona a regra `mcp(snipr/*)` em
`~/.gemini/antigravity-cli/settings.json`: sem ela, o modo headless bloqueia as
ferramentas do Snipr.

Uma etapa de **planejamento** transforma a frase do usuário em um plano:
produto, alternativas ("gigabyte ou msi"), requisitos, condição, preço máximo e
termos de busca. O plano aparece na interface e guia a busca.

## Como uma busca funciona

1. **Plano** (modelo rápido).
2. **Leitura das lojas, sem IA.** Cada loja é lida na etapa dela:

   | Etapa | Como lê | Exemplos |
   |---|---|---|
   | **A** | HTTP comum | Amazon, KaBuM!, Kalunga, Chipart, lojas Shopify/Nuvemshop/WooCommerce/Magento/Tray |
   | **B** | HTTP com fingerprint TLS/HTTP2 de Chrome ([wreq](https://github.com/0x676e67/wreq)) | Terabyte, Pichau, OLX |
   | **C** | Como B, com os cookies que o Chrome obtém ao passar pela proteção anti-bot | Magazine Luiza |
   | **D** | Chrome abre a busca e o Snipr lê os cards | Mercado Livre, Enjoei, Facebook, Shopee, Casas Bahia |

   Lojas VTEX usam a API pública de catálogo da plataforma (Americanas, Fast
   Shop, Acer). Páginas que trazem os produtos num JSON embutido (ex.:
   `__NEXT_DATA__`) também são lidas por HTTP.
3. **Filtro.** Os cards passam por filtros locais (preço, palavras do plano,
   repetidos). Uma chamada curta à IA por lote de lojas devolve só os ids dos
   anúncios que atendem ao plano, com a condição. Anúncios duvidosos têm a
   página aberta (com especificações e "ver mais" expandidos) e passam por uma
   segunda chamada.
4. **Agent**, só para lojas sem busca conhecida: a IA opera o Chrome com
   ferramentas, registra anúncios pelo id do card e tem um limite de passos.

### Verificação de lojas

Ao adicionar uma loja (ou em **Lojas > Verificar lojas**), o Snipr:

1. Abre a home sem cookies e vê se ela pede login.
2. Acha a URL de busca: formulário da página, busca digitada no campo (como
   uma pessoa) e caminhos conhecidos da plataforma. Uma URL só vale se a página
   mostra resultados da palavra pesquisada.
3. Lê essa busca no Chrome como referência e testa as etapas A, B e C. Fica a
   mais leve que lê pelo menos 70% dos anúncios da referência.

A etapa fica salva nos ajustes. Durante uma busca, se a etapa falhar (bloqueio
ou página sem cards), o Snipr sobe para a próxima e grava a nova. A cada 7 dias
ele testa de novo a etapa mais leve.

Os segredos (tokens e API keys) ficam no cofre do sistema
(Credential Manager / Keychain / Secret Service). A interface só consegue
gravar, trocar ou remover um segredo, nunca ler. Também dá para usar `.env`
(veja `.env.example`).

## Arquitetura

```
crates/
  snipr-browser   CDP próprio, Chrome real, isolated worlds, input humano, anti-desafio, abas reusadas
  snipr-llm       ChatSession: Anthropic (HTTP), OpenAI-compatível
  snipr-core      Engine (orquestrador), busca (search.rs), etapas HTTP (fetch.rs, parse.rs),
                  verificação de lojas (stage.rs), modelos (models.rs), CLIs de IA, ferramentas,
                  servidor MCP, lojas, cofre, ajustes
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

- **Navegadores compatíveis**: Chrome, Chromium, Edge e Brave (Windows, macOS
  e Linux). Em **Ajustes > Navegador** o Snipr lista os encontrados e testa o
  escolhido. Todos falam o mesmo protocolo (CDP). O Brave randomiza o
  fingerprint, e alguns sistemas anti-bot tratam isso como suspeito. No Linux,
  versões em Snap ou Flatpak podem não acessar a pasta do perfil do Snipr.
- **Um único processo Chrome** com perfil persistente próprio, aberto só quando
  alguma loja precisa dele (etapas C e D, agent).
- **Abas reusadas**: ao terminar, a aba vai para about:blank e fica aberta para
  o próximo uso. Uma página de loja parada continua rodando scripts (~55% de CPU
  com 4 abas, medido no macOS); estacionada em about:blank, ~0,1%. O Chrome
  ocioso fecha após 30 minutos (ajustável).
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
- **CPU antes de RAM**: quando é preciso escolher, o Snipr gasta RAM para
  poupar CPU (abas e Chrome mantidos abertos, composição na GPU). Imagens,
  fontes, mídia e redes de anúncio/rastreamento continuam bloqueadas.
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
SNIPR_TEST_SITES=kabum,dt3 cargo test -p snipr-core --release verifica_lojas -- --ignored --nocapture
                                        # verifica lojas reais (Chrome + internet, sem IA); SNIPR_TEST_PROFILE = pasta do perfil
cargo test -p snipr-core export_bindings  # regenera os tipos TypeScript em ui/src/bindings
npm --prefix ui run check               # checagem de tipos da interface
```

Ferramentas de diagnóstico (sem IA, sem custo):

| Comando | Para quê |
|---|---|
| `cargo run -p snipr-browser --example probe --release -- "<url>" [hidden\|headless\|visible]` | Abre a URL, mostra desafio, sinais de automação, cards extraídos (preços, riscado, título, link) e snapshot |
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
