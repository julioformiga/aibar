use crate::agents::Agent;
use crate::model::{LimitWindow, Provider, ProviderState, SourceState, WindowKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Prefix of the error shown when the Codex CLI is not authenticated with a
/// ChatGPT account. The poller matches on it to stop auto-retrying until the
/// user forces a refresh, since no amount of waiting fixes it.
pub const LOGIN_REQUIRED_ERROR: &str = "codex login required";

/// Returns true when an agent error means the Codex quota cannot be fetched
/// until the user runs `codex login` interactively.
pub fn is_login_required(error: &str) -> bool {
    error.starts_with(LOGIN_REQUIRED_ERROR)
}

const LOGIN_HINT: &str = "run `codex login` in another terminal, then press R to retry";

/// Reads the ChatGPT plan quota that Codex exposes over its app-server
/// (JSON-RPC on stdio). It never starts a thread or a turn, so it neither
/// sends prompts nor consumes quota — it only reads `account/read` and
/// `account/rateLimits/read`.
pub struct CodexAgent {
    bin: PathBuf,
}

/// Kills the spawned `codex` process on drop, including when this future is
/// cancelled mid-await by the outer timeout in `run_poller` — plain
/// `tokio::process::Child` does not kill its child on drop.
struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

impl CodexAgent {
    pub fn from_env() -> Option<Self> {
        let bin = codex_bin()?;
        tracing::info!(bin = %bin.display(), "codex from_env");
        Some(Self { bin })
    }

    /// Runs the app-server handshake and returns the `account/read` and
    /// `account/rateLimits/read` results.
    async fn read_account_and_limits(&self) -> anyhow::Result<(Value, Value)> {
        let child = tokio::process::Command::new(&self.bin)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn codex app-server: {e}"))?;
        let mut guard = ChildGuard(child);

        let mut stdin = guard
            .0
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("codex app-server stdin unavailable"))?;
        let stdout = guard
            .0
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("codex app-server stdout unavailable"))?;
        let mut lines = BufReader::new(stdout).lines();

        // The app-server rejects every other method until this handshake
        // completes, and accepts it only once per connection.
        send(
            &mut stdin,
            json!({
                "method": "initialize",
                "id": 0,
                "params": {
                    "clientInfo": {
                        "name": "aibar",
                        "title": "aibar",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }
            }),
        )
        .await?;
        read_result(&mut lines, 0).await?;
        send(&mut stdin, json!({ "method": "initialized", "params": {} })).await?;

        send(
            &mut stdin,
            json!({
                "method": "account/read",
                "id": 1,
                "params": { "refreshToken": false }
            }),
        )
        .await?;
        let account = read_result(&mut lines, 1).await?;

        send(
            &mut stdin,
            json!({ "method": "account/rateLimits/read", "id": 2 }),
        )
        .await?;
        let limits = read_result(&mut lines, 2).await?;

        let _ = stdin.shutdown().await;
        drop(stdin);
        let _ = guard.0.kill().await;
        let _ = guard.0.wait().await;

        Ok((account, limits))
    }
}

/// Writes one JSON-RPC message as a single line (the app-server speaks
/// newline-delimited JSON and omits the `"jsonrpc"` field on the wire).
async fn send(stdin: &mut tokio::process::ChildStdin, msg: Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(&msg)?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|e| anyhow::anyhow!("failed to write to codex app-server: {e}"))?;
    stdin
        .flush()
        .await
        .map_err(|e| anyhow::anyhow!("failed to flush codex app-server: {e}"))?;
    Ok(())
}

/// Reads lines until the response with `id` arrives, skipping notifications
/// and responses to other requests.
async fn read_result<R>(
    lines: &mut tokio::io::Lines<BufReader<R>>,
    id: i64,
) -> anyhow::Result<Value>
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read from codex app-server: {e}"))?
    {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if msg["id"].as_i64() != Some(id) {
            continue;
        }
        if let Some(error) = msg.get("error") {
            let message = error["message"].as_str().unwrap_or("unknown error");
            return Err(anyhow::anyhow!("codex app-server error: {message}"));
        }
        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
    }
    Err(anyhow::anyhow!(
        "codex app-server closed before answering request {id}"
    ))
}

/// Resolves the Codex binary: `AIBAR_CODEX_BIN` wins, otherwise the first
/// `codex` found in `PATH`. Only a filesystem check happens here — no
/// subprocess is spawned during detection, so startup never blocks.
fn codex_bin() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("AIBAR_CODEX_BIN") {
        let path = PathBuf::from(raw);
        if path.as_os_str().is_empty() {
            return None;
        }
        return path.is_file().then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join("codex");
            candidate.is_file().then_some(candidate)
        })
    })
}

/// Builds the source label from the account payload. The plan is shown only
/// when Codex reports one; it is never guessed from the quota values.
fn label_for_account(account: &Value) -> String {
    match account["planType"].as_str().map(plan_label) {
        Some(Some(plan)) => format!("OpenAI (Codex {plan})"),
        _ => "OpenAI (Codex)".to_string(),
    }
}

/// Maps the plan identifiers Codex reports to display names. Unknown values
/// return `None` so the label degrades to a neutral "OpenAI (Codex)".
fn plan_label(plan: &str) -> Option<&'static str> {
    match plan.trim().to_ascii_lowercase().as_str() {
        "free" => Some("Free"),
        "go" => Some("Go"),
        "plus" => Some("Plus"),
        "pro" => Some("Pro"),
        "pro_lite" | "prolite" => Some("Pro Lite"),
        "team" => Some("Team"),
        "business" | "self_serve_business_prolite" | "self_serve_business_usage_based" => {
            Some("Business")
        }
        "enterprise" | "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" => {
            Some("Enterprise")
        }
        "edu" | "education" | "edu_plus" | "edu_pro" => Some("Edu"),
        _ => None,
    }
}

/// Fails when Codex is not signed in with a ChatGPT account. An API-key login
/// is reported separately: it authenticates model calls but carries no
/// ChatGPT plan quota, so there is nothing to display.
fn check_chatgpt_account(account: &Value) -> anyhow::Result<()> {
    match account["account"]["type"].as_str() {
        Some("chatgpt") => Ok(()),
        Some("apiKey") => Err(anyhow::anyhow!(
            "{LOGIN_REQUIRED_ERROR}: codex is using an API key (no ChatGPT plan quota); {LOGIN_HINT}"
        )),
        _ => Err(anyhow::anyhow!("{LOGIN_REQUIRED_ERROR}: {LOGIN_HINT}")),
    }
}

/// Extracts the ChatGPT quota windows. Prefers the `codex` entry of
/// `rateLimitsByLimitId` and falls back to the single-snapshot `rateLimits`
/// view, so the two are never counted twice.
fn parse_rate_limits(limits: &Value) -> Vec<LimitWindow> {
    let snapshot = match limits["rateLimitsByLimitId"].get("codex") {
        Some(v) if v.is_object() => v,
        _ => &limits["rateLimits"],
    };

    ["primary", "secondary"]
        .iter()
        .filter_map(|slot| parse_window(&snapshot[slot]))
        .collect()
}

/// Parses one quota window. A window without `usedPercent` is reported as
/// missing rather than as zero usage.
fn parse_window(window: &Value) -> Option<LimitWindow> {
    let used_percent = window["usedPercent"].as_f64()?;
    let kind = window_kind(window["windowDurationMins"].as_u64());
    let reset_at = window["resetsAt"]
        .as_i64()
        .filter(|secs| *secs > 0)
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0));

    Some(LimitWindow::from_fraction(
        kind,
        None,
        (used_percent / 100.0) as f32,
        reset_at,
    ))
}

/// Maps the reported window length to a display kind. The two common plan
/// windows keep their canonical names so they render (and cache) exactly like
/// the other providers; anything else is preserved as reported.
fn window_kind(duration_mins: Option<u64>) -> WindowKind {
    match duration_mins {
        Some(300) => WindowKind::FiveHours,
        Some(10_080) => WindowKind::SevenDays,
        Some(mins) if mins > 0 && mins <= u32::MAX as u64 => WindowKind::Minutes(mins as u32),
        _ => WindowKind::Unknown,
    }
}

#[async_trait]
impl Agent for CodexAgent {
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    fn source_id(&self) -> &str {
        "codex"
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::OpenAI,
            label: "OpenAI (Codex)".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let (account, limits) = self.read_account_and_limits().await?;
        check_chatgpt_account(&account)?;

        let windows = parse_rate_limits(&limits);
        if windows.is_empty() {
            return Err(anyhow::anyhow!("codex returned no ChatGPT quota windows"));
        }

        Ok(SourceState::Quota(ProviderState {
            provider: Provider::OpenAI,
            label: label_for_account(&account),
            windows,
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn is_login_required_matches_only_login_errors() {
        assert!(is_login_required(&format!(
            "{LOGIN_REQUIRED_ERROR}: {LOGIN_HINT}"
        )));
        assert!(!is_login_required("codex app-server error: boom"));
        assert!(!is_login_required("request timed out"));
    }

    #[test]
    fn parse_rate_limits_reads_both_windows() {
        let limits = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 95, "windowDurationMins": 300, "resetsAt": 1788906517 },
                "secondary": { "usedPercent": 15, "windowDurationMins": 10080, "resetsAt": 1789493317 }
            }
        });

        let windows = parse_rate_limits(&limits);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].percentage().round(), 95.0);
        assert_eq!(
            windows[0].reset_at,
            DateTime::<Utc>::from_timestamp(1788906517, 0)
        );
        assert_eq!(windows[1].kind, WindowKind::SevenDays);
        assert_eq!(windows[1].percentage().round(), 15.0);
    }

    #[test]
    fn parse_rate_limits_prefers_codex_entry_without_double_counting() {
        let limits = json!({
            "rateLimits": {
                "limitId": "codex_other",
                "primary": { "usedPercent": 90, "windowDurationMins": 300 }
            },
            "rateLimitsByLimitId": {
                "codex_other": { "primary": { "usedPercent": 90, "windowDurationMins": 300 } },
                "codex": { "primary": { "usedPercent": 10, "windowDurationMins": 300 } }
            }
        });

        let windows = parse_rate_limits(&limits);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percentage().round(), 10.0);
    }

    #[test]
    fn parse_rate_limits_keeps_missing_windows_missing() {
        let limits = json!({
            "rateLimits": {
                "primary": { "usedPercent": 40, "windowDurationMins": 300 },
                "secondary": null
            }
        });
        let windows = parse_rate_limits(&limits);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percentage().round(), 40.0);

        assert!(parse_rate_limits(&json!({})).is_empty());
        assert!(parse_rate_limits(&json!({ "rateLimits": null })).is_empty());
        assert!(parse_rate_limits(&json!({ "rateLimits": { "primary": {} } })).is_empty());
    }

    #[test]
    fn parse_window_handles_fractional_percent_and_invalid_reset() {
        let w = parse_window(&json!({ "usedPercent": 12.5, "windowDurationMins": 300 })).unwrap();
        assert_eq!(w.percentage().round(), 13.0);
        assert_eq!(w.reset_at, None);

        let w = parse_window(&json!({ "usedPercent": 5, "resetsAt": 0 })).unwrap();
        assert_eq!(w.reset_at, None);
        assert_eq!(w.kind, WindowKind::Unknown);
    }

    #[test]
    fn window_kind_maps_known_and_custom_durations() {
        assert_eq!(window_kind(Some(300)), WindowKind::FiveHours);
        assert_eq!(window_kind(Some(10_080)), WindowKind::SevenDays);
        assert_eq!(window_kind(Some(90)), WindowKind::Minutes(90));
        assert_eq!(window_kind(None), WindowKind::Unknown);
        assert_eq!(window_kind(Some(0)), WindowKind::Unknown);
        assert_eq!(window_kind(Some(u64::MAX)), WindowKind::Unknown);
    }

    #[test]
    fn check_chatgpt_account_rejects_missing_and_api_key_logins() {
        let ok = json!({ "account": { "type": "chatgpt", "planType": "plus" } });
        assert!(check_chatgpt_account(&ok).is_ok());

        let api_key = json!({ "account": { "type": "apiKey" } });
        let err = check_chatgpt_account(&api_key).unwrap_err().to_string();
        assert!(is_login_required(&err), "error was: {err}");
        assert!(err.contains("API key"), "error was: {err}");

        for missing in [json!({}), json!({ "account": null })] {
            let err = check_chatgpt_account(&missing).unwrap_err().to_string();
            assert!(is_login_required(&err), "error was: {err}");
        }
    }

    #[test]
    fn label_reflects_reported_plan_only() {
        let account = json!({ "account": { "type": "chatgpt" }, "planType": "plus" });
        assert_eq!(label_for_account(&account), "OpenAI (Codex Plus)");

        let account = json!({ "account": { "type": "chatgpt" }, "planType": "go" });
        assert_eq!(label_for_account(&account), "OpenAI (Codex Go)");

        for unknown in [json!({}), json!({ "planType": "quorum" })] {
            assert_eq!(label_for_account(&unknown), "OpenAI (Codex)");
        }
    }

    #[tokio::test]
    async fn read_result_skips_notifications_and_reports_errors() {
        let stream = concat!(
            "{\"method\":\"thread/started\",\"params\":{}}\n",
            "not json\n",
            "{\"id\":9,\"result\":{\"ignored\":true}}\n",
            "{\"id\":1,\"result\":{\"account\":{\"type\":\"chatgpt\"}}}\n",
        );
        let mut lines = BufReader::new(stream.as_bytes()).lines();
        let result = read_result(&mut lines, 1).await.unwrap();
        assert_eq!(result["account"]["type"], "chatgpt");

        let stream = "{\"id\":2,\"error\":{\"code\":-32001,\"message\":\"Server overloaded\"}}\n";
        let mut lines = BufReader::new(stream.as_bytes()).lines();
        let err = read_result(&mut lines, 2).await.unwrap_err().to_string();
        assert!(err.contains("Server overloaded"), "error was: {err}");
        assert!(!is_login_required(&err), "error was: {err}");

        let mut lines = BufReader::new("".as_bytes()).lines();
        let err = read_result(&mut lines, 0).await.unwrap_err().to_string();
        assert!(err.contains("closed before answering"), "error was: {err}");
    }
}
