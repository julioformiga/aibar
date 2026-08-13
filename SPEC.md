# aibar — Especificação Técnica

> Monitor TUI de limites de API (janelas de 5h e semanal) para Claude, Z.ai e Gemini.

---

## 1. Visão Geral

`aibar` é uma aplicação de interface de terminal (TUI) minimalista escrita em
Rust que monitora o consumo de limites de taxa de APIs de modelos de
linguagem. É projetada para rodar em um terminal secundário (ex.: um painel
lateral no tmux) e fornecer feedback visual instantâneo sobre quão próximo
cada conta está do limite.

### Objetivos

- Detectar automaticamente quais provedoras estão configuradas (via variáveis
  de ambiente ou arquivo de configuração).
- Exibir o consumo relativo às janelas de **5 horas** e **semanal**.
- Atualizar os dados em segundo plano via polling assíncrono, sem travar a UI.
- Manter um cache local para sobreviver a reinícios e reduzir chamadas.

### Não-objetivos

- Não é um cliente de chat; não envia prompts.
- Não faz automação de fallback entre provedoras.
- Não provê alertas desktop/notificações nativas (apenas visual na TUI).

---

## 2. Stack Tecnológica

| Componente         | Crate        | Versão   | Função                                   |
|--------------------|--------------|----------|------------------------------------------|
| Framework TUI      | `ratatui`    | ^0.28    | Renderização de widgets e layout         |
| Backend terminal   | `crossterm`  | ^0.28    | Eventos de teclado, modo raw, cores      |
| Runtime async      | `tokio`      | ^1.40    | Tasks async, timers, canais              |
| Cliente HTTP       | `reqwest`    | ^0.12    | Chamadas às APIs (com `rustls-tls`)      |
| Serialização       | `serde`      | ^1.0     | (De)serialização JSON                    |
| Data/hora          | `chrono`     | ^0.4     | Timestamps e resets                      |
| Diretórios         | `directories`| ^5.0     | `~/.cache/aibar/` portável               |
| Log                | `tracing`    | ^0.1     | Logs estruturados                        |

> Adicionar feature flags conforme necessário: `reqwest = { version = "^0.12", features = ["json", "rustls-tls"], default-features = false }`.

---

## 3. Modelo de Dados

### 3.1 Janela de Limite

```rust
/// Tipo de janela de rate limiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    FiveHours,
    Weekly,
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
    /// Quantidade usada no período.
    pub used: u64,
    /// Limite total da janela.
    pub limit: u64,
    /// Timestamp ISO 8601 de quando a janela reseta.
    pub reset_at: Option<DateTime<Utc>>,
}

impl LimitWindow {
    /// Porcentagem de uso (0.0–100.0). Retorna 0 se limit == 0.
    pub fn percentage(&self) -> f32 {
        if self.limit == 0 {
            0.0
        } else {
            (self.used as f32 / self.limit as f32) * 100.0
        }
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }
}
```

### 3.2 Provedora

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Provider {
    Claude,
    Zai,
    Gemini,
}

/// Snapshot completo dos limites de uma provedora.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderState {
    pub provider: Provider,
    /// Apelido/label exibido na aba (ex.: "Claude (Pro)").
    pub label: String,
    /// 2 janelas para Claude/Z.ai; 4 janelas para Gemini.
    pub windows: Vec<LimitWindow>,
    /// Quando os dados foram coletados pela última vez.
    pub last_updated: Option<DateTime<Utc>>,
    /// Erro da última tentativa, se houver.
    pub last_error: Option<String>,
}
```

---

## 4. Interface e UX

### 4.1 Layout Geral

```
┌ aibar ──────────────────────────────────────────────────────────────┐
│ [1] Claude   [2] Z.ai   [3] Gemini            [r] Refresh  [q] Quit │
├─────────────────────────────────────────────────────────────────────┤
│                       ACTIVE TAB CONTENT                            │
└─────────────────────────────────────────────────────────────────────┘
```

> **Regra de idioma:** Todas as strings exibidas na TUI (labels, barras,
> erros, dicas) são em **inglês**. A spec é em PT-BR, mas a UI é EN-US.

### 4.2 Aba Claude / Z.ai (2 barras)

```
┌ Claude ─────────────────────────────────────────────────────────────┐
│  5h      2h15  [██████████████░░░░░░░░░░░░░░░░░░░░░░░] 58% 1.1k/2k  │
│  Weekly 1d12h  [████████████████████████████████████] 100% 500/500  │
└─────────────────────────────────────────────────────────────────────┘
```

### 4.3 Aba Gemini (4 barras)

```
┌ Gemini ────────────────────────────────────────────────────────────────┐
│  Google 5h      2h15  [████████░░░░░░░░░░░░░░░░░░░░░░░░░░] 22% 110/500 │
│  Google Weekly 4d10h  [████████████████████████░░░░░░░░░] 65% 3.2k/5k  │
│  ThPart 5h      3h20  [██████████████░░░░░░░░░░░░░░░░░░░░] 36% 180/500 │
│  ThPart Weekly 6d02h  [████████████████████████████░░░░░] 75% 750/1k   │
└────────────────────────────────────────────────────────────────────────┘
```

### 4.4 Estados Especiais

**Carregando (primeira carga):**

```
  5h --  [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░] —
```

**Erro de conexão:**

```
  ⚠ 5h 2h15  [██████████████░░░░░░░░░░░░░░░░░░░░░░░] 58% (cached)
```

**Provedora não configurada (não aparece nas tabs):**

Tabs só exibem provedoras com credenciais detectadas. Se nenhuma credencial
for encontrada, exibe tela de boas-vindas:

```
┌ aibar ──────────────────────────────────────────────────────────────┐
│                    No provider detected.                            │
│  Set at least one of these environment variables:                   │
│    export ANTHROPIC_API_KEY="sk-ant-..."        # Claude            │
│    export ZAI_API_KEY="..."                     # Z.ai              │
│    export GEMINI_API_KEY="..."                  # Gemini            │
│  Docs: https://github.com/.../aibar                                 │
│  [q] Quit                                                           │
└─────────────────────────────────────────────────────────────────────┘
```

### 4.5 Sistema de Cores

| Faixa de uso    | Cor      | Constante (`ratatui::style::Color`)   |
|-----------------|----------|---------------------------------------|
| 0 % – 70 %      | Verde    | `Color::Green`                        |
| 71 % – 90 %     | Amarelo  | `Color::Yellow`                       |
| 91 % – 100 %    | Vermelho | `Color::Red`                          |

```rust
fn color_for_percentage(pct: f32) -> Color {
    match pct {
        p if p <= 70.0 => Color::Green,
        p if p <= 90.0 => Color::Yellow,
        _              => Color::Red,
    }
}
```

### 4.6 Keybindings

| Key            | Action                                            |
|----------------|---------------------------------------------------|
| `1` `2` `3`    | Jump directly to tab 1, 2, or 3                   |
| `Tab`          | Next tab                                          |
| `Shift+Tab`    | Previous tab                                      |
| `r`            | Force refresh (30 s cooldown)                     |
| `q` / `Ctrl+C` | Quit                                              |

---

## 5. Regras de Rate Limit da TUI

A TUI em si tem regras para não abusar das APIs monitoradas:

| Regra                 | Valor / Comportário                                       |
|-----------------------|------------------------------------------------------------|
| Polling automático    | A cada **5 minutos**                                       |
| Cooldown de refresh   | A tecla `r` só dispara nova requisição se a última foi há  |
|                       | mais de **30 segundos**; caso contrário, exibe aviso sutil |
|                       | "Wait Ns to refresh"                                        |
| Cache local           | `~/.cache/aibar/state.json` — gravado a cada update bem-   |
|                       | sucedido; lido na inicialização                            |
| Async não-bloqueante  | Todas as chamadas HTTP rodam em tasks `tokio`; a UI nunca  |
|                       | aguarda rede de forma síncrona                             |
| Backoff em falha      | Em erro consecutivo, dobra o intervalo até 20 min (max)    |
| Timeout HTTP          | 15 segundos por requisição                                 |

### 5.1 Cache — formato `state.json`

Localização: `~/.cache/aibar/state.json`

```json
{
  "providers": [
    {
      "provider": "Claude",
      "label": "Claude (Pro)",
      "windows": [
        {
          "kind": "FiveHours",
          "scope": null,
          "used": 1160,
          "limit": 2000,
          "reset_at": "2026-08-13T17:30:00Z"
        },
        {
          "kind": "Weekly",
          "scope": null,
          "used": 500,
          "limit": 500,
          "reset_at": "2026-08-15T00:00:00Z"
        }
      ],
      "last_updated": "2026-08-13T15:15:00Z",
      "last_error": null
    }
  ]
}
```

---

## 6. Agentes de Coleta

Cada provedora implementa um trait comum:

```rust
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    /// Nome exibido e tipo da provedora.
    fn provider(&self) -> Provider;

    /// Busca os limites atuais. Não deve panicar; erros viram `Err`.
    async fn fetch(&self) -> anyhow::Result<ProviderState>;
}
```

### 6.1 Claude (`agents/claude.rs`)

- **Endpoint:** `GET https://api.anthropic.com/v1/organizations/rate_limits`
  (ou leitura dos headers `anthropic-ratelimit-*` de um request leve).
- **Auth:** `x-api-key: $ANTHROPIC_API_KEY`, `anthropic-version: 2023-06-01`.
- **Mapeamento:** dois limites (5h e weekly/daily conforme o plano).

### 6.2 Z.ai (`agents/zai.rs`)

- **Endpoint:** Documentação da API de rate limits da Z.ai (GLM).
- **Auth:** `Authorization: Bearer $ZAI_API_KEY`.
- **Mapeamento:** dois limites (5h e semanal).

### 6.3 Gemini (`agents/gemini.rs`)

- **Endpoint:** Leitura de headers de rate limit ou endpoint de usage do
  Google AI Studio / Vertex AI.
- **Auth:** `x-goog-api-key: $GEMINI_API_KEY` (ou OAuth do Vertex).
- **Mapeamento:** quatro limites — Modelos Google (5h, semanal) e
  Modelos de Terceiros (5h, semanal).

> **Nota de implementação:** os endpoints e headers exatos de cada provedora
> devem ser confirmados contra a documentação oficial vigente antes de
> implementar. A arquitetura isola cada provedora em seu módulo para
> facilitar ajustes.

### 6.4 Detecção de credenciais

```rust
/// Retorna os agentes cujas credenciais estão presentes no ambiente.
fn detect_agents() -> Vec<Box<dyn Agent>> {
    let mut agents = Vec::new();
    if env::var("ANTHROPIC_API_KEY").is_ok() {
        agents.push(Box::new(ClaudeAgent::from_env()));
    }
    if env::var("ZAI_API_KEY").is_ok() {
        agents.push(Box::new(ZaiAgent::from_env()));
    }
    if env::var("GEMINI_API_KEY").is_ok() {
        agents.push(Box::new(GeminiAgent::from_env()));
    }
    agents
}
```

---

## 7. Arquitetura

### 7.1 Estrutura de Módulos

```
src/
├── main.rs              # Entry point: setup terminal, event loop, teardown
├── app.rs               # AppState: tabs, providers, lógica de polling
├── ui.rs                # Funções de renderização (ratatui)
├── cache.rs             # Leitura/escrita de ~/.cache/aibar/state.json
├── agents/
│   ├── mod.rs           # Trait Agent, enum Provider, detect_agents()
│   ├── claude.rs        # ClaudeAgent
│   ├── zai.rs           # ZaiAgent
│   └── gemini.rs        # GeminiAgent
└── config.rs            # Constantes (intervalos, timeouts, cores)
```

### 7.2 Responsabilidades por Módulo

#### `main.rs`

- Inicializa o runtime `tokio`.
- Configura terminal: `enable_raw_mode`, `EnterAlternateScreen`.
- Cria `AppState` a partir do cache + detecção de agentes.
- Spawna a **task de polling** (loop que dorme 5 min entre updates).
- Executa o **event loop**: lê eventos de teclado do `crossterm` e
  despacha ações (`InputAction`) para o `AppState`.
- Restaura terminal no `Drop` / shutdown.

#### `app.rs`

```rust
pub struct AppState {
    /// Agentes detectados (1–3).
    agents: Vec<Box<dyn Agent>>,
    /// Snapshot em memória, indexado por aba.
    states: Vec<ProviderState>,
    /// Índice da aba ativa.
    active_tab: usize,
    /// Instant do último fetch bem-sucedido (para cooldown).
    last_refresh: Option<Instant>,
    /// Canal para enviar comandos à task de polling.
    tx: mpsc::Sender<PollCommand>,
}

impl AppState {
    pub fn handle_input(&mut self, key: KeyEvent) -> Option<Action>;
    pub fn switch_tab(&mut self, idx: usize);
    pub fn next_tab(&mut self);
    pub fn prev_tab(&mut self);
    pub fn request_refresh(&mut self) -> RefreshResult;
    pub fn apply_update(&mut self, state: ProviderState);
}

/// Resultado de uma tentativa de refresh manual.
pub enum RefreshResult {
    Triggered,
    CooldownActive { secs_remaining: u32 },
}
```

#### `ui.rs`

Função pura que recebe `&AppState` e desenha no `Frame`:

```rust
pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = Layout::vertical([
        Constraint::Length(3),  // tabs + hints
        Constraint::Min(1),    // conteúdo
    ]).split(f.area());

    draw_tabs(f, app, chunks[0]);
    draw_content(f, app, chunks[1]);
}

fn draw_limit_bar(f: &mut Frame, area: Rect, window: &LimitWindow);
```

#### `cache.rs`

```rust
pub struct Cache {
    path: PathBuf,
}

impl Cache {
    pub fn new() -> io::Result<Self>;          // resolve ~/.cache/aibar/
    pub fn load(&self) -> Option<CachedState>;  // Retorna None se não existe
    pub fn save(&self, state: &CachedState) -> io::Result<()>;
}
```

### 7.3 Fluxo de Polling Assíncrono

```
                      ┌──────────────┐
  Event loop          │  AppState    │
  (crossterm)  ──key──│  (tabs, etc) │
         │            └──────┬───────┘
         │                   │ request_refresh()
         │                   ▼
         │         ┌──────────────────┐    mpsc     ┌───────────────┐
         │         │   Poller task    │────────────▶│  Agent fetch  │
         │         │  (loop 5 min +   │             │  (tokio::spawn)│
         │         │   listen canal)  │◀────────────│  reqwest HTTP │
         │         └────────┬─────────┘   result    └───────────────┘
         │                  │ write cache + update AppState
         ▼                  ▼
  ┌──────────┐      ┌──────────────┐
  │   ui.rs  │◀─────│  AppState    │  draw() a cada frame
  │ (render) │      │  .states     │
  └──────────┘      └──────────────┘
```

**Detalhes do poller:**

```rust
// Pseudocódigo da task de polling.
loop {
    let result = agent.fetch().await; // timeout de 15s
    match result {
        Ok(state) => {
            cache.save(&state).await.ok();
            app_tx.send(AppMsg::Update(state)).await.ok();
            interval = Duration::from_secs(300); // reset p/ 5 min
        }
        Err(e) => {
            app_tx.send(AppMsg::Error(e.to_string())).await.ok();
            interval = (interval * 2).min(Duration::from_secs(1200)); // backoff
        }
    }
    tokio::select! {
        _ = tokio::time::sleep(interval) => {},
        cmd = poll_rx.recv() => {
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
│  1. tokio::main                                           │
│  2. setup_terminal()                                      │
│  3. cache.load() → estados iniciais                       │
│  4. detect_agents() → tabs                                │
│  5. spawn poller task                                     │
│                                                           │
│  loop {                                                   │
│      ┌─ event::poll(timeout) ─┐                           │
│      │  Tecla? ──▶ app.handle_input(key)                  │
│      │          ──▶ match Action { Tab, Refresh, Quit }   │
│      │                                                     │
│      │  AppMsg do poller? ──▶ app.apply_update(state)     │
│      │                        cache.save(state)           │
│      └─────────────────────────┘                           │
│                                                           │
│      ui::draw(&mut frame, &app)                           │
│                                                           │
│      if action == Quit { break }                          │
│  }                                                        │
│                                                           │
│  6. restore_terminal()                                    │
└───────────────────────────────────────────────────────────┘
```

---

## 8. Cargo.toml

```toml
[package]
name = "aibar"
version = "0.1.0"
edition = "2021"
description = "TUI monitor for AI API rate limits (Claude, Z.ai, Gemini)"

[dependencies]
ratatui = "0.28"
crossterm = "0.28"
tokio = { version = "1.40", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
directories = "5.0"
tracing = "0.1"
tracing-subscriber = "0.3"
async-trait = "0.1"
anyhow = "1.0"

[profile.release]
opt-level = 3
lto = true
strip = true
```

---

## 9. Variáveis de Ambiente

| Variável            | Provedora | Obrigatória para |
|---------------------|-----------|------------------|
| `ANTHROPIC_API_KEY` | Claude    | Aba Claude       |
| `ZAI_API_KEY`       | Z.ai      | Aba Z.ai         |
| `GEMINI_API_KEY`    | Gemini    | Aba Gemini       |

Opcionais:

| Variável             | Default   | Descrição                          |
|----------------------|-----------|------------------------------------|
| `AIBAR_POLL_SECS`    | `300`     | Intervalo de polling (segundos)    |
| `AIBAR_COOLDOWN_SECS`| `30`      | Cooldown do refresh manual         |
| `AIBAR_LOG`          | `warn`    | Nível de log do `tracing`          |

---

## 10. Considerações de Implementação

1. **Resiliência de terminal:** O `restore_terminal()` deve rodar mesmo em
   `panic` — usar `std::panic::set_hook` para desabilitar raw mode antes de
   imprimir o backtrace.

2. **Renderização eficiente:** O `ui::draw` só é chamado quando há evento ou
   mensagem nova (event-driven), não em loop contínuo, economizando CPU.

3. **Formatação de tempo:** Usar `humantime` ou formatação manual para
   exibir "reseta em 2h15min" em vez de timestamps crus.

4. **Idioma da UI:** Todas as strings exibidas na TUI são em **inglês**
   (labels, barras, erros, dicas). Manter as strings em constantes
   centralizadas em `config.rs` para fácil manutenção.

5. **Testes:** Agentes devem ser testáveis com mocks HTTP (`mockito` em
   `dev-dependencies`); `AppState` e lógica de cores têm testes unitários
   puros sem I/O.
