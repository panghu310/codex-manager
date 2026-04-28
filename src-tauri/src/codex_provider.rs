use crate::config::{read_config_at, write_config_at, CodexProvider};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key_masked: String,
    pub has_api_key: bool,
    pub auth_text: String,
    pub rendered_auth_text: String,
    pub config_text: String,
    pub rendered_config_text: String,
    pub context_window_1m: bool,
    pub auto_compact_token_limit: Option<u64>,
    pub active: bool,
    pub is_official: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStore {
    #[serde(default)]
    pub active_id: Option<String>,
    #[serde(default)]
    pub providers: Vec<CodexProvider>,
}

impl ProviderStore {
    #[allow(dead_code)]
    fn empty() -> Self {
        Self {
            active_id: None,
            providers: Vec::new(),
        }
    }
}

pub fn list_provider_views(store_path: &Path) -> Result<Vec<CodexProviderView>, String> {
    let store = read_store(store_path)?;
    Ok(store
        .providers
        .iter()
        .map(|provider| to_view(provider, store.active_id.as_deref()))
        .collect())
}

pub fn save_provider(
    store_path: &Path,
    mut provider: CodexProvider,
) -> Result<CodexProviderView, String> {
    provider.id = normalize_id(&provider.id, &provider.name);

    let mut store = read_store(store_path)?;
    if let Some(existing) = store
        .providers
        .iter_mut()
        .find(|item| item.id == provider.id)
    {
        if provider.api_key.as_deref().unwrap_or("").trim().is_empty() {
            provider.api_key = existing.api_key.clone();
        }
        if provider
            .auth_text
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            provider.auth_text = existing.auth_text.clone();
        }
        provider.active = existing.active;
        validate_provider(&provider)?;
        *existing = provider.clone();
    } else {
        validate_provider(&provider)?;
        store.providers.push(provider.clone());
    }

    if store.active_id.is_none() {
        store.active_id = Some(provider.id.clone());
    }
    write_store(store_path, &store)?;
    Ok(to_view(&provider, store.active_id.as_deref()))
}

pub fn delete_provider(store_path: &Path, id: &str) -> Result<(), String> {
    let mut store = read_store(store_path)?;
    store.providers.retain(|provider| provider.id != id);
    if store.active_id.as_deref() == Some(id) {
        store.active_id = store.providers.first().map(|provider| provider.id.clone());
    }
    write_store(store_path, &store)
}

pub fn activate_provider(
    store_path: &Path,
    codex_dir: &Path,
    id: &str,
) -> Result<CodexProviderView, String> {
    let mut store = read_store(store_path)?;
    let provider = store
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .cloned()
        .ok_or_else(|| format!("找不到 Codex 供应商：{id}"))?;

    write_codex_live_config(codex_dir, &provider)?;
    store.active_id = Some(provider.id.clone());
    write_store(store_path, &store)?;
    Ok(to_view(&provider, store.active_id.as_deref()))
}

pub fn read_live_config(codex_dir: &Path) -> Result<String, String> {
    let path = codex_dir.join("config.toml");
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|err| format!("读取 Codex config.toml 失败：{err}"))
}

pub fn default_store_path() -> Result<PathBuf, String> {
    crate::config::default_config_path()
}

pub fn default_codex_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".codex"))
        .ok_or_else(|| "无法定位用户 HOME 目录".to_string())
}

pub fn render_provider_config(provider: &CodexProvider) -> String {
    if provider.is_official {
        return String::new();
    }

    if let Some(text) = provider
        .config_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        return text.to_string();
    }

    let key = provider_key(&provider.name);
    let mut text = format!(
        "model_provider = \"{key}\"\nmodel = \"{}\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n",
        escape_toml_string(&provider.model),
    );
    if provider.context_window_1m {
        text.push_str("model_context_window = 1000000\n");
        text.push_str(&format!(
            "model_auto_compact_token_limit = {}\n",
            provider.auto_compact_token_limit.unwrap_or(900_000)
        ));
    }
    text.push_str(&format!(
        "\n[model_providers.{key}]\nname = \"{key}\"\nbase_url = \"{}\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n",
        escape_toml_string(&provider.base_url),
    ));
    text
}

pub fn render_provider_auth_text(provider: &CodexProvider) -> String {
    if provider.is_official {
        return "{}\n".to_string();
    }

    if let Some(text) = provider
        .auth_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        return text.to_string();
    }

    let auth = match provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(api_key) => json!({ "OPENAI_API_KEY": api_key }),
        None => json!({}),
    };
    let mut text = serde_json::to_string_pretty(&auth).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.chars().count() <= 8 {
        return "*".repeat(value.chars().count());
    }
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{}{}",
        "*".repeat(value.chars().count().saturating_sub(4).max(8)),
        suffix
    )
}

fn read_store(path: &Path) -> Result<ProviderStore, String> {
    let config = read_config_at(path)?;
    Ok(ProviderStore {
        active_id: config.active_provider_id,
        providers: config.providers,
    })
}

fn write_store(path: &Path, store: &ProviderStore) -> Result<(), String> {
    let mut config = read_config_at(path)?;
    config.active_provider_id = store.active_id.clone();
    config.providers = store.providers.clone();
    write_config_at(path, &config)
}

fn write_codex_live_config(codex_dir: &Path, provider: &CodexProvider) -> Result<(), String> {
    fs::create_dir_all(codex_dir).map_err(|err| format!("创建 Codex 配置目录失败：{err}"))?;
    let config_text = render_provider_config(provider);
    if !config_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&config_text)
            .map_err(|err| format!("Codex config.toml 语法错误：{err}"))?;
    }

    let auth_text = render_provider_auth_text(provider);
    let auth: Value = serde_json::from_str(&auth_text)
        .map_err(|err| format!("Codex auth.json 语法错误：{err}"))?;
    if !auth.is_object() {
        return Err("Codex auth.json 必须是 JSON 对象".to_string());
    }
    let auth_bytes = serde_json::to_vec_pretty(&auth)
        .map_err(|err| format!("编码 Codex auth.json 失败：{err}"))?;
    atomic_write(&codex_dir.join("auth.json"), &auth_bytes)?;
    atomic_write(&codex_dir.join("config.toml"), config_text.as_bytes())
}

fn to_view(provider: &CodexProvider, active_id: Option<&str>) -> CodexProviderView {
    let api_key = provider.api_key.as_deref().unwrap_or("");
    CodexProviderView {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        model: provider.model.clone(),
        api_key_masked: mask_secret(api_key),
        has_api_key: !api_key.trim().is_empty(),
        auth_text: provider.auth_text.clone().unwrap_or_default(),
        rendered_auth_text: render_provider_auth_text(provider),
        config_text: provider.config_text.clone().unwrap_or_default(),
        rendered_config_text: render_provider_config(provider),
        context_window_1m: provider.context_window_1m,
        auto_compact_token_limit: provider.auto_compact_token_limit,
        active: active_id == Some(provider.id.as_str()),
        is_official: provider.is_official,
    }
}

fn validate_provider(provider: &CodexProvider) -> Result<(), String> {
    if provider.name.trim().is_empty() {
        return Err("供应商名称不能为空".to_string());
    }
    if provider.is_official {
        return Ok(());
    }
    if provider.base_url.trim().is_empty() {
        return Err("Base URL 不能为空".to_string());
    }
    if provider.model.trim().is_empty() {
        return Err("模型名称不能为空".to_string());
    }
    if let Some(text) = provider
        .config_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        toml::from_str::<toml::Table>(text)
            .map_err(|err| format!("Codex config.toml 语法错误：{err}"))?;
    }
    let auth_text = render_provider_auth_text(provider);
    let auth: Value = serde_json::from_str(&auth_text)
        .map_err(|err| format!("Codex auth.json 语法错误：{err}"))?;
    if !auth.is_object() {
        return Err("Codex auth.json 必须是 JSON 对象".to_string());
    }
    let api_key = auth
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if api_key.is_empty() {
        return Err("自定义供应商必须填写 OPENAI_API_KEY".to_string());
    }
    Ok(())
}

fn normalize_id(id: &str, name: &str) -> String {
    let source = if id.trim().is_empty() { name } else { id };
    provider_key(source)
}

fn provider_key(value: &str) -> String {
    let mut key = String::new();
    let mut last_underscore = false;
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch);
            last_underscore = false;
        } else if !last_underscore {
            key.push('_');
            last_underscore = true;
        }
    }
    let key = key.trim_matches('_').to_string();
    if key.is_empty() {
        "custom".to_string()
    } else {
        key
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| format!("写入临时文件失败：{err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("替换文件失败：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn render_provider_config_generates_codex_toml() {
        let provider = CodexProvider {
            id: "demo".to_string(),
            name: "Demo API".to_string(),
            base_url: "https://example.com/v1".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: Some("sk-demo".to_string()),
            auth_text: None,
            config_text: None,
            context_window_1m: false,
            auto_compact_token_limit: None,
            active: false,
            is_official: false,
        };

        let text = render_provider_config(&provider);

        assert!(text.contains("model_provider = \"demo_api\""));
        assert!(text.contains("base_url = \"https://example.com/v1\""));
        toml::from_str::<toml::Table>(&text).expect("有效 TOML");
    }

    #[test]
    fn render_provider_config_adds_context_window_fields() {
        let provider = CodexProvider {
            id: "demo".to_string(),
            name: "Demo API".to_string(),
            base_url: "https://example.com/v1".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: Some("sk-demo".to_string()),
            auth_text: None,
            config_text: None,
            context_window_1m: true,
            auto_compact_token_limit: Some(850_000),
            active: false,
            is_official: false,
        };

        let text = render_provider_config(&provider);

        assert!(text.contains("model_context_window = 1000000"));
        assert!(text.contains("model_auto_compact_token_limit = 850000"));
        toml::from_str::<toml::Table>(&text).expect("有效 TOML");
    }

    #[test]
    fn render_provider_auth_uses_auth_json_when_present() {
        let provider = CodexProvider {
            id: "demo".to_string(),
            name: "Demo API".to_string(),
            base_url: "https://example.com/v1".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: Some("sk-field".to_string()),
            auth_text: Some("{\"OPENAI_API_KEY\":\"sk-auth\"}".to_string()),
            config_text: None,
            context_window_1m: false,
            auto_compact_token_limit: None,
            active: false,
            is_official: false,
        };

        let text = render_provider_auth_text(&provider);
        let auth: serde_json::Value = serde_json::from_str(&text).expect("有效 JSON");

        assert_eq!(auth["OPENAI_API_KEY"], "sk-auth");
    }

    #[test]
    fn official_provider_writes_empty_auth_and_config() {
        let provider = CodexProvider {
            id: "openai".to_string(),
            name: "OpenAI 官方".to_string(),
            base_url: String::new(),
            model: String::new(),
            api_key: None,
            auth_text: Some("{\"OPENAI_API_KEY\":\"sk-ignored\"}".to_string()),
            config_text: Some("model = \"ignored\"".to_string()),
            context_window_1m: true,
            auto_compact_token_limit: Some(900_000),
            active: false,
            is_official: true,
        };

        assert_eq!(render_provider_auth_text(&provider), "{}\n");
        assert_eq!(render_provider_config(&provider), "");
    }

    #[test]
    fn save_provider_preserves_existing_key_when_new_key_is_empty() {
        let dir = temp_dir("provider-preserve");
        let store_path = dir.join("providers.json");
        let first = CodexProvider {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            base_url: "https://one.example/v1".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: Some("sk-old".to_string()),
            auth_text: None,
            config_text: None,
            context_window_1m: false,
            auto_compact_token_limit: None,
            active: false,
            is_official: false,
        };
        save_provider(&store_path, first).expect("第一次保存");

        save_provider(
            &store_path,
            CodexProvider {
                id: "demo".to_string(),
                name: "Demo".to_string(),
                base_url: "https://two.example/v1".to_string(),
                model: "gpt-5.4".to_string(),
                api_key: Some(String::new()),
                auth_text: None,
                config_text: None,
                context_window_1m: false,
                auto_compact_token_limit: None,
                active: false,
                is_official: false,
            },
        )
        .expect("第二次保存");

        let store = read_store(&store_path).expect("读取 store");
        assert_eq!(store.providers[0].api_key.as_deref(), Some("sk-old"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn activate_provider_writes_live_codex_files() {
        let dir = temp_dir("provider-activate");
        let store_path = dir.join("providers.json");
        let codex_dir = dir.join(".codex");
        let provider = CodexProvider {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            base_url: "https://example.com/v1".to_string(),
            model: "gpt-5.4".to_string(),
            api_key: Some("sk-demo".to_string()),
            auth_text: None,
            config_text: None,
            context_window_1m: true,
            auto_compact_token_limit: Some(900_000),
            active: false,
            is_official: false,
        };
        save_provider(&store_path, provider).expect("保存");

        activate_provider(&store_path, &codex_dir, "demo").expect("激活");

        let auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_dir.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(auth["OPENAI_API_KEY"], "sk-demo");
        let config_text = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
        assert!(config_text.contains("wire_api = \"responses\""));
        assert!(config_text.contains("model_context_window = 1000000"));
        assert!(config_text.contains("model_auto_compact_token_limit = 900000"));
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
