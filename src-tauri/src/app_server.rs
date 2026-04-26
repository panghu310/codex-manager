use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppServerThread {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default, rename = "path")]
    pub rollout_path: Option<String>,
    #[serde(default)]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<i64>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_thread_status")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppServerThreadRead {
    pub thread: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppServerTurnResult {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub output: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppServerTurnHandle {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadStatusChange {
    pub thread_id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct PersistentAppServerClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    events: broadcast::Sender<Value>,
    child: Arc<Mutex<Child>>,
}

pub fn request(method: impl Into<String>, params: Option<Value>) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0",
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        method: method.into(),
        params,
    }
}

pub fn notification(method: impl Into<String>, params: Option<Value>) -> Value {
    let mut value = json!({ "method": method.into() });
    if let Some(params) = params {
        value["params"] = params;
    }
    value
}

pub fn initialize_request() -> RpcRequest {
    request(
        "initialize",
        Some(json!({
            "clientInfo": {
                "name": "codex-manager",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
    )
}

pub fn initialized_notification() -> Value {
    notification("initialized", Some(json!({})))
}

pub fn thread_list_request(limit: usize) -> RpcRequest {
    request(
        "thread/list",
        Some(json!({
            "limit": normalize_limit(limit),
            "sortKey": "updated_at",
            "sortDirection": "desc"
        })),
    )
}

pub fn thread_read_request(thread_id: &str, include_turns: bool) -> RpcRequest {
    request(
        "thread/read",
        Some(json!({
            "threadId": thread_id,
            "includeTurns": include_turns
        })),
    )
}

pub fn thread_start_request(cwd: Option<&str>) -> RpcRequest {
    let params = cwd
        .filter(|value| !value.trim().is_empty())
        .map(|cwd| json!({ "cwd": cwd }))
        .unwrap_or_else(|| json!({}));
    request("thread/start", Some(params))
}

pub fn turn_start_request(thread_id: &str, prompt: &str, cwd: Option<&str>) -> RpcRequest {
    let mut params = json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": prompt }]
    });
    if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
        params["cwd"] = json!(cwd);
    }
    request("turn/start", Some(params))
}

pub fn turn_interrupt_request(thread_id: &str, turn_id: &str) -> RpcRequest {
    request(
        "turn/interrupt",
        Some(json!({
            "threadId": thread_id,
            "turnId": turn_id
        })),
    )
}

pub fn turn_steer_request(thread_id: &str, turn_id: &str, prompt: &str) -> RpcRequest {
    request(
        "turn/steer",
        Some(json!({
            "threadId": thread_id,
            "expectedTurnId": turn_id,
            "input": [{ "type": "text", "text": prompt }]
        })),
    )
}

pub fn thread_rollback_request(thread_id: &str, num_turns: usize) -> RpcRequest {
    request(
        "thread/rollback",
        Some(json!({
            "threadId": thread_id,
            "numTurns": num_turns.max(1)
        })),
    )
}

pub fn thread_turns_list_request(
    thread_id: &str,
    cursor: Option<&str>,
    limit: usize,
    sort_direction: &str,
) -> RpcRequest {
    let mut params = json!({
        "threadId": thread_id,
        "limit": normalize_limit(limit),
        "sortDirection": sort_direction
    });
    if let Some(cursor) = cursor.map(str::trim).filter(|value| !value.is_empty()) {
        params["cursor"] = json!(cursor);
    }
    request("thread/turns/list", Some(params))
}

pub fn thread_resume_request(thread_id: &str) -> RpcRequest {
    request(
        "thread/resume",
        Some(json!({
            "threadId": thread_id
        })),
    )
}

pub fn thread_archive_request(thread_id: &str) -> RpcRequest {
    request(
        "thread/archive",
        Some(json!({
            "threadId": thread_id
        })),
    )
}

pub fn standalone_cwd() -> Result<PathBuf, String> {
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let dir_name = format!(
        "chat-{}-{}",
        now.format("%H%M%S%3f"),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    dirs::document_dir()
        .map(|dir| dir.join("Codex").join(date).join(dir_name))
        .ok_or_else(|| "无法定位文稿目录".to_string())
}

pub async fn list_threads(codex_path: &str, limit: usize) -> Result<Vec<AppServerThread>, String> {
    let result = call_once(
        codex_path,
        vec![initialize_request(), thread_list_request(limit)],
    )
    .await?;
    parse_thread_list_result(result).map(normalize_thread_list)
}

pub async fn read_thread(
    codex_path: &str,
    thread_id: &str,
    include_turns: bool,
) -> Result<AppServerThreadRead, String> {
    let result = call_once(
        codex_path,
        vec![
            initialize_request(),
            thread_read_request(thread_id, include_turns),
        ],
    )
    .await?;
    parse_thread_read_result(result)
}

pub async fn archive_thread(codex_path: &str, thread_id: &str) -> Result<(), String> {
    call_once(
        codex_path,
        vec![initialize_request(), thread_archive_request(thread_id)],
    )
    .await?;
    Ok(())
}

pub async fn rollback_thread(
    codex_path: &str,
    thread_id: &str,
    num_turns: usize,
) -> Result<AppServerThreadRead, String> {
    let result = call_once(
        codex_path,
        vec![
            initialize_request(),
            thread_rollback_request(thread_id, num_turns),
        ],
    )
    .await?;
    parse_thread_read_result(result)
}

pub async fn list_thread_turns(
    codex_path: &str,
    thread_id: &str,
    cursor: Option<&str>,
    limit: usize,
    sort_direction: &str,
) -> Result<Value, String> {
    call_once(
        codex_path,
        vec![
            initialize_request(),
            thread_turns_list_request(thread_id, cursor, limit, sort_direction),
        ],
    )
    .await
}

pub async fn list_thread_turns_compatible(
    codex_path: &str,
    thread_id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value, String> {
    match list_thread_turns(codex_path, thread_id, cursor, limit, "desc").await {
        Ok(result) => Ok(result),
        Err(err) if is_unsupported_thread_turns_list_error(&err) => {
            let read = read_thread(codex_path, thread_id, true).await?;
            Ok(thread_read_to_turns_list_result(read.thread, limit))
        }
        Err(err) => Err(err),
    }
}

pub async fn run_turn(
    codex_path: &str,
    thread_id: Option<&str>,
    cwd: Option<&str>,
    prompt: &str,
) -> Result<AppServerTurnResult, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_string());
    }

    let owned_cwd = standalone_cwd_for_new_thread(thread_id, cwd)?;
    let cwd = owned_cwd.as_deref().or(cwd);
    let process_cwd = process_cwd(cwd)?;
    let effective_cwd = effective_cwd(cwd, &process_cwd)?;
    let mut conn = AppServerConnection::start(codex_path, &process_cwd).await?;
    conn.initialize().await?;
    let thread_id = match thread_id {
        Some(id) if !id.trim().is_empty() => {
            let trimmed = id.trim();
            match conn.request(thread_resume_request(trimmed)).await {
                Ok(result) => result
                    .get("thread")
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(trimmed)
                    .to_string(),
                Err(err) if is_unsupported_thread_resume_error(&err) => trimmed.to_string(),
                Err(err) => return Err(err),
            }
        }
        _ => {
            let result = conn
                .request(thread_start_request(Some(&effective_cwd)))
                .await?;
            result
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("thread/start 响应缺少 thread.id：{result}"))?
                .to_string()
        }
    };

    let result = conn
        .request(turn_start_request(&thread_id, prompt, Some(&effective_cwd)))
        .await?;
    let turn_id = result
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let output = conn.read_turn_output().await?;
    Ok(AppServerTurnResult {
        thread_id,
        turn_id,
        output,
        completed: true,
    })
}

pub async fn run_turn_interruptible(
    codex_path: &str,
    thread_id: Option<&str>,
    cwd: Option<&str>,
    prompt: &str,
    started: oneshot::Sender<AppServerTurnHandle>,
    interrupt: oneshot::Receiver<oneshot::Sender<Result<(), String>>>,
) -> Result<AppServerTurnResult, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_string());
    }

    let owned_cwd = standalone_cwd_for_new_thread(thread_id, cwd)?;
    let cwd = owned_cwd.as_deref().or(cwd);
    let process_cwd = process_cwd(cwd)?;
    let effective_cwd = effective_cwd(cwd, &process_cwd)?;
    let mut conn = AppServerConnection::start(codex_path, &process_cwd).await?;
    conn.initialize().await?;
    let thread_id = match thread_id {
        Some(id) if !id.trim().is_empty() => {
            let trimmed = id.trim();
            match conn.request(thread_resume_request(trimmed)).await {
                Ok(result) => result
                    .get("thread")
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(trimmed)
                    .to_string(),
                Err(err) if is_unsupported_thread_resume_error(&err) => trimmed.to_string(),
                Err(err) => return Err(err),
            }
        }
        _ => {
            let result = conn
                .request(thread_start_request(Some(&effective_cwd)))
                .await?;
            result
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("thread/start 响应缺少 thread.id：{result}"))?
                .to_string()
        }
    };

    let result = conn
        .request(turn_start_request(&thread_id, prompt, Some(&effective_cwd)))
        .await?;
    let turn_id = result
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("turn/start 响应缺少 turn.id：{result}"))?;
    let _ = started.send(AppServerTurnHandle {
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
    });

    let turn_output = conn
        .read_turn_output_or_interrupt(&thread_id, &turn_id, interrupt)
        .await?;
    Ok(AppServerTurnResult {
        thread_id,
        turn_id: Some(turn_id),
        output: turn_output.output,
        completed: turn_output.completed,
    })
}

pub async fn run_turn_interruptible_persistent(
    client: Arc<PersistentAppServerClient>,
    thread_id: Option<&str>,
    cwd: Option<&str>,
    prompt: &str,
    started: oneshot::Sender<AppServerTurnHandle>,
    interrupt: oneshot::Receiver<oneshot::Sender<Result<(), String>>>,
) -> Result<AppServerTurnResult, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_string());
    }

    let owned_cwd = standalone_cwd_for_new_thread(thread_id, cwd)?;
    if let Some(cwd) = owned_cwd.as_deref() {
        fs::create_dir_all(cwd).map_err(|err| format!("创建 standalone 工作目录失败：{err}"))?;
    }
    let cwd = owned_cwd.as_deref().or(cwd);
    let fallback_cwd = process_cwd(cwd)?;
    let effective_cwd = effective_cwd(cwd, &fallback_cwd)?;

    let thread_id = match thread_id {
        Some(id) if !id.trim().is_empty() => {
            let trimmed = id.trim();
            match client.request(thread_resume_request(trimmed)).await {
                Ok(result) => result
                    .get("thread")
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(trimmed)
                    .to_string(),
                Err(err) if is_unsupported_thread_resume_error(&err) => trimmed.to_string(),
                Err(err) => return Err(err),
            }
        }
        _ => {
            let result = client
                .request(thread_start_request(Some(&effective_cwd)))
                .await?;
            result
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("thread/start 响应缺少 thread.id：{result}"))?
                .to_string()
        }
    };

    let mut events = client.subscribe();
    let result = client
        .request(turn_start_request(&thread_id, prompt, Some(&effective_cwd)))
        .await?;
    let turn_id = result
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("turn/start 响应缺少 turn.id：{result}"))?;
    let _ = started.send(AppServerTurnHandle {
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
    });

    let turn_output = read_persistent_turn_output_or_interrupt(
        client,
        &mut events,
        &thread_id,
        &turn_id,
        interrupt,
    )
    .await?;
    Ok(AppServerTurnResult {
        thread_id,
        turn_id: Some(turn_id),
        output: turn_output.output,
        completed: turn_output.completed,
    })
}

async fn read_persistent_turn_output_or_interrupt(
    client: Arc<PersistentAppServerClient>,
    events: &mut broadcast::Receiver<Value>,
    thread_id: &str,
    turn_id: &str,
    interrupt: oneshot::Receiver<oneshot::Sender<Result<(), String>>>,
) -> Result<TurnOutput, String> {
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);
    tokio::pin!(interrupt);
    let mut output = String::new();

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(value) => {
                        if !event_matches_turn(&value, thread_id, turn_id) {
                            continue;
                        }
                        if let Some(delta) = agent_delta(&value) {
                            output.push_str(delta);
                        }
                        if value.get("method").and_then(Value::as_str) == Some("turn/completed") {
                            return Ok(TurnOutput { output, completed: true });
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Err("app-server 事件通道已关闭".to_string()),
                }
            }
            ack = &mut interrupt => {
                let result = client.request(turn_interrupt_request(thread_id, turn_id)).await.map(|_| ());
                if let Ok(ack) = ack {
                    let _ = ack.send(result.clone());
                }
                result?;
                return Ok(TurnOutput { output, completed: false });
            }
            _ = &mut deadline => {
                return Err("等待 Codex turn 完成超时".to_string());
            }
        }
    }
}

fn spawn_codex_command(codex_path: &str, process_cwd: &Path) -> Result<Child, String> {
    let mut cmd = Command::new(codex_path);
    cmd.arg("app-server")
        .current_dir(process_cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Ok(current_path) = std::env::var("PATH") {
        let mut extra = vec!["/opt/homebrew/bin".to_string(), "/usr/local/bin".to_string()];
        if let Some(home) = dirs::home_dir() {
            extra.push(home.join(".local/bin").to_string_lossy().to_string());
        }
        let new_path = format!("{}:{}", extra.join(":"), current_path);
        cmd.env("PATH", new_path);
    }

    cmd.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            format!(
                "未找到 codex 命令（{}）。请在「设置」中配置 Codex 路径，或确保 codex 已安装且可被访问。",
                codex_path
            )
        } else {
            format!("启动 codex app-server 失败：{err}")
        }
    })
}

impl PersistentAppServerClient {
    pub async fn start(codex_path: &str, cwd: Option<&str>) -> Result<Self, String> {
        let process_cwd = process_cwd(cwd)?;
        let mut child = spawn_codex_command(codex_path, &process_cwd)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法打开 app-server stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法打开 app-server stdout".to_string())?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(256);
        let client = Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending: pending.clone(),
            events: events.clone(),
            child: Arc::new(Mutex::new(child)),
        };
        tokio::spawn(read_persistent_app_server(stdout, pending, events));
        client.initialize().await?;
        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub async fn initialize(&self) -> Result<(), String> {
        self.request(initialize_request()).await?;
        self.send_notification(initialized_notification()).await
    }

    pub async fn request(&self, rpc: RpcRequest) -> Result<Value, String> {
        let wanted_id = rpc.id;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(wanted_id, tx);
        if let Err(err) = self
            .write_json(
                serde_json::to_value(rpc).map_err(|err| format!("编码 JSON-RPC 失败：{err}"))?,
            )
            .await
        {
            self.pending.lock().await.remove(&wanted_id);
            return Err(err);
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("app-server 响应通道已关闭".to_string()),
            Err(_) => {
                self.pending.lock().await.remove(&wanted_id);
                Err("等待 app-server 响应超时".to_string())
            }
        }
    }

    pub async fn send_notification(&self, value: Value) -> Result<(), String> {
        self.write_json(value).await
    }

    pub async fn steer_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        prompt: &str,
    ) -> Result<(), String> {
        self.request(turn_steer_request(thread_id, turn_id, prompt))
            .await
            .map(|_| ())
    }

    async fn write_json(&self, value: Value) -> Result<(), String> {
        let line =
            serde_json::to_string(&value).map_err(|err| format!("编码 JSON-RPC 失败：{err}"))?;
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| format!("写入 app-server 请求失败：{err}"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|err| format!("写入 app-server 请求失败：{err}"))?;
        stdin
            .flush()
            .await
            .map_err(|err| format!("刷新 app-server stdin 失败：{err}"))
    }
}

impl Drop for PersistentAppServerClient {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

async fn read_persistent_app_server(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    events: broadcast::Sender<Value>,
) {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        let line = match reader.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                fail_pending(&pending, "app-server 提前退出").await;
                return;
            }
            Err(err) => {
                fail_pending(&pending, &format!("读取 app-server 响应失败：{err}")).await;
                return;
            }
        };
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                let _ = events.send(json!({
                    "method": "app-server/parse-error",
                    "params": { "error": err.to_string(), "line": line }
                }));
                continue;
            }
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let result = if let Some(error) = value.get("error") {
                Err(format!("app-server 返回错误：{error}"))
            } else {
                value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "app-server 响应缺少 result".to_string())
            };
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(result);
            }
            continue;
        }
        let _ = events.send(value);
    }
}

async fn fail_pending(
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    message: &str,
) {
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(message.to_string()));
    }
}

async fn call_once(codex_path: &str, requests: Vec<RpcRequest>) -> Result<Value, String> {
    let process_cwd = standalone_cwd()?;
    fs::create_dir_all(&process_cwd)
        .map_err(|err| format!("创建 standalone 工作目录失败：{err}"))?;
    let mut conn = AppServerConnection::start(codex_path, &process_cwd).await?;

    let mut final_result = None;
    let last_index = requests.len().saturating_sub(1);

    for (index, rpc) in requests.into_iter().enumerate() {
        let is_initialize = rpc.method == "initialize";
        let result = conn.request(rpc).await?;
        if is_initialize {
            conn.send_notification(initialized_notification()).await?;
        }
        if index == last_index {
            final_result = Some(result);
        }
    }
    final_result.ok_or_else(|| "缺少 app-server 响应".to_string())
}

struct AppServerConnection {
    child: Child,
    stdin: ChildStdin,
    reader: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl AppServerConnection {
    async fn start(codex_path: &str, process_cwd: &Path) -> Result<Self, String> {
        let mut child = spawn_codex_command(codex_path, process_cwd)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法打开 app-server stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法打开 app-server stdout".to_string())?;

        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout).lines(),
        })
    }

    async fn initialize(&mut self) -> Result<(), String> {
        self.request(initialize_request()).await?;
        self.send_notification(initialized_notification()).await
    }

    async fn request(&mut self, rpc: RpcRequest) -> Result<Value, String> {
        let wanted_id = rpc.id;
        self.write_json(
            serde_json::to_value(rpc).map_err(|err| format!("编码 JSON-RPC 失败：{err}"))?,
        )
        .await?;
        self.read_response(wanted_id, Duration::from_secs(8)).await
    }

    async fn send_notification(&mut self, value: Value) -> Result<(), String> {
        self.write_json(value).await
    }

    async fn write_json(&mut self, value: Value) -> Result<(), String> {
        let line =
            serde_json::to_string(&value).map_err(|err| format!("编码 JSON-RPC 失败：{err}"))?;
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| format!("写入 app-server 请求失败：{err}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|err| format!("写入 app-server 请求失败：{err}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|err| format!("刷新 app-server stdin 失败：{err}"))
    }

    async fn read_response(&mut self, wanted_id: u64, timeout: Duration) -> Result<Value, String> {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                line = self.reader.next_line() => {
                    let line = line.map_err(|err| format!("读取 app-server 响应失败：{err}"))?;
                    let Some(line) = line else {
                        return Err("app-server 提前退出".to_string());
                    };
                    let value: Value = serde_json::from_str(&line)
                        .map_err(|err| format!("解析 app-server JSON 失败：{err}; line={line}"))?;
                    if value.get("id").and_then(Value::as_u64) != Some(wanted_id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(format!("app-server 返回错误：{error}"));
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| "app-server 响应缺少 result".to_string());
                }
                _ = &mut deadline => {
                    return Err("等待 app-server 响应超时".to_string());
                }
            }
        }
    }

    async fn read_turn_output(&mut self) -> Result<String, String> {
        let deadline = tokio::time::sleep(Duration::from_secs(120));
        tokio::pin!(deadline);
        let mut output = String::new();

        loop {
            tokio::select! {
                line = self.reader.next_line() => {
                    let line = line.map_err(|err| format!("读取 app-server 通知失败：{err}"))?;
                    let Some(line) = line else {
                        return Err("app-server 提前退出".to_string());
                    };
                    let value: Value = serde_json::from_str(&line)
                        .map_err(|err| format!("解析 app-server 通知失败：{err}; line={line}"))?;
                    if let Some(delta) = agent_delta(&value) {
                        output.push_str(delta);
                    }
                    if value.get("method").and_then(Value::as_str) == Some("turn/completed") {
                        return Ok(output);
                    }
                }
                _ = &mut deadline => {
                    return Err("等待 Codex turn 完成超时".to_string());
                }
            }
        }
    }

    async fn read_turn_output_or_interrupt(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        interrupt: oneshot::Receiver<oneshot::Sender<Result<(), String>>>,
    ) -> Result<TurnOutput, String> {
        let deadline = tokio::time::sleep(Duration::from_secs(120));
        tokio::pin!(deadline);
        tokio::pin!(interrupt);
        let mut output = String::new();

        loop {
            tokio::select! {
                line = self.reader.next_line() => {
                    let line = line.map_err(|err| format!("读取 app-server 通知失败：{err}"))?;
                    let Some(line) = line else {
                        return Err("app-server 提前退出".to_string());
                    };
                    let value: Value = serde_json::from_str(&line)
                        .map_err(|err| format!("解析 app-server 通知失败：{err}; line={line}"))?;
                    if let Some(delta) = agent_delta(&value) {
                        output.push_str(delta);
                    }
                    if value.get("method").and_then(Value::as_str) == Some("turn/completed") {
                        return Ok(TurnOutput { output, completed: true });
                    }
                }
                ack = &mut interrupt => {
                    let result = self.request(turn_interrupt_request(thread_id, turn_id)).await.map(|_| ());
                    if let Ok(ack) = ack {
                        let _ = ack.send(result.clone());
                    }
                    result?;
                    return Ok(TurnOutput { output, completed: false });
                }
                _ = &mut deadline => {
                    return Err("等待 Codex turn 完成超时".to_string());
                }
            }
        }
    }
}

struct TurnOutput {
    output: String,
    completed: bool,
}

fn process_cwd(cwd: Option<&str>) -> Result<PathBuf, String> {
    if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
        return Ok(PathBuf::from(cwd));
    }
    let standalone = standalone_cwd()?;
    fs::create_dir_all(&standalone)
        .map_err(|err| format!("创建 standalone 工作目录失败：{err}"))?;
    Ok(standalone)
}

fn standalone_cwd_for_new_thread(
    thread_id: Option<&str>,
    cwd: Option<&str>,
) -> Result<Option<String>, String> {
    if thread_id.is_some_and(|id| !id.trim().is_empty())
        || cwd.is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(None);
    }
    let standalone = standalone_cwd()?;
    standalone
        .to_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| format!("cwd 不是有效 UTF-8：{}", standalone.display()))
}

fn effective_cwd(cwd: Option<&str>, process_cwd: &Path) -> Result<String, String> {
    if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(cwd.to_string());
    }
    process_cwd
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("cwd 不是有效 UTF-8：{}", process_cwd.display()))
}

impl Drop for AppServerConnection {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub fn parse_thread_list_result(result: Value) -> Result<Vec<AppServerThread>, String> {
    if let Some(items) = result
        .get("threads")
        .or_else(|| result.get("items"))
        .or_else(|| result.get("data"))
    {
        return serde_json::from_value(items.clone())
            .map_err(|err| format!("解析 thread/list 结果失败：{err}"));
    }
    if result.is_array() {
        return serde_json::from_value(result)
            .map_err(|err| format!("解析 thread/list 结果失败：{err}"));
    }
    Err(format!("未知 thread/list 响应结构：{result}"))
}

pub fn parse_thread_turns_list_result(result: Value) -> Result<Vec<Value>, String> {
    if let Some(turns) = result
        .get("turns")
        .or_else(|| result.get("items"))
        .or_else(|| result.get("data"))
    {
        return turns
            .as_array()
            .cloned()
            .ok_or_else(|| format!("thread/turns/list 响应 turns 不是数组：{result}"));
    }
    if result.is_array() {
        return result
            .as_array()
            .cloned()
            .ok_or_else(|| format!("thread/turns/list 响应不是数组：{result}"));
    }
    Err(format!("未知 thread/turns/list 响应结构：{result}"))
}

pub fn parse_thread_status_change(value: &Value) -> Option<ThreadStatusChange> {
    if value.get("method").and_then(Value::as_str) != Some("thread/status/changed") {
        return None;
    }
    let params = value.get("params")?;
    let thread_id = params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
        .or_else(|| params.get("thread").and_then(|thread| thread.get("id")))
        .and_then(Value::as_str)?
        .to_string();
    let status = status_value_to_string(
        params
            .get("status")
            .or_else(|| params.get("thread").and_then(|thread| thread.get("status")))?,
    )?;
    Some(ThreadStatusChange { thread_id, status })
}

pub fn normalize_thread_cwd(cwd: Option<String>) -> Option<String> {
    let cwd = cwd.map(|value| value.trim().trim_end_matches('/').to_string())?;
    if cwd.is_empty() || is_manager_standalone_cwd(&cwd) || is_codex_desktop_chat_cwd(&cwd) {
        None
    } else {
        Some(cwd)
    }
}

fn normalize_thread_list(threads: Vec<AppServerThread>) -> Vec<AppServerThread> {
    threads
        .into_iter()
        .map(|mut thread| {
            thread.cwd = normalize_thread_cwd(thread.cwd);
            thread
        })
        .collect()
}

pub fn parse_thread_read_result(result: Value) -> Result<AppServerThreadRead, String> {
    let thread = result
        .get("thread")
        .cloned()
        .ok_or_else(|| format!("thread/read 响应缺少 thread：{result}"))?;
    Ok(AppServerThreadRead { thread })
}

fn agent_delta(value: &Value) -> Option<&str> {
    if value.get("method").and_then(Value::as_str) != Some("item/agentMessage/delta") {
        return None;
    }
    value
        .get("params")
        .and_then(|params| params.get("delta").or_else(|| params.get("text")))
        .and_then(Value::as_str)
}

fn event_matches_turn(value: &Value, thread_id: &str, turn_id: &str) -> bool {
    let Some(params) = value.get("params") else {
        return true;
    };
    let event_thread_id = params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
        .and_then(Value::as_str);
    let event_turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .or_else(|| params.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str);
    event_thread_id.is_none_or(|value| value == thread_id)
        && event_turn_id.is_none_or(|value| value == turn_id)
}

pub fn turn_completed(value: &Value) -> Option<(String, String)> {
    if value.get("method").and_then(Value::as_str) != Some("turn/completed") {
        return None;
    }
    let params = value.get("params")?;
    let thread_id = params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
        .and_then(Value::as_str)?
        .to_string();
    let turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .or_else(|| params.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)?
        .to_string();
    Some((thread_id, turn_id))
}

fn deserialize_thread_status<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(status_value_to_string))
}

fn status_value_to_string(value: &Value) -> Option<String> {
    if let Some(status) = value.as_str() {
        return (!status.trim().is_empty()).then(|| status.trim().to_string());
    }
    let status_type = value
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())?;
    let flags = value
        .get("activeFlags")
        .or_else(|| value.get("active_flags"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|flag| !flag.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if flags.is_empty() {
        Some(status_type.to_string())
    } else {
        Some(format!("{}:{}", status_type, flags.join(",")))
    }
}

fn normalize_limit(limit: usize) -> usize {
    if limit == 0 {
        25
    } else {
        limit
    }
}

fn is_unsupported_thread_resume_error(error: &str) -> bool {
    error.contains("thread resume path is no longer supported")
        || error.contains("thread resume history is no longer supported")
}

fn is_unsupported_thread_turns_list_error(error: &str) -> bool {
    error.contains("unknown variant `thread/turns/list`")
}

fn thread_read_to_turns_list_result(thread: Value, limit: usize) -> Value {
    let mut turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    turns.reverse();
    turns.truncate(normalize_limit(limit));
    json!({ "turns": turns })
}

fn is_manager_standalone_cwd(cwd: &str) -> bool {
    let Ok(standalone) = standalone_cwd() else {
        return false;
    };
    paths_equal(cwd, &standalone)
        || cwd.contains("/Documents/Codex/") && is_codex_desktop_chat_cwd(cwd)
        || cwd.ends_with("/Application Support/CodexManager/standalone")
        || cwd.ends_with("/ApplicationSupport/CodexManager/standalone")
}

fn is_codex_desktop_chat_cwd(cwd: &str) -> bool {
    let components = path_components(cwd);
    components
        .windows(4)
        .any(|window| window[0] == "Documents" && window[1] == "Codex" && is_iso_date(&window[2]))
        && components
            .iter()
            .position(|part| part == "Documents")
            .is_some_and(|index| {
                components
                    .get(index + 1)
                    .is_some_and(|part| part == "Codex")
                    && components.len() == index + 4
            })
}

fn paths_equal(left: &str, right: &Path) -> bool {
    Path::new(left) == right
}

fn path_components(value: &str) -> Vec<String> {
    Path::new(value)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_string),
            _ => None,
        })
        .collect()
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_list_request_uses_json_rpc() {
        let rpc = thread_list_request(10);

        assert_eq!(rpc.jsonrpc, "2.0");
        assert_eq!(rpc.method, "thread/list");
        let params = rpc.params.unwrap();
        assert_eq!(params["limit"], 10);
        assert_eq!(params["sortKey"], "updated_at");
        assert_eq!(params["sortDirection"], "desc");
        assert!(params.get("sourceKinds").is_none());
    }

    #[test]
    fn parse_thread_list_result_accepts_threads_key() {
        let threads = parse_thread_list_result(json!({
            "threads": [
                { "id": "thread-1", "title": "测试", "cwd": "/work/demo", "updatedAt": 100 }
            ]
        }))
        .expect("parse");

        assert_eq!(threads[0].id, "thread-1");
        assert_eq!(threads[0].title.as_deref(), Some("测试"));
        assert_eq!(threads[0].cwd.as_deref(), Some("/work/demo"));
        assert_eq!(threads[0].updated_at, Some(100));
    }

    #[test]
    fn parse_thread_list_result_accepts_array() {
        let threads = parse_thread_list_result(json!([
            { "id": "thread-1" },
            { "id": "thread-2" }
        ]))
        .expect("parse");

        assert_eq!(threads.len(), 2);
    }

    #[test]
    fn parse_thread_list_result_accepts_app_server_data_key() {
        let threads = parse_thread_list_result(json!({
            "data": [
                {
                    "id": "thread-1",
                    "cwd": "/work/demo",
                    "preview": "第一条消息",
                    "path": "/tmp/rollout.jsonl",
                    "updatedAt": 1777105074,
                    "status": "running"
                }
            ],
            "nextCursor": "cursor"
        }))
        .expect("parse");

        assert_eq!(threads[0].id, "thread-1");
        assert_eq!(threads[0].preview.as_deref(), Some("第一条消息"));
        assert_eq!(
            threads[0].rollout_path.as_deref(),
            Some("/tmp/rollout.jsonl")
        );
        assert_eq!(threads[0].updated_at, Some(1777105074));
        assert_eq!(threads[0].status.as_deref(), Some("running"));
    }

    #[test]
    fn parse_thread_list_result_accepts_object_status() {
        let threads = parse_thread_list_result(json!({
            "data": [
                {
                    "id": "thread-1",
                    "status": { "type": "active", "activeFlags": ["waitingOnApproval"] }
                }
            ]
        }))
        .expect("parse");

        assert_eq!(
            threads[0].status.as_deref(),
            Some("active:waitingOnApproval")
        );
    }

    #[test]
    fn parse_thread_status_change_accepts_object_status() {
        let change = parse_thread_status_change(&json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thread-1",
                "status": { "type": "active", "activeFlags": ["waitingOnApproval"] }
            }
        }))
        .expect("change");

        assert_eq!(
            change,
            ThreadStatusChange {
                thread_id: "thread-1".to_string(),
                status: "active:waitingOnApproval".to_string(),
            }
        );
    }

    #[test]
    fn thread_turns_list_request_uses_pagination_params() {
        let rpc = thread_turns_list_request("thread-1", Some("cursor-1"), 6, "desc");

        assert_eq!(rpc.method, "thread/turns/list");
        let params = rpc.params.unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["cursor"], "cursor-1");
        assert_eq!(params["limit"], 6);
        assert_eq!(params["sortDirection"], "desc");
    }

    #[test]
    fn thread_rollback_request_uses_num_turns() {
        let rpc = thread_rollback_request("thread-1", 3);

        assert_eq!(rpc.method, "thread/rollback");
        let params = rpc.params.unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["numTurns"], 3);
    }

    #[test]
    fn turn_steer_request_uses_expected_turn_id() {
        let rpc = turn_steer_request("thread-1", "turn-1", "追加说明");

        assert_eq!(rpc.method, "turn/steer");
        let params = rpc.params.unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["expectedTurnId"], "turn-1");
        assert_eq!(params["input"][0]["text"], "追加说明");
    }

    #[test]
    fn normalize_thread_cwd_treats_codex_desktop_chat_as_standalone() {
        assert_eq!(
            normalize_thread_cwd(Some(
                "/Users/example/Documents/Codex/2026-04-25/new-chat".to_string()
            )),
            None
        );
        assert_eq!(
            normalize_thread_cwd(Some("/Users/example/workspaces/codex-bot".to_string()))
                .as_deref(),
            Some("/Users/example/workspaces/codex-bot")
        );
    }

    #[test]
    fn standalone_cwd_uses_codex_desktop_chat_directory_shape() {
        let cwd = standalone_cwd().expect("standalone cwd");
        let cwd = cwd.to_string_lossy();

        assert!(cwd.contains("/Documents/Codex/"));
        assert!(is_codex_desktop_chat_cwd(&cwd));
    }

    #[test]
    fn standalone_cwd_uses_distinct_directory_per_new_thread() {
        let first = standalone_cwd().expect("first standalone cwd");
        let second = standalone_cwd().expect("second standalone cwd");

        assert_ne!(first, second);
    }

    #[test]
    fn standalone_cwd_for_new_thread_only_applies_to_new_standalone_threads() {
        let standalone = standalone_cwd_for_new_thread(None, None).expect("standalone cwd");
        assert!(
            standalone.as_deref().is_some_and(
                |cwd| cwd.contains("/Documents/Codex/") && is_codex_desktop_chat_cwd(cwd)
            )
        );

        assert_eq!(
            standalone_cwd_for_new_thread(Some("thread-1"), None).unwrap(),
            None
        );
        assert_eq!(
            standalone_cwd_for_new_thread(None, Some("/work/project")).unwrap(),
            None
        );
    }

    #[test]
    fn thread_read_request_uses_include_turns() {
        let rpc = thread_read_request("thread-1", true);

        assert_eq!(rpc.method, "thread/read");
        assert_eq!(rpc.params.unwrap()["threadId"], "thread-1");
    }

    #[test]
    fn thread_resume_request_uses_thread_id() {
        let rpc = thread_resume_request("thread-1");

        assert_eq!(rpc.method, "thread/resume");
        assert_eq!(rpc.params.unwrap()["threadId"], "thread-1");
    }

    #[test]
    fn thread_archive_request_uses_thread_id() {
        let rpc = thread_archive_request("thread-1");

        assert_eq!(rpc.method, "thread/archive");
        assert_eq!(rpc.params.unwrap()["threadId"], "thread-1");
    }

    #[test]
    fn turn_interrupt_request_uses_thread_and_turn_id() {
        let rpc = turn_interrupt_request("thread-1", "turn-1");

        assert_eq!(rpc.method, "turn/interrupt");
        let params = rpc.params.unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["turnId"], "turn-1");
    }

    #[test]
    fn unsupported_thread_resume_errors_are_detected() {
        assert!(is_unsupported_thread_resume_error(
            "app-server 返回错误：thread resume path is no longer supported by the current app-server protocol"
        ));
        assert!(!is_unsupported_thread_resume_error("其他错误"));
    }

    #[test]
    fn unsupported_thread_turns_list_errors_are_detected() {
        assert!(is_unsupported_thread_turns_list_error(
            "app-server 返回错误：{\"code\":-32600,\"message\":\"Invalid request: unknown variant `thread/turns/list`\"}"
        ));
        assert!(!is_unsupported_thread_turns_list_error("其他错误"));
    }

    #[test]
    fn thread_read_to_turns_list_result_returns_latest_turns_first() {
        let result = thread_read_to_turns_list_result(
            json!({
                "id": "thread-1",
                "turns": [
                    { "id": "old" },
                    { "id": "middle" },
                    { "id": "new" }
                ]
            }),
            2,
        );

        assert_eq!(result["turns"][0]["id"], "new");
        assert_eq!(result["turns"][1]["id"], "middle");
        assert_eq!(result["turns"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn thread_start_request_without_cwd_still_sends_empty_params() {
        let rpc = thread_start_request(None);

        assert_eq!(rpc.method, "thread/start");
        assert_eq!(rpc.params, Some(json!({})));
    }

    #[test]
    fn parse_thread_read_result_requires_thread_key() {
        let read = parse_thread_read_result(json!({
            "thread": { "id": "thread-1", "turns": [] }
        }))
        .expect("parse");

        assert_eq!(read.thread["id"], "thread-1");
    }

    #[test]
    fn agent_delta_reads_delta_param() {
        let value = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello" }
        });

        assert_eq!(agent_delta(&value), Some("hello"));
    }

    #[test]
    fn event_matches_turn_uses_thread_and_turn_when_present() {
        let value = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "delta": "hello"
            }
        });

        assert!(event_matches_turn(&value, "thread-1", "turn-1"));
        assert!(!event_matches_turn(&value, "thread-2", "turn-1"));
        assert!(!event_matches_turn(&value, "thread-1", "turn-2"));
    }
}
