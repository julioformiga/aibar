# aibar — Especificação Técnica

> Monitor TUI de limites de API (janelas de 5h e 7 dias) para Claude, Z.ai e Gemini, e saldo de Hypercredits para Hyper (Charm).

---

## 1. Visão Geral

`aibar` é uma aplicação de interface de terminal (TUI) minimalista escrita em
Rust que monitora o consumo de limites de taxa de APIs de modelos de
linguagem. É projetada para rodar em um terminal secundário (ex.: um painel
lateral no tmux) e fornecer feedback visual instantâneo sobre quão próximo
cada conta está do limite.

### Objetivos

- Detectar automaticamente quais provedoras e fontes estão configuradas (via
  variáveis de ambiente, arquivos de credenciais, ou fontes locais).
- Suportar múltiplas fontes por provedora (ex.: Claude OAuth + Claude API key)
  com navegação por `Enter`.
- Exibir o consumo relativo às janelas de **5 horas** e **7 dias** em barras
  de progresso, ou tetos de taxa (RPM/TPM) quando apenas limites por minuto
  estão disponíveis.
- Atualizar os dados em segundo plano via polling assíncrono, sem travar a UI.
- Manter um cache local para sobreviver a reinícios e reduzir chamadas.
- Mostrar um timer visual de contagem regressiva até o próximo poll.

### Não-objetivos

- Não é um cliente de chat; não envia prompts.
- Não faz automação de fallback entre provedoras.
- Não provê alertas desktop/notificações nativas (apenas visual na TUI).

---

## 2. Stack Tecnológica

| Componente         | Crate        | Versão   | Função                                       |
|--------------------|--------------|----------|----------------------------------------------|
| Framework TUI      | `ratatui`    | ^0.30    | Renderização de widgets e layout             |
| Backend terminal   | `crossterm`  | ^0.29    | Eventos de teclado (event-stream), modo raw |
| Runtime async      | `tokio`      | ^1.53    | Tasks async, timers, canais                 |
| Cliente HTTP       | `reqwest`    | ^0.13    | Chamadas às APIs (`rustls-no-provider`)     |
| Provider TLS       | `rustls`     | ^0.23    | Crypto provider `ring` (pure Rust)          |
| Serialização       | `serde`      | ^1.0     | (De)serialização JSON                        |
| JSON               | `serde_json` | ^1.0     | Parsing de respostas e cache                 |
| Data/hora          | `chrono`     | ^0.4     | Timestamps e resets                          |
| Diretórios         | `directories`| ^6.0     | `~/.cache/aibar/` portável                   |
| Log                | `tracing`    | ^0.1     | Logs estruturados                            |
| Log subscriber     | `tracing-subscriber` | ^0.3 | Filtro de log por env-var             |
| Async trait        | `async-trait`| ^0.1     | Trait `Agent` assíncrono                     |
| Erros              | `anyhow`     | ^1.0     | Error handling ergonômico                    |
| Futures utils      | `futures`    | ^0.3     | `EventStream` do crossterm                   |

> `reqwest = { version = "^0.13", features = ["json", "rustls-no-provider"], default-features = false }`
> `rustls = { version = "^0.23", default-features = false, features = ["ring", "logging", "std", "tls12"] }`
> `crossterm = { version = "^0.29", features = ["event-stream"] }`

> **Nota:** `rustls-no-provider` + `ring` evita `aws-lc-sys`, que requer
> compilação C (e segfaulta no rustc 1.97.1). O crypto provider `ring` é
> instalado manualmente em `main.rs` via
> `rustls::crypto::ring::default_provider().install_default()`.

---

## 3. Modelo de Dados

### 3.1 Tipos Fundamentais

```rust
/// Tipo de janela de rate limiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    FiveHours,
    SevenDays,
}

/// Subcategoria de limite (usado apenas pelo Gemini).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitScope {
    Standard,   // Gemini: Modelos Google
    ThirdParty, // Gemini: Modelos de Terceiros
}

/// Estado de uma única janela de rate limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitWindow {
    pub kind: WindowKind,
    pub scope: Option<LimitScope>,
    pub used: u64,
    pub limit: u64,
    pub reset_at: Option<DateTime<Utc>>,
}
```

`LimitWindow` possui dois construtores auxiliares:

- **`from_fraction(kind, scope, used_fraction, reset_at)`** — usado quando a
  API retorna apenas uma porcentagem de utilização (Claude OAuth, Gemini). O
  `limit` é preenchido com `NOTIONAL_LIMIT` (1000) para que a barra reflita a
  fração real.
- **`from_values(kind, scope, used, limit, reset_at)`** — usado quando a API
  retorna valores absolutos de usado/limite (Z.ai).

```rust
impl LimitWindow {
    pub fn percentage(&self) -> f32;  // 0.0–100.0
    pub fn remaining(&self) -> u64;
}
```

### 3.2 Provedora

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Claude,
    Zai,
    Gemini,
    Hyper,
}

impl Provider {
    pub fn label(self) -> &'static str;  // "Claude", "Z.ai", "Gemini", "Hyper"
}
```

### 3.3 SourceState — Quota vs Ceiling

Cada fonte de dados produz um `SourceState`, que pode ser de dois tipos:

```rust
/// Snapshot de limites percentuais (janelas 5h/7d).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderState {
    pub provider: Provider,
    pub label: String,
    pub windows: Vec<LimitWindow>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Teto de taxa por grupo de modelo (RPM, TPM in/out).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ceiling {
    pub group: String,
    pub rpm: u64,
    pub in_tpm: u64,
    pub out_tpm: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CeilingReport {
    pub label: String,
    pub ceilings: Vec<Ceiling>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Saldo de créditos pré-pagos (ex.: Hypercredits do Hyper).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditsState {
    pub label: String,
    pub balance: Option<f64>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SourceState {
    Quota(ProviderState),    // Barras de progresso (Claude OAuth, Z.ai, Gemini)
    Ceiling(CeilingReport),  // Tabela RPM/TPM (Claude API key)
    Credits(CreditsState),   // Barra de saldo (Hyper)
}

impl SourceState {
    pub fn label(&self) -> &str;
    pub fn last_error(&self) -> Option<&str>;
    pub fn last_updated(&self) -> Option<DateTime<Utc>>;
    pub fn set_error(&mut self, err: String);
    /// Detecta mudança observável de uso vs. outro snapshot, usada pelo
    /// watch mode. Quota compara cada janela (used/limit por kind+scope);
    /// Credits compara o saldo; Ceiling nunca muda.
    pub fn usage_changed(&self, other: &SourceState) -> bool;
}
```

**Quando cada tipo é usado:**

| Fonte                     | SourceState  | Motivo                                             |
|---------------------------|--------------|----------------------------------------------------|
| Claude OAuth (Pro/Max)    | `Quota`      | API retorna % de utilização por janela             |
| Claude API key (Console)  | `Ceiling`    | API retorna tetos RPM/TPM, não janelas             |
| Z.ai                      | `Quota`      | API retorna usado/total por janela                 |
| Gemini (Antigravity/agy)  | `Quota`      | Servidor local retorna fração restante por janela  |
| Hyper (Charm)             | `Credits`    | API retorna saldo de Hypercredits, não janelas     |

---

## 4. Interface e UX

### 4.1 Layout Geral

```
┌ AIBar │ Claude (Pro) │ Z.ai │ Gemini ──────────────━━━━━━━━━━━┄┄┄┄┄┄┄┄┄┐
│                       ACTIVE TAB CONTENT                               │
│  ⚠  Error                                                              │
└──────────────────────────────────────── Enter Source  Refresh  Quit ───┘
```

O título da janela é um **breadcrumb** com separadores `│`. Cada aba mostra o
label da fonte ativa (ex.: "Claude (Pro)"). A aba selecionada é destacada em
amarelo, negrito e sublinhado.

No canto superior direito, um **timer bar** (`━━━━┄┄┄┄`) mostra o progresso
entre o último poll e o próximo, atualizado a cada segundo.

A linha inferior esquerda (dentro da borda) mostra status (erro ou mensagem
de cooldown). A linha inferior direita, também embutida na borda
(`title_bottom` alinhado à direita), mostra dicas de keybindings: o rótulo
completo da ação (`↑↓ Theme`, `Watch`, `Refresh`, `Quit`, e opcionalmente
`Enter Source`), separados por dois espaços, com a tecla de atalho em
**negrito** dentro do próprio rótulo (`Refresh` → `R` em negrito), em vez de
uma letra solta antes da palavra.

> **Regra de idioma:** Todas as strings exibidas na TUI (labels, barras,
> erros, dicas) são em **inglês**. A spec é em PT-BR, mas a UI é EN-US.

### 4.2 Aba Quota — Barras de Progresso (Claude OAuth, Z.ai)

```
┌ AIBar │ Claude (Pro) │ Z.ai │ Gemini ────────────━━━━━━━━━━━┄┄┄┄┄┄┄┄┄┐
│       0h30m/5h  [█████████████░░░░░░░░░░░░░░░░░░░░░░░░░]  32% 320/1k │
│       6d16h/7d  [██░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   4%  40/1k │
└───────────────────────────────────── Enter Source  Refresh  Quit ────┘
```

O formato do campo de tempo é `XhYm/5h` ou `XdYh/7d`, onde a parte antes
da barra é o tempo até o reset e a parte após a barra (`/5h` ou `/7d`)
indica a janela, exibida em cor cinza escuro para destaque visual.

Os valores `used/limit` à direita são formatados com sufixos (`1.1k`, `3.2M`).
Quando a API retorna apenas porcentagem, o limite é notional (1000), então
exibe `580/1k`.

### 4.3 Aba Quota — Gemini (4 barras)

```
┌ Gemini ────────────────────────────────────────────━━━━━━┄┄┄┄┄┄┄┄┄┄┄─┐
│  Google   2h15m/5h  [████████░░░░░░░░░░░░░░░░░░░░░░░░░░] 22% 220/1k  │
│  Google 4d10h/7d  [████████████████████████████░░░░░░░░░] 65% 650/1k │
│  Partner  3h20m/5h  [██████████████░░░░░░░░░░░░░░░░░░░░] 36% 360/1k  │
│  Partner 6d02h/7d  [████████████████████████████░░░░░] 75% 750/1k    │
└──────────────────────────────────────────────────────────────────────┘
```

A ordem das barras é sempre: Google 5h, Google 7d, Partner 5h, Partner 7d.

> **Scopes do Gemini:**
> - **Google** — Modelos do Google (Gemini Flash, Pro, etc.)
> - **Partner** — Modelos de terceiros (Claude, GPT, etc. via Antigravity)

### 4.4 Aba Ceiling — Tabela RPM/TPM (Claude API key)

```
┌ Claude (API) ────────────────────────────────────━━━━━━┄┄┄┄┄┄┄┄┄┄┄─┐
│  claude-sonnet-4    50 RPM    100k in    20k out                   │
│  claude-opus-4      10 RPM     20k in     8k out                   │
│  claude-haiku-4    100 RPM    200k in    40k out                   │
└────────────────────────────────────────────────────────────────────┘
```

Cada linha mostra o grupo (nome do modelo sem timestamp), RPM em ciano,
TPM de entrada em verde, TPM de saída em amarelo.

### 4.5 Aba Credits — Saldo (Hyper)

```
┌ Hyper ─────────────────────────────────────────────━━━━━━┄┄┄┄┄┄┄┄┄┄┄─┐
│  Credits  [███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  23% bal 77 │
└──────────────────────────────────────────────────────────────────────┘
```

A API de credits do Hyper (`GET /v1/credits`) retorna apenas o saldo
(`{"balance": 100}`), não janelas de uso. A barra representa o quanto da
**mesada gratuita mensal** (`HYPER_FREE_CREDITS` = 100 Hypercredits) já foi
consumida: `spent = 100 - balance` (0 quando o saldo excede a mesada, ex.:
creditos comprados), mantendo a semântica das demais barras (barra cheia e
vermelha = quase sem créditos). O sufixo à direita mostra o saldo absoluto
(`bal 77`; valores fracionários com 1 casa decimal, ex. `bal 42.5`).

Estados especiais: antes do primeiro fetch exibe "Loading credits…"; em erro
com dados em cache, o prefixo `⚠` e o sufixo `(cached)` — igual às abas Quota.

### 4.6 Múltiplas Fontes por Provedora

Uma provedora pode ter múltiplas fontes (ex.: Claude OAuth + Claude API key).
Nesse caso, o label da aba mostra a fonte ativa e `Enter` alterna (cycle)
entre as fontes disponíveis. A dica "Enter Source" aparece na linha de
hints quando há múltiplas fontes.

### 4.7 Watch Mode (auto-switch por mudança de uso)

A tecla `w` alterna o **watch mode**. Quando ativo, todas as fontes continuam
polling em segundo plano (mesmo as de abas inativas) e, ao detectar uma
mudança no uso de uma fonte, o aibar troca automaticamente para a aba
daquela provedora. A linha de status exibe brevemente `watch on`
ou `watch off`.

A detecção de mudança (`SourceState::usage_changed`) compara o estado de cada
fonte:

- **Quota** — compara cada janela individualmente (`used`/`limit` por
  `kind` + `scope`). Uma mudança em qualquer janela conta, não apenas na de
  maior uso (ex.: o consumo de 5h pode subir enquanto o de 7d permanece).
- **Credits** — compara o saldo bruto (`balance`).
- **Ceiling** — nunca dispara auto-switch (não tem métrica de uso).

Uma fonte sem dado prévio (primeiro fetch com janelas vazias, ou sem saldo)
nunca dispara a troca. Com o watch desligado, o comportamento volta ao
padrão: apenas a aba ativa faz polling e nenhuma troca automática ocorre.

### 4.8 Estados Especiais

**Carregando (primeira carga):**

```
       --/5h  [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░] —%
```

**Erro de conexão (com dados em cache):**

```
  ⚠     2h15m/5h  [██████████████░░░░░░░░░░░░░░░░░░░░░░░] 58% (cached)
```

O ícone `⚠` aparece amarelo na primeira barra; o sufixo muda para `(cached)`.
Erros também aparecem na linha de status inferior em vermelho.

**Provedora não configurada (nenhuma credencial detectada):**

```
┌ aibar ──────────────────────────────────────────────────────────────┐
│                    No provider detected.                            │
│  Set at least one of these environment variables:                   │
│    export ANTHROPIC_API_KEY="..."   # Claude (API)                  │
│    export ZAI_API_KEY="..."         # Z.ai                          │
│    export GEMINI_API_KEY="..."      # Gemini                        │
│    export HYPER_API_KEY="..."       # Hyper (Charm)                 │
│  Fallbacks: ~/.claude/.credentials.json,                            │
│    pass Z_AI_API_KEY, Antigravity (agy)                             │
│  [q] Quit                                                           │
└─────────────────────────────────────────────────────────────────────┘
```

### 4.9 Sistema de Cores

| Faixa de uso    | Cor      | Constante (`ratatui::style::Color`)   |
|-----------------|----------|---------------------------------------|
| 0 % – 70 %      | Verde    | `Color::Green`                        |
| 71 % – 90 %     | Amarelo  | `Color::Yellow`                       |
| 91 % – 100 %    | Vermelho | `Color::Red`                          |

```rust
pub fn color_for_percentage(pct: f32) -> Color {
    if pct <= COLOR_LOW_THRESHOLD {       // 70.0
        Color::Green
    } else if pct <= COLOR_HIGH_THRESHOLD { // 90.0
        Color::Yellow
    } else {
        Color::Red
    }
}
```

### 4.10 Temas

Três temas embutidos (`src/theme.rs`), alternados com `↑`/`↓` (com wrap) e
persistidos no cache (`state.json` → `theme`). O tema ativo é exibido
brevemente na linha de status (`theme: crush`).

| Tema       | Identidade visual                                                                 |
|------------|-----------------------------------------------------------------------------------|
| `default`  | Visual clássico do aibar: bordas retas, barras `[█▒]`, cores por faixa (verde/amarelo/vermelho), aba ativa amarela sublinhada |
| `crush`    | Paleta Charmtone Pantera (do agente Crush): bordas arredondadas em lilás Hazy (`#8B75FF`, como o box de input do Crush) sobre fundo escuro `#1F1C23` (extraído da screenshot do Crush, pintado inclusive sob a moldura), título branco brilhante (Salt `#F7F6FB`) com separadores de aba em Iron (`#4D4C57`), aba ativa magenta Dolly (`#FF60FF`), barras `(━┄)` com gradiente Hazy→Dolly (`#8B75FF`→`#FF60FF`), severidade menta/mostarda/rosa (`#00FFB2`/`#F5EF34`/`#EB4268`), texto sutil em Squid (`#858392`), timer em gradiente lilás→magenta |
| `btop`     | Tema default do btop: borda esverdeada (`#556D59`), aba ativa com fundo vermelho selecionado (`#6A2F2F`), barras sem colchetes com gradiente CPU (`#77CA9B`→`#CBC06C`→`#DC4C4C`) sobre trilho cinza sólido, timer amarelo (`#CBC06C`) |

A paleta define: background sólido opcional (o tema `crush` pinta o painel
inteiro com `#1F1C23`, inclusive sob a moldura; os demais usam o fundo do
terminal), tipo/cor de borda, estilo do título e das abas, separadores,
caracteres de barra (cheio/vazio, aberturas), cor de trilho vazio, função de
cor por posição da barra (gradiente) e por porcentagem, cor do sufixo
`/5h`/`/7d`, cores de status/erro/warn, cores das dicas, cores do timer,
carregando, RPM/TPM e tela de boas-vindas.

No tema `default`, `bar_fill`/`pct_color` usam os mesmos thresholds de
`color_for_percentage` (Seção 4.8), preservando o comportamento original.
Nos temas `crush` e `btop`, cada célula da barra recebe a cor do gradiente na
sua posição (`i / (bar_width - 1)`), como os gráficos do btop.

### 4.11 Keybindings

| Key             | Action                                            |
|-----------------|---------------------------------------------------|
| `1`–`9`         | Jump directly to tab 1–9                          |
| `Tab` / `→`     | Next tab                                          |
| `Shift+Tab` / `←` | Previous tab                                    |
| `↑` / `↓`       | Cycle theme (default → crush → btop, com wrap)    |
| `Enter`         | Cycle source (apenas se há múltiplas fontes)      |
| `r`             | Force refresh (30 s cooldown)                     |
| `w`             | Toggle watch mode (auto-switch ao mudar uso)      |
| `q` / `Ctrl+C`  | Quit                                              |

---

## 5. Regras de Rate Limit da TUI

A TUI em si tem regras para não abusar das APIs monitoradas:

| Regra                 | Valor / Comportário                                       |
|-----------------------|------------------------------------------------------------|
| Polling automático    | A cada **5 minutos** (`AIBAR_POLL_SECS`)                   |
| Cooldown de refresh   | A tecla `r` só dispara nova requisição se a última foi há  |
|                       | mais de **30 segundos** (`AIBAR_COOLDOWN_SECS`); caso      |
|                       | contrário, exibe "Wait Ns to refresh"                      |
| Cache local           | `~/.cache/aibar/state.json` — gravado a cada update/event; |
|                       | lido na inicialização                                      |
| Async não-bloqueante  | Todas as chamadas HTTP rodam em tasks `tokio` separadas;   |
|                       | cada fonte tem sua própria task de polling                 |
| Backoff em falha      | Em erro consecutivo, dobra o intervalo até 20 min (max)    |
| Timeout HTTP          | 15 segundos por requisição (`HTTP_TIMEOUT_SECS`)           |
| Redraw tick           | A cada 1 segundo (para atualizar o timer bar do título)    |

### 5.1 Cache — formato `state.json`

Localização: `~/.cache/aibar/state.json`

```json
{
  "active_tab": 0,
  "theme": "crush",
  "active_sources": {
    "Claude": 0,
    "Gemini": 0
  },
  "sources": [
    {
      "provider": "Claude",
      "source_id": "oauth",
      "state": {
        "Quota": {
          "provider": "Claude",
          "label": "Claude (Pro)",
          "windows": [
            {
              "kind": "FiveHours",
              "scope": null,
              "used": 580,
              "limit": 1000,
              "reset_at": "2026-08-13T17:30:00Z"
            }
          ],
          "last_updated": "2026-08-13T15:15:00Z",
          "last_error": null
        }
      }
    },
    {
      "provider": "Claude",
      "source_id": "api-key",
      "state": {
        "Ceiling": {
          "label": "Claude (API)",
          "ceilings": [
            { "group": "claude-sonnet-4", "rpm": 50, "in_tpm": 100000, "out_tpm": 20000 }
          ],
          "last_updated": "2026-08-13T15:15:00Z",
          "last_error": null
        }
      }
    }
  ]
}
```

O cache preserva:
- `active_tab`: última aba selecionada.
- `theme`: último tema selecionado (`"default"`, `"crush"` ou `"btop"`);
  ausente em caches antigos → `default` (via `#[serde(default)]`).
- `active_sources`: última fonte ativa por provedora (ex.: `{"Claude": 0}`).
- `sources`: snapshot completo de cada fonte, usado para exibir dados em
  cache enquanto o primeiro poll não completa.

---

## 6. Agentes de Coleta

Cada fonte implementa um trait comum:

```rust
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    /// Tipo da provedora (Claude, Zai, Gemini).
    fn provider(&self) -> Provider;

    /// Identificador único da fonte dentro da provedora (ex.: "oauth",
    /// "api-key", "default").
    fn source_id(&self) -> &str;

    /// Estado inicial exibido antes do primeiro fetch bem-sucedido.
    fn initial_state(&self) -> SourceState;

    /// Busca os limites atuais. Não deve panicar; erros viram `Err`.
    async fn fetch(&self) -> anyhow::Result<SourceState>;
}
```

### 6.1 Claude — OAuth (`agents/claude.rs` → `ClaudeOAuthAgent`)

- **Detecção:** Lê `~/.claude/.credentials.json` (ou `$CLAUDE_CONFIG_DIR/
  .credentials.json`). Hoje detecta no máximo uma conta OAuth por processo
  (um único caminho de credenciais é verificado).
- **Label:** Lê `~/.claude.json` → `oauthAccount.organizationType` para
  determinar o tier: "Claude (Pro)", "Claude (Max)", "Claude (Team)", ou
  "Claude (OAuth)" como fallback.
- **source_id:** `"oauth"`
- **Endpoint:** `GET https://api.anthropic.com/api/oauth/usage`
- **Auth:** `Authorization: Bearer <accessToken>` (lido do credentials.json).
- **Resposta:** JSON com `five_hour.utilization` (0–100) e
  `seven_day.utilization` (0–100), mais `resets_at` (RFC 3339).
- **Mapeamento:** `SourceState::Quota` com 2 janelas (5h, 7d), usando
  `from_fraction` (limit notional = 1000).

### 6.2 Claude — API Key (`agents/claude.rs` → `ClaudeApiAgent`)

- **Detecção:** Variável de ambiente `ANTHROPIC_API_KEY`.
- **Label:** `"Claude (API)"`
- **source_id:** `"api-key"`
- **Endpoint:** `GET https://api.anthropic.com/v1/organizations/rate_limits`
- **Auth:** `x-api-key: $ANTHROPIC_API_KEY`,
  `anthropic-version: 2023-06-01`.
- **Resposta:** Array `data[]` com entradas `group_type: "model_group"`,
  cada uma contendo `models[]`, e `limits[]` com tipos
  `requests_per_minute`, `input_tokens_per_minute`,
  `output_tokens_per_minute`.
- **Mapeamento:** `SourceState::Ceiling` com um `Ceiling` por grupo de
  modelo. O nome do modelo é simplificado removendo o sufixo de data
  (8 dígitos finais após último `-`).

### 6.3 Z.ai (`agents/zai.rs` → `ZaiAgent`)

- **Detecção:** Variável de ambiente `ZAI_API_KEY`. **Fallback:** comando
  `pass Z_AI_API_KEY` (unix password store).
- **Label:** `"Z.ai"`
- **source_id:** `"default"`
- **Endpoint:** `GET https://api.z.ai/api/monitor/usage/quota/limit`
- **Auth:** `Authorization: Bearer $ZAI_API_KEY`
- **Resposta:** `data.limits[]`, cada item tem `unit` (3 = 5h, 6 = 7d),
  e valores de uso. O parser tenta múltiplas estratégias:
  1. `currentValue` + `usage` → valores absolutos (`from_values`).
  2. `currentValue` + `remaining` → `used` e `used + remaining`.
  3. `percentage` → fração (`from_fraction`).
  - `nextResetTime` (ms epoch) é usado como `reset_at`.
- **Mapeamento:** `SourceState::Quota` com 2 janelas (5h, 7d).

### 6.4 Gemini (`agents/gemini.rs` → `GeminiAgent`)

- **Detecção:** Variável de ambiente `GEMINI_API_KEY` **ou** existência do
  diretório `~/.gemini/antigravity-cli/log` (indica que Antigravity/agy
  está instalado).
- **Label:** `"Gemini"`
- **source_id:** `"default"`
- **Mecanismo:** Não usa a API do Google diretamente. Em vez disso, consulta
  o **servidor local do Antigravity (agy)** via gRPC-over-HTTP em
  `127.0.0.1:<port>`:

  1. **Busca de porta candidata:**
     - Lê arquivos `~/.gemini/antigravity-cli/log/cli-*.log` (mais recentes
       primeiro), procurando pela linha
       `"Language server listening on random port at <port> for HTTP"`.
     - Se não encontrar nos logs, executa `ss -tulpn` e procura processos
       `"agy"`.
  2. **Probe:** `POST http://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary`
     com body `{}`. Valida se a resposta contém `"groups"`.
  3. **Auto-start:** Se nenhuma porta responder, localiza o binário `agy`
     no `PATH`, executa `agy --dangerously-skip-permissions --print ok`,
     aguarda até 15s por uma nova porta aparecer nos logs, e tenta novamente.
     O processo agy é morto após a coleta.
  4. **Login obrigatório:** Se o agy auto-iniciado não conseguir autenticar
     silenciosamente (marcadores nos logs: `"Print mode: silent auth failed"`
     ou `"Print mode: triggering interactive OAuth"`, sem
     `"authenticated successfully"`), o aibar mata o processo antes que a
     tela de login do Google abra e retorna o erro
     `"agy login required: run 'agy' in another terminal to log in, then press R to retry"`.
     O poller não tenta novamente automaticamente nesse caso (evita reabrir
     o login a cada ciclo); apenas um refresh manual (`R`) retenta. A
     autenticação não pode ser evitada: o endpoint de quota exige conta
     Google.

  - **Endpoint gRPC:**
    `http://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary`
  - **Resposta:** `response.groups[]`, cada grupo tem `displayName`
    ("Gemini Models" → `Standard`, resto → `ThirdParty`) e `buckets[]`
    com `window` ("5h" ou "weekly"), `remainingFraction` (0.0–1.0) e
    `resetTime` (RFC 3339).
  - **Mapeamento:** `SourceState::Quota` com até 4 janelas (Google 5h,
    Google 7d, Partner 5h, Partner 7d), usando `from_fraction` com
    `1.0 - remainingFraction`. Ordenação fixa por (scope, kind).

### 6.5 Hyper — Charm (`agents/hyper.rs` → `HyperAgent`)

- **Detecção:** Variável de ambiente `HYPER_API_KEY` (não vazia). Sem
  fallback.
- **Label:** `"Hyper"`
- **source_id:** `"default"`
- **Endpoint:** `GET https://hyper.charm.land/v1/credits`
- **Auth:** `Authorization: Bearer $HYPER_API_KEY`
- **Resposta:** `{"balance": <number>}` — saldo atual de Hypercredits da
  equipe autenticada (cada usuário recebe 100 Hypercredits/mês grátis).
- **Mapeamento:** `SourceState::Credits` com `balance` (ver Seção 4.5 para a
  renderização). Erros 401 (`authentication_error`) viram erro HTTP do
  reqwest e seguem o fluxo padrão de erro/backoff.

### 6.6 Detecção de Credenciais

```rust
fn detect_agents() -> Vec<Box<dyn Agent>> {
    let mut agents = Vec::new();

    // Claude OAuth (uma conta por processo, hoje)
    for oauth in ClaudeOAuthAgent::detect_all() {
        agents.push(Box::new(oauth));
    }
    // Claude API key
    if let Some(a) = ClaudeApiAgent::from_env() {
        agents.push(Box::new(a));
    }
    // Z.ai (env var ou pass)
    if let Some(a) = ZaiAgent::from_env() {
        agents.push(Box::new(a));
    }
    // Gemini (env var ou logs do agy)
    if let Some(a) = GeminiAgent::from_env() {
        agents.push(Box::new(a));
    }
    // Hyper (Charm)
    if let Some(a) = HyperAgent::from_env() {
        agents.push(Box::new(a));
    }
    agents
}
```

Agentes são **agrupados por `Provider`** em `build_tabs()` — cada grupo
vira uma aba; múltiplos agentes do mesmo provider viram fontes alternáveis
dentro da aba.

---

## 7. Arquitetura

### 7.1 Estrutura de Módulos

```
src/
├── main.rs              # Entry point: setup terminal, event loop, teardown,
│                        #   build_tabs(), run_poller(), cache save
├── app.rs               # AppState, Tab, SourceSlot, Action, AppMsg, PollCommand
├── ui.rs                # Funções de renderização (ratatui)
├── cache.rs             # Leitura/escrita de ~/.cache/aibar/state.json
├── config.rs            # Constantes, env-var overrides, color_for_percentage
├── model.rs             # Tipos de dados (Provider, SourceState, LimitWindow, etc.)
└── agents/
    ├── mod.rs           # Trait Agent, detect_agents()
    ├── claude.rs        # ClaudeOAuthAgent + ClaudeApiAgent
    ├── zai.rs           # ZaiAgent
    ├── hyper.rs         # HyperAgent (credits do Hyper/Charm)
    └── gemini.rs        # GeminiAgent
```

### 7.2 Tipos Centrais da Aplicação

```rust
pub enum Action { Quit }

pub enum RefreshResult {
    Triggered,
    CooldownActive { secs_remaining: u32 },
}

/// Mensagens enviadas pelas tasks de polling para o event loop.
pub enum AppMsg {
    Update { provider: Provider, source_id: String, state: SourceState },
    Error { provider: Provider, source_id: String, error: String },
    Scheduled { provider: Provider, source_id: String, next_at: Instant },
}

pub enum PollCommand { ForceRefresh }

/// Uma fonte de dados dentro de uma aba.
pub struct SourceSlot {
    pub id: String,
    pub state: SourceState,
    pub poll_tx: mpsc::Sender<PollCommand>,
    pub last_poll_at: Option<Instant>,
    pub next_poll_at: Option<Instant>,
}

/// Uma aba, agrupando fontes da mesma provedora.
pub struct Tab {
    pub provider: Provider,
    pub sources: Vec<SourceSlot>,
    pub active: usize,  // índice da fonte ativa
}

impl Tab {
    pub fn cycle_source(&mut self);   // Enter
    pub fn active_state(&self) -> Option<&SourceState>;
}

pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub last_refresh: Option<Instant>,
    pub status_message: Option<String>,
    pub theme: Theme,
    pub watch_mode: bool,  // auto-switch ao mudar porcentagem de uso
}

impl AppState {
    pub fn handle_input(&mut self, key: KeyEvent) -> Option<Action>;
    pub fn switch_tab(&mut self, idx: usize);
    pub fn next_tab(&mut self);
    pub fn prev_tab(&mut self);
    pub fn request_refresh(&mut self) -> RefreshResult;
    pub fn apply_update(&mut self, provider, source_id, state);
    pub fn apply_error(&mut self, provider, source_id, error);
    pub fn apply_scheduled(&mut self, provider, source_id, next_at);
    pub fn active_poll_timing(&self) -> Option<(Instant, Instant)>;
    pub fn toggle_watch_mode(&mut self);
}
```

### 7.3 Fluxo de Polling Assíncrono

Cada fonte (`SourceSlot`) tem sua própria task `tokio` que roda
`run_poller()`:

```
                      ┌──────────────┐
  Event loop          │  AppState    │
  (crossterm)  ──key──│  (tabs, etc) │
         │            └──────┬───────┘
         │                   │ request_refresh()
         │                   │   → slot.poll_tx.try_send(ForceRefresh)
         │                   ▼
         │         ┌──────────────────┐    mpsc     ┌────────────────┐
         │         │  run_poller()    │────────────▶│  agent.fetch() │
         │         │  (1 task por     │             │  (reqwest HTTP │
         │         │   fonte)         │◀────────────│   ou agy)      │
         │         └────────┬─────────┘   AppMsg    └────────────────┘
         │                  │ send Update/Error/Scheduled
         ▼                  ▼
  ┌──────────┐      ┌──────────────┐
  │   ui.rs  │◀─────│  AppState    │  draw() a cada frame
  │ (render) │      │  .tabs[].    │
  └──────────┘      │  sources[]   │
                    └──────────────┘
```

**Detalhes do poller (`run_poller`):**

```rust
// Pseudocódigo — uma task por fonte (agent).
let mut interval = poll_interval(); // 5 min
loop {
    // Fetch com timeout de 15s
    let result = tokio::time::timeout(http_timeout(), agent.fetch()).await;
    match result {
        Ok(Ok(state)) => {
            app_tx.send(AppMsg::Update { .. }).await;
            interval = poll_interval(); // reset backoff
        }
        Ok(Err(e)) => {
            app_tx.send(AppMsg::Error { error: e.to_string() }).await;
            interval = (interval * 2).min(max_backoff()); // backoff até 20 min
        }
        Err(_) => { // timeout
            app_tx.send(AppMsg::Error { error: "request timed out" }).await;
            interval = (interval * 2).min(max_backoff());
        }
    }

    // Notifica o event loop do próximo poll agendado (para o timer bar)
    let next_at = Instant::now() + interval;
    app_tx.send(AppMsg::Scheduled { next_at }).await;

    // Dorme até o próximo intervalo, ou até receber ForceRefresh
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        cmd = cmd_rx.recv() => {
            if matches!(cmd, Some(PollCommand::ForceRefresh)) {
                continue; // pula o sleep imediatamente
            }
        }
    }
}
```

### 7.4 Ciclo de Vida do Event Loop

```
┌─ main() ──────────────────────────────────────────────────┐
│                                                           │
│  1. init_tracing() — log em ~/.cache/aibar/aibar.log      │
│  2. rustls::crypto::ring::default_provider().install()    │
│  3. setup_terminal() — raw mode, alt screen, panic hook   │
│  4. cache.load() → estados iniciais                       │
│  5. detect_agents() → agentes detectados                  │
│                                                           │
│  if agentes vazios:                                       │
│    loop { draw_welcome; wait key; if Quit break }         │
│    return                                                 │
│                                                           │
│  6. build_tabs(agentes, cache, app_tx)                    │
│     - agrupa por Provider                                 │
│     - para cada agente: spawn run_poller()                │
│  7. AppState::new(tabs)                                   │
│                                                           │
│  loop {                                                   │
│      ui::draw(&mut frame, &app)                           │
│                                                           │
│      tokio::select! {                                     │
│          key = events.next() =>                           │
│              app.handle_input(key)                        │
│              save_cache(&cache, &app)                     │
│              if Quit { break }                            │
│                                                           │
│          msg = app_rx.recv() =>                           │
│              match msg {                                  │
│                  Update  => app.apply_update(); save_cache│
│                  Error   => app.apply_error()             │
│                  Scheduled => app.apply_scheduled()       │
│              }                                            │
│                                                           │
│          _ = redraw_tick.tick() => {} // 1s, só redesenha │
│      }                                                    │
│  }                                                        │
│                                                           │
│  8. restore_terminal()                                    │
└───────────────────────────────────────────────────────────┘
```

O `redraw_tick` de 1 segundo garante que o **timer bar** do título seja
atualizado continuamente, mesmo sem eventos.

### 7.5 Renderização (`ui.rs`)

```rust
pub fn draw(f: &mut Frame, app: &AppState) {
    // Block com:
    //   title = breadcrumb (AIBar │ <label fonte ativa> │ ...)
    //   title right = timer bar (━━━━┄┄┄┄)
    //   title_bottom = status line (erro/mensagem)
    //   title_bottom right = hint line (Enter Source  Refresh  Quit)
    //
    // inner content:
    //   if empty → draw_welcome()
    //   else → match active_state {
    //       Quota(ps)   → draw_quota_lines()  // barras de progresso
    //       Ceiling(cr) → draw_ceiling_lines() // tabela RPM/TPM
    //   }
}
```

**Timer bar:** Usa `app.active_poll_timing()` que retorna
`(last_poll_at, next_poll_at)`. Calcula a fração decorrida e renderiza
`━━━` (preenchido, `U+2501`) + `┄┄┄` (vazio, `U+2504`), largura fixa de 20.

---

## 8. Cargo.toml

```toml
[package]
name = "aibar"
version = "0.1.0"
edition = "2021"
description = "TUI monitor for AI API rate limits (Claude, Z.ai, Gemini)"

[dependencies]
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
tokio = { version = "1.53", features = ["full"] }
reqwest = { version = "0.13", features = ["json", "rustls-no-provider"], default-features = false }
rustls = { version = "0.23", default-features = false, features = ["ring", "logging", "std", "tls12"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
directories = "6.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
async-trait = "0.1"
anyhow = "1.0"
futures = "0.3"

[dev-dependencies]
mockito = "1.7"

[profile.release]
opt-level = 3
lto = true
strip = true
```

---

## 9. Variáveis de Ambiente

| Variável            | Provedora | Obrigatória para         |
|---------------------|-----------|--------------------------|
| `ANTHROPIC_API_KEY` | Claude    | Fonte Claude (API)       |
| `ZAI_API_KEY`       | Z.ai      | Fonte Z.ai (ou `pass`)   |
| `GEMINI_API_KEY`    | Gemini    | Gemini (opcional se agy) |
| `CLAUDE_CONFIG_DIR` | Claude    | Diretório alt. p/ creds  |

Opcionais:

| Variável             | Default   | Descrição                          |
|----------------------|-----------|------------------------------------|
| `AIBAR_POLL_SECS`    | `300`     | Intervalo de polling (segundos)    |
| `AIBAR_COOLDOWN_SECS`| `30`      | Cooldown do refresh manual         |
| `AIBAR_LOG`          | `warn`    | Nível de log do `tracing`          |

**Fallbacks de credenciais (sem env var):**

| Provedora | Fonte        | Fallback                                        |
|-----------|--------------|-------------------------------------------------|
| Claude    | OAuth        | `~/.claude/.credentials.json`                   |
| Z.ai      | default      | `pass Z_AI_API_KEY` (unix password store)       |
| Gemini    | default      | `~/.gemini/antigravity-cli/log/` (servidor agy) |

---

## 10. Considerações de Implementação

1. **Resiliência de terminal:** O `restore_terminal()` deve rodar mesmo em
   `panic` — `std::panic::set_hook` desabilita raw mode antes de imprimir o
   backtrace.

2. **Renderização eficiente:** O `ui::draw` é chamado a cada iteração do
   `tokio::select!` (evento de teclado, mensagem do poller, ou tick de 1s).
   O tick de 1s é necessário para atualizar o timer bar do título.

3. **Formatação de tempo:** Countdown manual em `reset_countdown()` —
   `XdYh` para 7 dias, `XhYm` para 5 horas. Valores numéricos formatados
   com `fmt_count()` (`1.1k`, `3.2M`).

4. **Idioma da UI:** Todas as strings exibidas na TUI são em **inglês**
   (labels, barras, erros, dicas). Constantes de configuração e URLs em
   `config.rs`.

5. **Poller por fonte:** Cada `SourceSlot` tem seu próprio canal
   `mpsc::Sender<PollCommand>` e sua própria task `run_poller`. Isso
   permite que fontes com diferentes latências (ex.: Gemini via agy local
   vs Claude OAuth via HTTPS) pollem independentemente.

6. **Agrupamento por provider:** `build_tabs()` agrupa agentes pelo
   `Provider` retornado. Assim, `ClaudeOAuthAgent` e `ClaudeApiAgent`
   aparecem na mesma aba "Claude", navegáveis com `Enter`.

7. **Timer bar:** Usa `Instant` (monotônico), não `DateTime<Utc>`, para
   evitar problemas de mudança de relógio do sistema.

8. **Tracing:** Logs vão para `~/.cache/aibar/aibar.log` se o diretório
   existir, senão para `stderr`. Nível controlado por `AIBAR_LOG`.

9. **Testes:** `mockito` disponível em `dev-dependencies` para mockar
   respostas HTTP. Lógica de cores e parsing têm testes unitários puros.
