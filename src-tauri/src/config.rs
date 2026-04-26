use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub config_text: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub is_official: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub telegram_bot_token: String,
    #[serde(default)]
    pub telegram_allowed_user_id: String,
    #[serde(default = "default_codex_path")]
    pub codex_path: String,
    #[serde(default)]
    pub providers: Vec<CodexProvider>,
    #[serde(default)]
    pub active_provider_id: Option<String>,
}

fn default_codex_path() -> String {
    "codex".to_string()
}

pub fn default_config_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".codex-manager").join("config.json"))
        .ok_or_else(|| "无法定位用户目录".to_string())
}

pub fn read_config_at(path: &Path) -> Result<AppConfig, String> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text = fs::read_to_string(path).map_err(|err| format!("读取配置失败：{err}"))?;
    serde_json::from_str(&text).map_err(|err| format!("解析配置失败：{err}"))
}

pub fn write_config_at(path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建配置目录失败：{err}"))?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|err| format!("编码配置失败：{err}"))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|err| format!("写入临时文件失败：{err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("替换配置文件失败：{err}"))
}
