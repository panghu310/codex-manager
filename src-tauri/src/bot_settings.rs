use crate::codex_provider::mask_secret;
use crate::config::{read_config_at, write_config_at};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

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

pub struct BotManager {
    child: Mutex<Option<Child>>,
}

impl BotManager {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        if let Some(ref mut child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) => {
                    *guard = None;
                    false
                }
                Err(_) => {
                    *guard = None;
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn take_child(&self) -> Option<Child> {
        self.child.lock().unwrap().take()
    }

    pub fn set_child(&self, child: Child) {
        *self.child.lock().unwrap() = Some(child);
    }
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

pub fn service_status(manager: &BotManager) -> BotServiceStatus {
    let config_complete = match default_env_path().and_then(|path| read_settings(&path)) {
        Ok(settings) => {
            settings.has_telegram_bot_token && !settings.telegram_allowed_user_id.is_empty()
        }
        Err(_) => false,
    };

    if !config_complete {
        return BotServiceStatus {
            configured: false,
            running: false,
            detail: "缺少 Bot Token 或用户 ID，请前往设置配置".to_string(),
        };
    }

    let running = manager.is_running();
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

pub fn start_bot(manager: &BotManager) -> Result<BotServiceStatus, String> {
    stop_bot(manager)?;

    let config = read_config_at(&default_env_path()?)?;
    if config.telegram_bot_token.is_empty() || config.telegram_allowed_user_id.is_empty() {
        return Err("缺少 Bot Token 或用户 ID".to_string());
    }

    let bot_bin = resolve_bot_binary()?;

    let mut command = Command::new(&bot_bin);
    command
        .env("TELEGRAM_BOT_TOKEN", &config.telegram_bot_token)
        .env("TELEGRAM_ALLOWED_USER_ID", &config.telegram_allowed_user_id)
        .env("CODEX_PATH", &config.codex_path)
        .env("CODEX_BOT_DROP_PENDING_UPDATES", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|err| format!("启动 bot 失败：{err}"))?;

    manager.set_child(child);
    Ok(service_status(manager))
}

pub fn stop_bot(manager: &BotManager) -> Result<(), String> {
    if let Some(mut child) = manager.take_child() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

pub fn restart_bot(manager: &BotManager) -> Result<BotServiceStatus, String> {
    start_bot(manager)
}

fn resolve_bot_binary() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join("telegram-codex-bot");
            if sidecar.exists() {
                return Ok(sidecar);
            }
        }
    }

    let exe = std::env::current_exe().map_err(|err| format!("获取当前路径失败：{err}"))?;
    let exe_dir = exe.parent().ok_or("无法定位可执行文件目录")?;
    let target_dir = exe_dir.parent().ok_or("无法定位 target 目录")?;
    let project_root = target_dir.parent().ok_or("无法定位项目根目录")?;

    let release = project_root.join("target/release/telegram-codex-bot");
    if release.exists() {
        return Ok(release);
    }

    let debug = project_root.join("target/debug/telegram-codex-bot");
    if debug.exists() {
        return Ok(debug);
    }

    Err("找不到 telegram-codex-bot 二进制文件".to_string())
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
