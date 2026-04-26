use crate::codex_provider::mask_secret;
use crate::config::{read_config_at, write_config_at};
use serde::{Deserialize, Serialize};
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
    crate::config::default_config_path()
}

pub fn read_settings(path: &Path) -> Result<BotSettingsView, String> {
    let config = read_config_at(path)?;
    Ok(BotSettingsView {
        telegram_bot_token_masked: mask_secret(&config.telegram_bot_token),
        has_telegram_bot_token: !config.telegram_bot_token.trim().is_empty(),
        telegram_allowed_user_id: config.telegram_allowed_user_id,
        codex_path: config.codex_path,
        env_path: path.to_string_lossy().to_string(),
    })
}

pub fn save_settings(path: &Path, input: BotSettingsInput) -> Result<BotSettingsView, String> {
    let mut config = read_config_at(path)?;
    if let Some(token) = input
        .telegram_bot_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.telegram_bot_token = token.to_string();
    }
    config.telegram_allowed_user_id = input.telegram_allowed_user_id.trim().to_string();
    config.codex_path = input.codex_path.trim().to_string();
    write_config_at(path, &config)?;
    read_settings(path)
}

pub fn service_status() -> BotServiceStatus {
    let config_complete = match default_env_path().and_then(|path| read_settings(&path)) {
        Ok(settings) => {
            settings.has_telegram_bot_token && !settings.telegram_allowed_user_id.is_empty()
        }
        Err(_) => false,
    };

    let label = "com.local.telegram-codex-bot";
    let output = Command::new("launchctl")
        .args(["print", &format!("gui/{}/{}", current_uid(), label)])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            let running = text.contains("state = running");
            if !config_complete {
                return BotServiceStatus {
                    configured: false,
                    running: false,
                    detail: "缺少 Bot Token 或用户 ID，请前往设置配置".to_string(),
                };
            }
            BotServiceStatus {
                configured: true,
                running,
                detail: if running {
                    "服务已配置并运行中".to_string()
                } else {
                    "服务已配置但未运行".to_string()
                },
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

#[allow(dead_code)]
fn first_non_empty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_settings_preserves_existing_token_when_input_empty() {
        let dir = temp_dir("bot-settings");
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"telegramBotToken":"secret-token","telegramAllowedUserId":"1"}"#,
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
        assert!(text.contains("codex"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_settings_keeps_other_config_values() {
        let dir = temp_dir("bot-settings-extra");
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"telegramBotToken":"secret-token","providers":[{"id":"demo","name":"Demo","baseUrl":"https://example.com/v1","model":"gpt-5.4"}]}"#,
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
        assert!(text.contains("secret-token"));
        assert!(text.contains("Demo"));
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
