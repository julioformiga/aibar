use crate::agents::Agent;
use crate::config::http_timeout;
use crate::model::{LimitScope, LimitWindow, Provider, ProviderState, SourceState, WindowKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Prefix of the error shown when the auto-spawned `agy` requires Google
/// login. The poller matches on it to stop auto-retrying until the user
/// forces a refresh.
pub const LOGIN_REQUIRED_ERROR: &str = "agy login required";

/// Returns true when an agent error means the Gemini quota cannot be
/// fetched until the user runs `agy` interactively.
pub fn is_login_required(error: &str) -> bool {
    error.starts_with(LOGIN_REQUIRED_ERROR)
}

pub struct GeminiAgent {
    log_dir: Option<PathBuf>,
    client: reqwest::Client,
}

/// Sentinel error: the auto-spawned `agy` could not authenticate silently
/// and fell back to interactive OAuth, which cannot complete inside aibar.
#[derive(Debug)]
struct LoginRequired;

impl std::fmt::Display for LoginRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agy requires Google login")
    }
}

impl std::error::Error for LoginRequired {}

/// Kills the spawned `agy` process on drop, including when this future is
/// cancelled mid-await by the outer HTTP timeout in `run_poller` — plain
/// `tokio::process::Child` does not kill its child on drop.
struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

impl GeminiAgent {
    pub fn from_env() -> Option<Self> {
        let client = reqwest::Client::builder()
            .timeout(http_timeout())
            .build()
            .ok()?;

        let home = std::env::var_os("HOME")?;
        let log_dir = PathBuf::from(home).join(".gemini/antigravity-cli/log");
        let log_dir = if log_dir.exists() {
            Some(log_dir)
        } else {
            None
        };

        let has_api_key = std::env::var("GEMINI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .is_some();

        if !has_api_key && log_dir.is_none() {
            return None;
        }

        tracing::info!(
            has_api_key,
            log_dir_exists = log_dir.is_some(),
            "gemini from_env"
        );

        Some(Self { log_dir, client })
    }

    /// Returns all HTTP ports found across log files, newest file first,
    /// last occurrence within each file first.
    async fn candidate_ports_from_logs(&self) -> Vec<u16> {
        let mut ports = Vec::new();
        let Some(log_dir) = self.log_dir.as_ref() else {
            return ports;
        };
        for path in list_cli_logs_sorted_by_mtime(log_dir) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines().rev() {
                    if let Some(port) = extract_port(line) {
                        if !ports.contains(&port) {
                            ports.push(port);
                        }
                    }
                }
            }
        }
        ports
    }

    async fn candidate_ports_from_ss(&self) -> Vec<u16> {
        let mut ports = Vec::new();
        let Ok(output) = std::process::Command::new("ss").args(["-tulpn"]).output() else {
            return ports;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if !line.contains("\"agy\"") {
                continue;
            }
            if let Some(port) = extract_ss_port(line) {
                ports.push(port);
            }
        }
        ports
    }

    /// POST to the agy quota endpoint. Returns the parsed JSON if the
    /// response body contains `"groups"` (same check as the bash script's
    /// `grep -q '"groups"'`).
    async fn post_quota(&self, port: u16) -> Option<Value> {
        let url = quota_url(port);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await;
        match resp {
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                tracing::info!(port, %status, body_len = text.len(), "agy probe response");
                if text.contains("\"groups\"") {
                    serde_json::from_str::<Value>(&text).ok()
                } else {
                    tracing::info!(port, "response has no groups key");
                    None
                }
            }
            Err(e) => {
                tracing::info!(port, error = %e, "agy probe error");
                None
            }
        }
    }

    async fn find_port_and_fetch(&self) -> anyhow::Result<SourceState> {
        if let Some(resp) = self.try_existing_ports().await {
            return self.parse_response(resp);
        }

        match self.spawn_agy_and_fetch().await {
            Ok(resp) => self.parse_response(resp),
            Err(e) if e.downcast_ref::<LoginRequired>().is_some() => Err(anyhow::anyhow!(
                "{LOGIN_REQUIRED_ERROR}: run `agy` in another terminal to log in, then press R to retry"
            )),
            Err(e) => Err(anyhow::anyhow!(
                "Antigravity (agy) server not found and auto-start failed: {e}"
            )),
        }
    }

    async fn try_existing_ports(&self) -> Option<Value> {
        for port in self.candidate_ports_from_logs().await {
            if let Some(resp) = self.post_quota(port).await {
                return Some(resp);
            }
        }
        for port in self.candidate_ports_from_ss().await {
            if let Some(resp) = self.post_quota(port).await {
                return Some(resp);
            }
        }
        None
    }

    /// Spawns `agy --print`, waits for its language server port to appear
    /// in a new log file, polls the quota endpoint until it responds, then
    /// lets agy terminate naturally.
    async fn spawn_agy_and_fetch(&self) -> anyhow::Result<Value> {
        let agy_bin = which_agy().ok_or_else(|| anyhow::anyhow!("agy binary not found in PATH"))?;

        let known_logs = self.snapshot_log_files();

        tracing::info!("spawning agy to start language server");
        let child = tokio::process::Command::new(&agy_bin)
            .args(["--dangerously-skip-permissions", "--print", "ok"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn agy: {e}"))?;
        let mut guard = ChildGuard(child);

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            self.wait_for_new_port_and_fetch(&known_logs),
        )
        .await;

        let _ = guard.0.kill().await;
        let _ = guard.0.wait().await;

        if !matches!(result, Ok(Ok(_))) && self.new_logs_show_login_prompt(&known_logs) {
            tracing::info!("agy requires Google login (interactive OAuth)");
            return Err(anyhow::Error::new(LoginRequired));
        }

        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow::anyhow!("timed out waiting for agy language server")),
        }
    }

    /// Polls for a new log file (not in `known`), extracts the port, and
    /// retries the quota endpoint until it responds. Bails out early with
    /// [`LoginRequired`] when the new logs show agy fell back to interactive
    /// OAuth, so the login browser never gets a chance to open.
    async fn wait_for_new_port_and_fetch(&self, known: &[String]) -> anyhow::Result<Value> {
        let mut tried_ports: Vec<u16> = Vec::new();
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;

            for port in self.candidate_ports_from_logs().await {
                if tried_ports.contains(&port) {
                    continue;
                }
                let is_new = self
                    .port_source_log(port)
                    .map(|p| !known.iter().any(|k| k == &p))
                    .unwrap_or(true);
                if !is_new {
                    continue;
                }
                tried_ports.push(port);
                if let Some(resp) = self.post_quota(port).await {
                    return Ok(resp);
                }
                // Port appeared in log but server may not be ready yet;
                // keep retrying it in subsequent iterations.
            }

            // Also retry ports we've seen but haven't connected to yet.
            for port in &tried_ports {
                if let Some(resp) = self.post_quota(*port).await {
                    return Ok(resp);
                }
            }

            if new_logs_show_login_prompt(self.log_dir.as_deref(), known) {
                tracing::info!("agy fell back to interactive OAuth login");
                return Err(anyhow::Error::new(LoginRequired));
            }
        }
    }

    /// Returns true when any log file created after the `known` snapshot
    /// shows that agy needs an interactive Google login.
    fn new_logs_show_login_prompt(&self, known: &[String]) -> bool {
        new_logs_show_login_prompt(self.log_dir.as_deref(), known)
    }

    /// Returns the file paths of all current log files (as strings),
    /// newest (by mtime) first.
    fn snapshot_log_files(&self) -> Vec<String> {
        let Some(log_dir) = self.log_dir.as_ref() else {
            return vec![];
        };
        list_cli_logs_sorted_by_mtime(log_dir)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// Finds the log file that last mentions the given port.
    fn port_source_log(&self, port: u16) -> Option<String> {
        let log_dir = self.log_dir.as_ref()?;
        let needle = format!("port at {port} for HTTP");
        for path in list_cli_logs_sorted_by_mtime(log_dir) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if content.contains(&needle) {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }
        None
    }

    fn parse_response(&self, resp: Value) -> anyhow::Result<SourceState> {
        let groups = resp["response"]["groups"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing response.groups in agy response"))?;

        Ok(SourceState::Quota(ProviderState {
            provider: Provider::Gemini,
            label: "Gemini".into(),
            windows: parse_groups(groups),
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

fn parse_groups(groups: &[Value]) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    for group in groups {
        let display = group["displayName"].as_str().unwrap_or("");
        let scope = if display.eq_ignore_ascii_case("Gemini Models") {
            LimitScope::Standard
        } else {
            LimitScope::ThirdParty
        };
        if let Some(buckets) = group["buckets"].as_array() {
            for bucket in buckets {
                let window_str = bucket["window"].as_str().unwrap_or("");
                let kind = match window_str {
                    "5h" => WindowKind::FiveHours,
                    "weekly" => WindowKind::SevenDays,
                    _ => continue,
                };
                let remaining = bucket["remainingFraction"].as_f64().unwrap_or(1.0);
                let reset_at = bucket["resetTime"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                let used_fraction = (1.0 - remaining).max(0.0) as f32;
                windows.push(LimitWindow::from_fraction(
                    kind,
                    Some(scope),
                    used_fraction,
                    reset_at,
                ));
            }
        }
    }
    windows.sort_by_key(|w| match (w.scope, w.kind) {
        (Some(LimitScope::Standard), WindowKind::FiveHours) => 0,
        (Some(LimitScope::Standard), WindowKind::SevenDays) => 1,
        (Some(LimitScope::ThirdParty), WindowKind::FiveHours) => 2,
        (Some(LimitScope::ThirdParty), WindowKind::SevenDays) => 3,
        (None, WindowKind::FiveHours) => 0,
        (None, WindowKind::SevenDays) => 1,
        (_, WindowKind::Minutes(_) | WindowKind::Unknown) => 4,
    });
    windows
}

/// Lists `cli-*.log` files in `log_dir`, newest (by mtime) first.
fn list_cli_logs_sorted_by_mtime(log_dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return vec![];
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("cli-") && name.ends_with(".log") {
                Some((e.metadata().ok()?.modified().ok()?, e.path()))
            } else {
                None
            }
        })
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.into_iter().map(|(_, path)| path).collect()
}

fn quota_url(port: u16) -> String {
    format!(
        "http://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary"
    )
}

/// Returns true when any log file created after the `known` snapshot shows
/// that agy needs an interactive Google login.
fn new_logs_show_login_prompt(log_dir: Option<&std::path::Path>, known: &[String]) -> bool {
    let Some(log_dir) = log_dir else {
        return false;
    };
    for path in list_cli_logs_sorted_by_mtime(log_dir) {
        let path_str = path.to_string_lossy().to_string();
        if known.contains(&path_str) {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if login_prompt_in_content(&content) {
                return true;
            }
        }
    }
    false
}

/// Detects, from an agy log file, that the CLI could not authenticate
/// silently and needs an interactive Google login (which cannot happen
/// inside aibar). Auth markers come from agy's print-mode logger.
fn login_prompt_in_content(content: &str) -> bool {
    if content.contains("authenticated successfully")
        || content.contains("Print mode: authenticated as")
    {
        return false;
    }
    content.contains("Print mode: triggering interactive OAuth")
        || (content.contains("Print mode: silent auth failed")
            && !content.contains("Print mode: silent auth succeeded"))
}

fn which_agy() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join("agy");
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

fn extract_port(line: &str) -> Option<u16> {
    let marker = "Language server listening on random port at ";
    let idx = line.find(marker)?;
    let rest = &line[idx + marker.len()..];
    let end = rest.find(" for HTTP")?;
    let candidate = rest[..end].trim();
    let after = &rest[end + " for HTTP".len()..];
    if !after.is_empty() && !after.starts_with([' ', '\n', '\r', '\t', '(']) {
        return None;
    }
    candidate.parse().ok()
}

fn extract_ss_port(line: &str) -> Option<u16> {
    let marker = "127.0.0.1:";
    let idx = line.find(marker)?;
    let rest = &line[idx + marker.len()..];
    rest.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

#[async_trait]
impl Agent for GeminiAgent {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn source_id(&self) -> &str {
        "default"
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::Gemini,
            label: "Gemini".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        self.find_port_and_fetch().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_port_reads_log_line() {
        let line = "2026-08-14T10:00:00Z INFO Language server listening on random port at 54321 for HTTP requests";
        assert_eq!(extract_port(line), Some(54321));
    }

    #[test]
    fn extract_port_returns_none_without_marker() {
        assert_eq!(extract_port("just a regular log line"), None);
    }

    #[test]
    fn extract_port_rejects_trailing_garbage() {
        let line = "Language server listening on random port at 54321forHTTP";
        assert_eq!(extract_port(line), None);
    }

    #[test]
    fn extract_ss_port_reads_listening_socket() {
        let line = r#"tcp LISTEN 0 128 127.0.0.1:54321 0.0.0.0:* users:(("agy",pid=123,fd=9))"#;
        assert_eq!(extract_ss_port(line), Some(54321));
    }

    #[test]
    fn extract_ss_port_returns_none_without_marker() {
        assert_eq!(extract_ss_port("no address here"), None);
    }

    #[test]
    fn login_prompt_detects_interactive_oauth() {
        let content = "I0814 printmode.go:441] Print mode: silent auth failed\n\
                       I0814 printmode.go:443] Print mode: triggering interactive OAuth\n";
        assert!(login_prompt_in_content(content));
    }

    #[test]
    fn login_prompt_detects_silent_auth_failure() {
        let content = "I0814 printmode.go:440] Print mode: not authenticated, trying silent auth\n\
                       I0814 printmode.go:441] Print mode: silent auth failed\n";
        assert!(login_prompt_in_content(content));
    }

    #[test]
    fn login_prompt_ignores_successful_auth() {
        let content = "I0814 printmode.go:440] Print mode: not authenticated, trying silent auth\n\
                       I0814 server_oauth.go:194] OAuth: authenticated successfully as user@example.com\n\
                       I0814 printmode.go:442] Print mode: silent auth succeeded\n";
        assert!(!login_prompt_in_content(content));
    }

    #[test]
    fn login_prompt_ignores_regular_logs() {
        let content = "You are not logged into Antigravity.\n\
                       Language server listening on random port at 43755 for HTTPS (gRPC)\n";
        assert!(!login_prompt_in_content(content));
    }

    #[test]
    fn is_login_required_matches_error_prefix() {
        assert!(is_login_required(
            "agy login required: run `agy` in another terminal to log in, then press R to retry"
        ));
        assert!(!is_login_required(
            "Antigravity (agy) server not found and auto-start failed: timed out"
        ));
        assert!(!is_login_required("request timed out"));
    }

    #[test]
    fn new_logs_show_login_prompt_only_checks_new_files() {
        let dir = std::env::temp_dir().join(format!(
            "aibar-test-login-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("cli-old.log");
        let new = dir.join("cli-new.log");
        std::fs::write(&old, "authenticated successfully\n").unwrap();
        std::fs::write(&new, "Print mode: triggering interactive OAuth\n").unwrap();

        let old_known = vec![old.to_string_lossy().to_string()];
        assert!(new_logs_show_login_prompt(Some(&dir), &old_known));

        let all_known = vec![
            old.to_string_lossy().to_string(),
            new.to_string_lossy().to_string(),
        ];
        assert!(!new_logs_show_login_prompt(Some(&dir), &all_known));
        assert!(!new_logs_show_login_prompt(None, &old_known));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_groups_maps_standard_and_third_party_scopes() {
        let groups = json!([
            {
                "displayName": "Gemini Models",
                "buckets": [
                    { "window": "5h", "remainingFraction": 0.78, "resetTime": "2026-08-14T15:00:00Z" },
                    { "window": "weekly", "remainingFraction": 0.35 }
                ]
            },
            {
                "displayName": "Third Party Models",
                "buckets": [
                    { "window": "5h", "remainingFraction": 0.64 },
                    { "window": "weekly", "remainingFraction": 0.25 },
                    { "window": "unknown", "remainingFraction": 0.1 }
                ]
            }
        ]);
        let windows = parse_groups(groups.as_array().unwrap());

        assert_eq!(windows.len(), 4);
        assert_eq!(windows[0].scope, Some(LimitScope::Standard));
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].used, 220);
        assert!(windows[0].reset_at.is_some());

        assert_eq!(windows[1].scope, Some(LimitScope::Standard));
        assert_eq!(windows[1].kind, WindowKind::SevenDays);

        assert_eq!(windows[2].scope, Some(LimitScope::ThirdParty));
        assert_eq!(windows[2].kind, WindowKind::FiveHours);
        assert_eq!(windows[3].scope, Some(LimitScope::ThirdParty));
        assert_eq!(windows[3].kind, WindowKind::SevenDays);
    }

    #[test]
    fn parse_groups_ignores_groups_without_buckets() {
        let groups = json!([{ "displayName": "Gemini Models" }]);
        assert!(parse_groups(groups.as_array().unwrap()).is_empty());
    }
}
