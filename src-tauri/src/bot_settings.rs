use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSettingsView {
    pub telegram_bot_token_masked: String,
    pub has_telegram_bot_token: bool,
    pub telegram_allowed_user_id: String,
    pub codex_path: String,
    pub env_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSettingsInput {
    pub telegram_bot_token: Option<String>,
    pub telegram_allowed_user_id: String,
    pub codex_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotServiceStatus {
    pub configured: bool,
    pub running: bool,
    pub detail: String,
}

pub fn default_env_path() -> Result<PathBuf, String> {
    project_root()
        .map(|root| root.join(".runtime.env"))
        .ok_or_else(|| "无法定位项目目录".to_string())
}

pub fn read_settings(path: &Path) -> Result<BotSettingsView, String> {
    let values = read_env(path)?;
    let token = values
        .get("TELEGRAM_BOT_TOKEN")
        .cloned()
        .unwrap_or_default();
    Ok(BotSettingsView {
        telegram_bot_token_masked: crate::codex_provider::mask_secret(&token),
        has_telegram_bot_token: !token.trim().is_empty(),
        telegram_allowed_user_id: values
            .get("TELEGRAM_ALLOWED_USER_ID")
            .cloned()
            .unwrap_or_default(),
        codex_path: values
            .get("CODEX_PATH")
            .cloned()
            .unwrap_or_else(|| "codex".to_string()),
        env_path: path.to_string_lossy().to_string(),
    })
}

pub fn save_settings(path: &Path, input: BotSettingsInput) -> Result<BotSettingsView, String> {
    let mut values = read_env(path)?;
    if let Some(token) = input
        .telegram_bot_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        values.insert("TELEGRAM_BOT_TOKEN".to_string(), token.to_string());
    }
    values.insert(
        "TELEGRAM_ALLOWED_USER_ID".to_string(),
        input.telegram_allowed_user_id.trim().to_string(),
    );
    values.insert(
        "CODEX_PATH".to_string(),
        input.codex_path.trim().to_string(),
    );
    write_env(path, &values)?;
    read_settings(path)
}

pub fn service_status() -> BotServiceStatus {
    let label = "com.local.telegram-codex-bot";
    let output = Command::new("launchctl")
        .args(["print", &format!("gui/{}/{}", current_uid(), label)])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let detail = String::from_utf8_lossy(&output.stdout).to_string();
            let running = detail.contains("state = running");
            BotServiceStatus {
                configured: true,
                running,
                detail: first_non_empty_line(&detail)
                    .unwrap_or_else(|| "launchd 服务已配置".to_string()),
            }
        }
        Ok(output) => BotServiceStatus {
            configured: false,
            running: false,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(err) => BotServiceStatus {
            configured: false,
            running: false,
            detail: format!("无法读取 launchd 状态：{err}"),
        },
    }
}

pub fn restart_service() -> Result<BotServiceStatus, String> {
    ensure_service_loaded()?;
    let label = format!("gui/{}/com.local.telegram-codex-bot", current_uid());
    let output = Command::new("launchctl")
        .args(["kickstart", "-k", &label])
        .output()
        .map_err(|err| format!("执行 launchctl 失败：{err}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(service_status())
}

fn ensure_service_loaded() -> Result<(), String> {
    if service_status().configured {
        return Ok(());
    }
    let plist = dirs::home_dir()
        .map(|home| home.join("Library/LaunchAgents/com.local.telegram-codex-bot.plist"))
        .ok_or_else(|| "无法定位 launchd plist".to_string())?;
    if !plist.exists() {
        return Err(format!("launchd plist 不存在：{}", plist.display()));
    }
    let domain = format!("gui/{}", current_uid());
    let output = Command::new("launchctl")
        .args(["bootstrap", &domain, &plist.to_string_lossy()])
        .output()
        .map_err(|err| format!("执行 launchctl bootstrap 失败：{err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn read_env(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(path).map_err(|err| format!("读取 TG Bot 配置失败：{err}"))?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(key.trim().to_string(), unquote(value.trim()));
    }
    Ok(values)
}

fn write_env(path: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建配置目录失败：{err}"))?;
    }
    let keys = [
        "TELEGRAM_BOT_TOKEN",
        "TELEGRAM_ALLOWED_USER_ID",
        "CODEX_PATH",
    ];
    let mut lines = Vec::new();
    for key in keys {
        let value = values.get(key).cloned().unwrap_or_default();
        lines.push(format!("{key}={}", shell_quote(&value)));
    }
    for (key, value) in values {
        if !keys.contains(&key.as_str()) {
            lines.push(format!("{key}={}", shell_quote(value)));
        }
    }
    fs::write(path, format!("{}\n", lines.join("\n")))
        .map_err(|err| format!("写入 TG Bot 配置失败：{err}"))
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        value.to_string()
    }
}

fn shell_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn current_uid() -> String {
    std::env::var("UID").unwrap_or_else(|_| {
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "501".to_string())
    })
}

fn first_non_empty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn project_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_settings_preserves_existing_token_when_input_empty() {
        let dir = temp_dir("bot-settings");
        let path = dir.join(".runtime.env");
        fs::write(
            &path,
            "TELEGRAM_BOT_TOKEN=\"secret-token\"\nTELEGRAM_ALLOWED_USER_ID=\"1\"\n",
        )
        .expect("写入");

        let view = save_settings(
            &path,
            BotSettingsInput {
                telegram_bot_token: Some(String::new()),
                telegram_allowed_user_id: "2".to_string(),
                codex_path: "codex".to_string(),
            },
        )
        .expect("保存");

        assert!(view.has_telegram_bot_token);
        assert_eq!(view.telegram_allowed_user_id, "2");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("secret-token"));
        assert!(text.contains("CODEX_PATH=\"codex\""));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_settings_keeps_extra_env_values() {
        let dir = temp_dir("bot-settings-extra");
        let path = dir.join(".runtime.env");
        fs::write(
            &path,
            "TELEGRAM_BOT_TOKEN=\"secret-token\"\nCUSTOM_FLAG=\"1\"\n",
        )
        .expect("写入");

        save_settings(
            &path,
            BotSettingsInput {
                telegram_bot_token: None,
                telegram_allowed_user_id: "2".to_string(),
                codex_path: "codex".to_string(),
            },
        )
        .expect("保存");

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("CUSTOM_FLAG=\"1\""));
        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("codex-bot-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
