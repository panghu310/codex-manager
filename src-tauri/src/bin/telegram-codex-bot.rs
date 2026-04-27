#[path = "../app_server.rs"]
mod app_server;

use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct Config {
    token: String,
    allowed_user_id: i64,
    codex_path: String,
    drop_pending_updates: bool,
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
    edited_message: Option<Message>,
    callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
    message_id: i64,
    from: Option<User>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct User {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    id: String,
    from: User,
    message: Option<Message>,
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SentMessage {
    message_id: i64,
}

const TELEGRAM_TEXT_CHUNK_LIMIT: usize = 3500;

struct Bot {
    config: Config,
    client: Client,
    current_thread_id: Option<String>,
    current_project_dir: Option<String>,
    loaded_thread_ids: HashSet<String>,
    active_turn: Option<ActiveTurn>,
    active_interrupt: Option<oneshot::Sender<oneshot::Sender<Result<(), String>>>>,
    active_task: Option<JoinHandle<()>>,
    app_client: Arc<app_server::PersistentAppServerClient>,
    thread_statuses: Arc<Mutex<HashMap<String, String>>>,
    token_usages: Arc<Mutex<HashMap<String, app_server::ThreadTokenUsageSummary>>>,
    rate_limit: Arc<Mutex<Option<app_server::RateLimitSummary>>>,
    tracked_prompts: HashMap<(i64, i64), TrackedPrompt>,
    thread_tokens: HashMap<String, ThreadTarget>,
    project_tokens: HashMap<String, String>,
    next_token: u64,
}

struct MenuView {
    text: String,
    reply_markup: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveTurn {
    chat_id: i64,
    thread_id: String,
    turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedPrompt {
    thread_id: String,
    project_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThreadTarget {
    thread_id: String,
    project_dir: Option<String>,
}

struct RuntimeStatusView {
    running: bool,
    current_context: String,
    thread_status: Option<String>,
    usage: Option<app_server::RateLimitSummary>,
    token_usage: Option<app_server::ThreadTokenUsageSummary>,
}

struct StreamPreviewState {
    interval_ms: u64,
    min_delta_chars: usize,
    max_chars: usize,
    full_text: String,
    last_sent_text: String,
    last_sent_at_ms: Option<u64>,
}

impl StreamPreviewState {
    fn new(interval_ms: u64, min_delta_chars: usize, max_chars: usize) -> Self {
        Self {
            interval_ms,
            min_delta_chars,
            max_chars,
            full_text: String::new(),
            last_sent_text: String::new(),
            last_sent_at_ms: None,
        }
    }

    fn push(&mut self, delta: &str, now_ms: u64) -> Option<String> {
        self.full_text.push_str(delta);
        let display_text = truncate(&self.full_text, self.max_chars);
        let delta_chars = display_text
            .chars()
            .count()
            .saturating_sub(self.last_sent_text.chars().count());
        let interval_elapsed = self
            .last_sent_at_ms
            .is_some_and(|last| now_ms.saturating_sub(last) >= self.interval_ms);
        if delta_chars < self.min_delta_chars && !interval_elapsed {
            return None;
        }
        if display_text == self.last_sent_text || display_text.trim().is_empty() {
            return None;
        }
        self.last_sent_text = display_text.clone();
        self.last_sent_at_ms = Some(now_ms);
        Some(display_text)
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = load_config()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| format!("创建 Telegram HTTP client 失败：{err}"))?;
    let app_client =
        Arc::new(app_server::PersistentAppServerClient::start(&config.codex_path, None).await?);
    let thread_statuses = Arc::new(Mutex::new(HashMap::new()));
    let token_usages = Arc::new(Mutex::new(HashMap::new()));
    let rate_limit = Arc::new(Mutex::new(None));
    spawn_app_server_event_listener(
        app_client.clone(),
        thread_statuses.clone(),
        token_usages.clone(),
        rate_limit.clone(),
    );
    let mut bot = Bot {
        config,
        client,
        current_thread_id: None,
        current_project_dir: None,
        loaded_thread_ids: HashSet::new(),
        active_turn: None,
        active_interrupt: None,
        active_task: None,
        app_client,
        thread_statuses,
        token_usages,
        rate_limit,
        tracked_prompts: HashMap::new(),
        thread_tokens: HashMap::new(),
        project_tokens: HashMap::new(),
        next_token: 1,
    };

    bot.set_commands().await?;
    if bot.config.drop_pending_updates {
        bot.drop_pending_updates().await?;
    }
    eprintln!("telegram bot started");
    bot.poll().await
}

fn spawn_app_server_event_listener(
    app_client: Arc<app_server::PersistentAppServerClient>,
    thread_statuses: Arc<Mutex<HashMap<String, String>>>,
    token_usages: Arc<Mutex<HashMap<String, app_server::ThreadTokenUsageSummary>>>,
    rate_limit: Arc<Mutex<Option<app_server::RateLimitSummary>>>,
) {
    tokio::spawn(async move {
        let mut events = app_client.subscribe();
        loop {
            match events.recv().await {
                Ok(value) => {
                    if let Some(change) = app_server::parse_thread_status_change(&value) {
                        thread_statuses
                            .lock()
                            .await
                            .insert(change.thread_id, change.status);
                    }
                    if let Some(usage) = app_server::parse_thread_token_usage_update(&value) {
                        token_usages
                            .lock()
                            .await
                            .insert(usage.thread_id.clone(), usage);
                    }
                    if let Some(summary) = app_server::parse_rate_limits_update(&value) {
                        *rate_limit.lock().await = Some(summary);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn load_config() -> Result<Config, String> {
    let token = required_env("TELEGRAM_BOT_TOKEN")?;
    let allowed_user_id = required_env("TELEGRAM_ALLOWED_USER_ID")?
        .parse::<i64>()
        .map_err(|err| format!("TELEGRAM_ALLOWED_USER_ID 不是有效数字：{err}"))?;
    let codex_path = env::var("CODEX_PATH").unwrap_or_else(|_| "codex".to_string());
    let drop_pending_updates = env_bool("CODEX_BOT_DROP_PENDING_UPDATES", true);
    Ok(Config {
        token,
        allowed_user_id,
        codex_path,
        drop_pending_updates,
    })
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} 未配置"))
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(default)
}

impl Bot {
    async fn poll(&mut self) -> Result<(), String> {
        let mut offset = 0_i64;
        loop {
            let updates = self.get_updates(offset).await?;
            for update in updates {
                offset = update.update_id + 1;
                if let Err(err) = self.handle_update(update).await {
                    eprintln!("处理 Telegram update 失败：{err}");
                }
                self.clear_finished_task();
            }
        }
    }

    fn clear_finished_task(&mut self) {
        if self
            .active_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            self.active_task = None;
            self.active_turn = None;
            self.active_interrupt = None;
        }
    }

    async fn get_updates(&self, offset: i64) -> Result<Vec<Update>, String> {
        let value = json!({
            "offset": offset,
            "timeout": 25,
            "allowed_updates": ["message", "edited_message", "callback_query"]
        });
        let response: TelegramResponse<Vec<Update>> = self.telegram("getUpdates", &value).await?;
        Ok(response.result.unwrap_or_default())
    }

    async fn drop_pending_updates(&self) -> Result<(), String> {
        let value = json!({ "drop_pending_updates": true });
        let _: TelegramResponse<Value> = self.telegram("deleteWebhook", &value).await?;
        eprintln!("telegram pending updates dropped");
        Ok(())
    }

    async fn handle_update(&mut self, update: Update) -> Result<(), String> {
        if let Some(message) = update.message {
            return self.handle_message(message).await;
        }
        if let Some(message) = update.edited_message {
            return self.handle_edited_message(message).await;
        }
        if let Some(callback) = update.callback_query {
            return self.handle_callback(callback).await;
        }
        Ok(())
    }

    async fn handle_message(&mut self, message: Message) -> Result<(), String> {
        if !self.is_allowed(message.from.as_ref().map(|user| user.id)) {
            self.send_text(message.chat.id, "没有权限使用这个 bot。")
                .await?;
            return Ok(());
        }

        let text = message.text.unwrap_or_default();
        let trimmed = text.trim();
        match menu_action(trimmed) {
            Some(MenuAction::Menu) => self.send_menu(message.chat.id).await,
            Some(MenuAction::NewThread) => self.start_new_thread(message.chat.id).await,
            Some(MenuAction::Projects) => self.send_projects(message.chat.id).await,
            Some(MenuAction::Sessions) => self.send_sessions(message.chat.id).await,
            Some(MenuAction::Status) => self.send_status(message.chat.id).await,
            Some(MenuAction::Stop) => self.stop_active_turn(message.chat.id).await,
            Some(MenuAction::UnsupportedCommand) => {
                self.send_text_unit(message.chat.id, "暂不支持这个命令。可用 /start 打开菜单。")
                    .await
            }
            None if trimmed.is_empty() => Ok(()),
            None => self.ask(message.chat.id, message.message_id, trimmed).await,
        }
    }

    async fn handle_edited_message(&mut self, message: Message) -> Result<(), String> {
        if !self.is_allowed(message.from.as_ref().map(|user| user.id)) {
            return Ok(());
        }
        let text = message.text.unwrap_or_default();
        let prompt = text.trim();
        if prompt.is_empty() || menu_action(prompt).is_some() {
            return Ok(());
        }
        let Some(tracked) = self
            .tracked_prompts
            .get(&(message.chat.id, message.message_id))
            .cloned()
        else {
            self.send_text(
                message.chat.id,
                "这条消息没有可回退的 Codex 记录，请直接发送新消息继续。",
            )
            .await?;
            return Ok(());
        };
        if self.active_turn.is_some() {
            self.send_text(
                message.chat.id,
                "Codex 正在处理当前任务，请等待完成或点击停止。",
            )
            .await?;
            return Ok(());
        }
        self.current_thread_id = Some(tracked.thread_id.clone());
        self.current_project_dir = tracked.project_dir.clone();
        self.ask(message.chat.id, message.message_id, prompt).await
    }

    async fn handle_callback(&mut self, callback: CallbackQuery) -> Result<(), String> {
        self.answer_callback(&callback.id).await?;
        if !self.is_allowed(Some(callback.from.id)) {
            return Ok(());
        }
        let Some(message) = callback.message else {
            return Ok(());
        };
        let data = callback.data.unwrap_or_default();
        match data.as_str() {
            "new" => self.start_new_thread(message.chat.id).await,
            "projects" => {
                self.edit_projects(message.chat.id, message.message_id)
                    .await
            }
            "sessions" => {
                self.edit_sessions(message.chat.id, message.message_id)
                    .await
            }
            "status" => self.edit_status(message.chat.id, message.message_id).await,
            "menu" => self.edit_menu(message.chat.id, message.message_id).await,
            "stop" => self.stop_active_turn(message.chat.id).await,
            value if value.starts_with("history:") => {
                let token = value.trim_start_matches("history:");
                if let Some(target) = self.thread_tokens.get(token).cloned() {
                    self.send_thread_history(message.chat.id, &target.thread_id)
                        .await
                } else {
                    self.send_text_unit(message.chat.id, "这个历史入口已过期，请重新打开对话列表。")
                        .await
                }
            }
            value if value.starts_with("thread:") => {
                let token = value.trim_start_matches("thread:");
                if let Some(target) = self.thread_tokens.get(token).cloned() {
                    self.current_thread_id = Some(target.thread_id.clone());
                    self.current_project_dir = target.project_dir.clone();
                    self.send_thread_selected(message.chat.id, &target).await
                } else {
                    self.send_text_unit(message.chat.id, "这个对话入口已过期，请重新打开对话列表。")
                        .await
                }
            }
            value if value.starts_with("project:") => {
                let token = value.trim_start_matches("project:");
                if let Some(project_dir) = self.project_tokens.get(token).cloned() {
                    self.edit_project_sessions(message.chat.id, message.message_id, &project_dir)
                        .await
                } else {
                    self.send_text_unit(message.chat.id, "这个项目入口已过期，请重新打开项目列表。")
                        .await
                }
            }
            value if value.starts_with("project_new:") => {
                let token = value.trim_start_matches("project_new:");
                if let Some(project_dir) = self.project_tokens.get(token).cloned() {
                    self.start_new_project_thread(message.chat.id, project_dir)
                        .await
                } else {
                    self.send_text_unit(message.chat.id, "这个项目入口已过期，请重新打开项目列表。")
                        .await
                }
            }
            _ => Ok(()),
        }
    }

    fn is_allowed(&self, user_id: Option<i64>) -> bool {
        user_id == Some(self.config.allowed_user_id)
    }

    async fn start_new_thread(&mut self, chat_id: i64) -> Result<(), String> {
        let thread_id = self.app_client.start_thread(None).await?;
        self.loaded_thread_ids.insert(thread_id.clone());
        self.current_thread_id = Some(thread_id);
        self.current_project_dir = None;
        self.send_text_unit(chat_id, "已切换到独立新对话。直接发送消息即可开始。")
            .await
    }

    async fn start_new_project_thread(
        &mut self,
        chat_id: i64,
        project_dir: String,
    ) -> Result<(), String> {
        let project_name = project_label(&project_dir);
        let thread_id = self.app_client.start_thread(Some(&project_dir)).await?;
        self.loaded_thread_ids.insert(thread_id.clone());
        self.current_thread_id = Some(thread_id);
        self.current_project_dir = Some(project_dir);
        self.send_text_unit(
            chat_id,
            &format!("已切换到项目新对话：{project_name}。直接发送消息即可开始。"),
        )
        .await
    }

    async fn ask(&mut self, chat_id: i64, message_id: i64, prompt: &str) -> Result<(), String> {
        self.clear_finished_task();
        if self.active_turn.is_none() {
            if let Some(notice) = new_thread_notice(
                self.current_thread_id.as_deref(),
                self.current_project_dir.as_deref(),
            ) {
                self.send_text(chat_id, notice).await?;
            }
        }
        if let Some(active) = self.active_turn.clone() {
            self.app_client
                .steer_turn(&active.thread_id, &active.turn_id, prompt)
                .await?;
            self.send_text(chat_id, "已追加到当前运行。").await?;
            return Ok(());
        }
        let client = self.client.clone();
        let token = self.config.token.clone();
        let processing_message = self.send_text(chat_id, "正在处理...").await?;
        let app_client = self.app_client.clone();
        let thread_id = self.current_thread_id.clone();
        let resume_existing_thread =
            should_resume_thread(thread_id.as_deref(), &self.loaded_thread_ids);
        let cwd = self.current_project_dir.clone();
        let prompt = prompt.to_string();
        let tracked_key = (chat_id, message_id);
        let tracked_project_dir = cwd.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let sender = TelegramSender { client, token };
            let preview_task = spawn_stream_preview(
                sender.clone(),
                chat_id,
                processing_message.message_id,
                progress_rx,
            );
            match app_server::run_turn_interruptible_persistent_with_progress(
                app_client,
                thread_id.as_deref(),
                cwd.as_deref(),
                &prompt,
                started_tx,
                interrupt_rx,
                resume_existing_thread,
                Some(progress_tx),
            )
            .await
            {
                Ok(result) => {
                    preview_task.abort();
                    if !result.completed {
                        return;
                    }
                    let output = if result.output.trim().is_empty() {
                        "Codex 已完成，但没有返回文本。".to_string()
                    } else {
                        result.output
                    };
                    if let Err(err) = sender
                        .replace_processing_with_output(
                            chat_id,
                            processing_message.message_id,
                            &output,
                        )
                        .await
                    {
                        eprintln!("发送 Telegram Codex 输出失败：{err}");
                    }
                }
                Err(err) => {
                    preview_task.abort();
                    if let Err(send_err) = sender
                        .edit_text(
                            chat_id,
                            processing_message.message_id,
                            &format!("Codex 调用失败：{err}"),
                        )
                        .await
                    {
                        eprintln!("发送 Telegram Codex 错误失败：{send_err}");
                    }
                }
            }
        });
        self.active_task = Some(task);
        self.active_interrupt = Some(interrupt_tx);
        if let Ok(handle) = tokio::time::timeout(Duration::from_secs(8), started_rx).await {
            if let Ok(handle) = handle {
                self.current_thread_id = Some(handle.thread_id.clone());
                self.loaded_thread_ids.insert(handle.thread_id.clone());
                self.tracked_prompts.insert(
                    tracked_key,
                    TrackedPrompt {
                        thread_id: handle.thread_id.clone(),
                        project_dir: tracked_project_dir,
                    },
                );
                self.active_turn = Some(ActiveTurn {
                    chat_id,
                    thread_id: handle.thread_id,
                    turn_id: handle.turn_id,
                });
            }
        }
        Ok(())
    }

    async fn send_thread_selected(
        &self,
        chat_id: i64,
        target: &ThreadTarget,
    ) -> Result<(), String> {
        let history = self
            .thread_history_text(&target.thread_id)
            .await
            .unwrap_or_else(|err| format!("历史记录读取失败：{err}"));
        let project = target
            .project_dir
            .as_deref()
            .map(project_label)
            .unwrap_or_else(|| "独立对话".to_string());
        self.send_message(
            chat_id,
            &format!(
                "已切换到对话：{}\n项目：{}\n\n{}",
                target.thread_id, project, history
            ),
            Some(Self::current_thread_keyboard()),
        )
        .await
        .map(|_| ())
    }

    async fn send_thread_history(&self, chat_id: i64, thread_id: &str) -> Result<(), String> {
        let history = self.thread_history_text(thread_id).await?;
        self.send_long_text(chat_id, &history).await
    }

    async fn thread_history_text(&self, thread_id: &str) -> Result<String, String> {
        let result =
            app_server::list_thread_turns(&self.config.codex_path, thread_id, None, 6, "desc")
                .await?;
        let mut turns = app_server::parse_thread_turns_list_result(result)?;
        turns.reverse();
        let body = format_turn_history(&turns);
        Ok(if body.trim().is_empty() {
            "最近聊天记录：\n暂无可显示内容。".to_string()
        } else {
            format!("最近聊天记录：\n{body}")
        })
    }

    async fn stop_active_turn(&mut self, chat_id: i64) -> Result<(), String> {
        self.clear_finished_task();
        if self
            .active_turn
            .as_ref()
            .filter(|turn| turn.chat_id == chat_id)
            .is_none()
        {
            self.send_text(chat_id, "当前没有可停止的任务。").await?;
            return Ok(());
        };
        let Some(interrupt) = self.active_interrupt.take() else {
            self.active_turn = None;
            self.send_text(chat_id, "当前没有可停止的任务。").await?;
            return Ok(());
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if interrupt.send(ack_tx).is_err() {
            self.active_turn = None;
            self.send_text(chat_id, "当前没有可停止的任务。").await?;
            return Ok(());
        }
        match tokio::time::timeout(Duration::from_secs(8), ack_rx).await {
            Ok(Ok(Ok(()))) => {
                self.active_turn = None;
                self.send_text_unit(chat_id, "已停止当前生成。").await
            }
            Ok(Ok(Err(err))) => {
                self.send_text_unit(chat_id, &format!("停止失败：{err}"))
                    .await
            }
            Ok(Err(_)) => {
                self.send_text_unit(chat_id, "停止失败：生成任务已结束。")
                    .await
            }
            Err(_) => {
                self.send_text_unit(chat_id, "停止失败：等待 app-server 确认超时。")
                    .await
            }
        }
    }

    async fn send_menu(&mut self, chat_id: i64) -> Result<(), String> {
        self.remove_reply_keyboard(chat_id).await?;
        let view = self.menu_view();
        self.send_menu_view(chat_id, view).await
    }

    async fn edit_menu(&mut self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let view = self.menu_view();
        self.edit_menu_view(chat_id, message_id, view).await
    }

    fn menu_view(&self) -> MenuView {
        let thread = current_context_label(
            self.current_thread_id.as_deref(),
            self.current_project_dir.as_deref(),
        );
        MenuView {
            text: format!("CodexManager\n\n当前对话：{thread}\n\n直接发送文本即可继续当前对话。"),
            reply_markup: Self::main_inline_keyboard(),
        }
    }

    fn main_inline_keyboard() -> Value {
        json!({
            "inline_keyboard": [
                [
                    { "text": "项目", "callback_data": "projects" },
                    { "text": "对话", "callback_data": "sessions" }
                ],
                [
                    { "text": "新对话", "callback_data": "new" },
                    { "text": "状态", "callback_data": "status" },
                    { "text": "停止", "callback_data": "stop" }
                ]
            ]
        })
    }

    fn current_thread_keyboard() -> Value {
        json!({
            "inline_keyboard": [
                [{ "text": "返回菜单", "callback_data": "menu" }]
            ]
        })
    }

    async fn send_projects(&mut self, chat_id: i64) -> Result<(), String> {
        let view = self.projects_view().await?;
        self.send_menu_view(chat_id, view).await
    }

    async fn edit_projects(&mut self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let view = self.projects_view().await?;
        self.edit_menu_view(chat_id, message_id, view).await
    }

    async fn projects_view(&mut self) -> Result<MenuView, String> {
        let threads = self.list_threads_with_live_status(100).await?;
        let projects = group_projects(&threads);
        self.project_tokens.clear();
        self.thread_tokens.clear();
        self.next_token = 1;

        let mut rows = Vec::new();
        for project in projects {
            let token = self.put_project_token(project.cwd.clone());
            rows.push(vec![json!({
                "text": format!("{} ({})", project_label(&project.cwd), project.thread_count),
                "callback_data": format!("project:{token}")
            })]);
        }
        rows.push(vec![json!({ "text": "返回", "callback_data": "menu" })]);

        let text = if rows.len() == 1 {
            "暂无项目会话。".to_string()
        } else {
            "项目：".to_string()
        };
        Ok(MenuView {
            text,
            reply_markup: json!({ "inline_keyboard": rows }),
        })
    }

    async fn edit_project_sessions(
        &mut self,
        chat_id: i64,
        message_id: i64,
        project_dir: &str,
    ) -> Result<(), String> {
        let view = self.project_sessions_view(project_dir).await?;
        self.edit_menu_view(chat_id, message_id, view).await
    }

    async fn project_sessions_view(&mut self, project_dir: &str) -> Result<MenuView, String> {
        let threads = self.list_threads_with_live_status(100).await?;
        self.thread_tokens.clear();
        self.next_token = 1;

        let mut rows = Vec::new();
        for thread in threads
            .into_iter()
            .filter(|thread| thread.cwd.as_deref() == Some(project_dir))
        {
            let token = self.put_thread_token(thread.id.clone(), thread.cwd.clone());
            rows.push(vec![json!({
                "text": thread_label(&thread),
                "callback_data": format!("thread:{token}")
            })]);
        }
        if let Some(project_token) = self.token_for_project(project_dir) {
            rows.push(vec![json!({
                "text": "开启项目新会话",
                "callback_data": format!("project_new:{project_token}")
            })]);
        }
        rows.push(vec![
            json!({ "text": "返回项目", "callback_data": "projects" }),
        ]);
        rows.push(vec![json!({ "text": "返回菜单", "callback_data": "menu" })]);

        let text = if rows.len() == 3 {
            format!("项目：{}\n暂无会话。", project_label(project_dir))
        } else {
            format!("项目：{}", project_label(project_dir))
        };
        Ok(MenuView {
            text,
            reply_markup: json!({ "inline_keyboard": rows }),
        })
    }

    async fn send_sessions(&mut self, chat_id: i64) -> Result<(), String> {
        let view = self.sessions_view().await?;
        self.send_menu_view(chat_id, view).await
    }

    async fn edit_sessions(&mut self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let view = self.sessions_view().await?;
        self.edit_menu_view(chat_id, message_id, view).await
    }

    async fn sessions_view(&mut self) -> Result<MenuView, String> {
        let threads = self.list_threads_with_live_status(20).await?;
        self.thread_tokens.clear();
        self.next_token = 1;

        let mut rows = Vec::new();
        for thread in threads
            .into_iter()
            .filter(|thread| app_server::normalize_thread_cwd(thread.cwd.clone()).is_none())
        {
            let token = self.put_thread_token(thread.id.clone(), None);
            rows.push(vec![json!({
                "text": thread_label(&thread),
                "callback_data": format!("thread:{token}")
            })]);
        }
        rows.push(vec![json!({ "text": "返回", "callback_data": "menu" })]);

        let text = if rows.len() == 1 {
            "暂无独立对话。".to_string()
        } else {
            "独立对话：".to_string()
        };
        Ok(MenuView {
            text,
            reply_markup: json!({ "inline_keyboard": rows }),
        })
    }

    async fn send_status(&self, chat_id: i64) -> Result<(), String> {
        let view = self.status_view().await;
        self.send_menu_view(chat_id, view).await
    }

    async fn edit_status(&self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let view = self.status_view().await;
        self.edit_menu_view(chat_id, message_id, view).await
    }

    async fn status_view(&self) -> MenuView {
        if let Ok(Some(summary)) = self.app_client.read_rate_limits().await {
            *self.rate_limit.lock().await = Some(summary);
        }
        let current_thread_id = self.current_thread_id.as_deref();
        let current_thread_status = match current_thread_id {
            Some(thread_id) => self.thread_statuses.lock().await.get(thread_id).cloned(),
            None => None,
        };
        let token_usage = match current_thread_id {
            Some(thread_id) => self.token_usages.lock().await.get(thread_id).cloned(),
            None => None,
        };
        let usage = self.rate_limit.lock().await.clone();
        MenuView {
            text: format_runtime_status(RuntimeStatusView {
                running: true,
                current_context: current_context_label(
                    self.current_thread_id.as_deref(),
                    self.current_project_dir.as_deref(),
                ),
                thread_status: current_thread_status,
                usage,
                token_usage,
            }),
            reply_markup: json!({
                "inline_keyboard": [
                    [{ "text": "返回菜单", "callback_data": "menu" }]
                ]
            }),
        }
    }

    async fn list_threads_with_live_status(
        &self,
        limit: usize,
    ) -> Result<Vec<app_server::AppServerThread>, String> {
        let mut threads = app_server::list_threads(&self.config.codex_path, limit).await?;
        let statuses = self.thread_statuses.lock().await.clone();
        for thread in &mut threads {
            if let Some(status) = statuses.get(&thread.id) {
                thread.status = Some(status.clone());
            }
        }
        Ok(threads)
    }

    fn put_thread_token(&mut self, thread_id: String, project_dir: Option<String>) -> String {
        let token = self.next_token.to_string();
        self.next_token += 1;
        self.thread_tokens.insert(
            token.clone(),
            ThreadTarget {
                thread_id,
                project_dir,
            },
        );
        token
    }

    fn put_project_token(&mut self, project_dir: String) -> String {
        let token = self.next_token.to_string();
        self.next_token += 1;
        self.project_tokens.insert(token.clone(), project_dir);
        token
    }

    fn token_for_project(&self, project_dir: &str) -> Option<String> {
        self.project_tokens
            .iter()
            .find_map(|(token, value)| (value == project_dir).then(|| token.clone()))
    }

    async fn set_commands(&self) -> Result<(), String> {
        self.delete_commands().await?;
        let commands = bot_commands();
        let _: TelegramResponse<Value> = self.telegram("setMyCommands", &commands).await?;
        Ok(())
    }

    async fn delete_commands(&self) -> Result<(), String> {
        let value = json!({});
        let _: TelegramResponse<Value> = self.telegram("deleteMyCommands", &value).await?;
        Ok(())
    }

    async fn send_text(&self, chat_id: i64, text: &str) -> Result<SentMessage, String> {
        self.send_message(chat_id, text, None).await
    }

    async fn send_text_unit(&self, chat_id: i64, text: &str) -> Result<(), String> {
        self.send_text(chat_id, text).await.map(|_| ())
    }

    async fn send_long_text(&self, chat_id: i64, text: &str) -> Result<(), String> {
        let sender = TelegramSender {
            client: self.client.clone(),
            token: self.config.token.clone(),
        };
        sender.send_long_text(chat_id, text).await
    }

    async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_markup: Option<Value>,
    ) -> Result<SentMessage, String> {
        let mut value = json!({
            "chat_id": chat_id,
            "text": text
        });
        if let Some(reply_markup) = reply_markup {
            value["reply_markup"] = reply_markup;
        }
        let response: TelegramResponse<SentMessage> = self.telegram("sendMessage", &value).await?;
        response
            .result
            .ok_or_else(|| "Telegram sendMessage 响应缺少消息".to_string())
    }

    async fn remove_reply_keyboard(&self, chat_id: i64) -> Result<(), String> {
        let value = json!({
            "chat_id": chat_id,
            "text": "正在打开菜单。",
            "reply_markup": { "remove_keyboard": true }
        });
        let response: TelegramResponse<SentMessage> = self.telegram("sendMessage", &value).await?;
        if let Some(message) = response.result {
            self.delete_message(chat_id, message.message_id).await?;
        }
        Ok(())
    }

    async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let value = json!({
            "chat_id": chat_id,
            "message_id": message_id
        });
        let _: TelegramResponse<Value> = self.telegram("deleteMessage", &value).await?;
        Ok(())
    }

    async fn send_menu_view(&self, chat_id: i64, view: MenuView) -> Result<(), String> {
        self.send_message(chat_id, &view.text, Some(view.reply_markup))
            .await
            .map(|_| ())
    }

    async fn edit_menu_view(
        &self,
        chat_id: i64,
        message_id: i64,
        view: MenuView,
    ) -> Result<(), String> {
        let value = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": view.text,
            "reply_markup": view.reply_markup
        });
        match self.telegram::<Value>("editMessageText", &value).await {
            Ok(_) => Ok(()),
            Err(err) if err.contains("message is not modified") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn answer_callback(&self, callback_query_id: &str) -> Result<(), String> {
        let value = json!({ "callback_query_id": callback_query_id });
        let _: TelegramResponse<Value> = self.telegram("answerCallbackQuery", &value).await?;
        Ok(())
    }

    async fn telegram<T>(&self, method: &str, value: &Value) -> Result<TelegramResponse<T>, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!(
            "https://api.telegram.org/bot{}/{}",
            self.config.token, method
        );
        let response = self
            .client
            .post(url)
            .json(value)
            .send()
            .await
            .map_err(|err| format!("请求 Telegram API 失败：{err}"))?;
        let status = response.status();
        let decoded: TelegramResponse<T> = response
            .json()
            .await
            .map_err(|err| format!("解析 Telegram API 响应失败：{err}"))?;
        if !status.is_success() || !decoded.ok {
            return Err(format!(
                "Telegram API {method} 失败：{}",
                decoded.description.unwrap_or_else(|| status.to_string())
            ));
        }
        Ok(decoded)
    }
}

#[derive(Clone)]
struct TelegramSender {
    client: Client,
    token: String,
}

fn spawn_stream_preview(
    sender: TelegramSender,
    chat_id: i64,
    message_id: i64,
    mut progress_rx: mpsc::UnboundedReceiver<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let started_at = Instant::now();
        let mut preview = StreamPreviewState::new(1500, 40, 1800);
        while let Some(delta) = progress_rx.recv().await {
            let now_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            let Some(text) = preview.push(&delta, now_ms) else {
                continue;
            };
            if let Err(err) = sender
                .edit_text(chat_id, message_id, &format!("正在处理...\n\n{text}"))
                .await
            {
                eprintln!("更新 Telegram 流式预览失败：{err}");
                return;
            }
        }
    })
}

impl TelegramSender {
    async fn send_text(&self, chat_id: i64, text: &str) -> Result<SentMessage, String> {
        self.send_message(chat_id, text).await
    }

    async fn send_long_text(&self, chat_id: i64, text: &str) -> Result<(), String> {
        for chunk in split_long_text(text) {
            self.send_text(chat_id, &chunk).await?;
        }
        Ok(())
    }

    async fn replace_processing_with_output(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), String> {
        let mut chunks = split_long_text(text);
        let first = chunks
            .drain(..1)
            .next()
            .unwrap_or_else(|| "已完成，但没有返回文本。".to_string());
        self.edit_text(chat_id, message_id, &first).await?;
        for chunk in chunks {
            self.send_text(chat_id, &chunk).await?;
        }
        Ok(())
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<SentMessage, String> {
        let value = json!({
            "chat_id": chat_id,
            "text": text
        });
        let response: TelegramResponse<SentMessage> = self.telegram("sendMessage", &value).await?;
        response
            .result
            .ok_or_else(|| "Telegram sendMessage 响应缺少消息".to_string())
    }

    async fn edit_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), String> {
        let value = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text
        });
        match self.telegram::<Value>("editMessageText", &value).await {
            Ok(_) => Ok(()),
            Err(err) if err.contains("message is not modified") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn telegram<T>(&self, method: &str, value: &Value) -> Result<TelegramResponse<T>, String>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        let response = self
            .client
            .post(url)
            .json(value)
            .send()
            .await
            .map_err(|err| format!("请求 Telegram API 失败：{err}"))?;
        let status = response.status();
        let decoded: TelegramResponse<T> = response
            .json()
            .await
            .map_err(|err| format!("解析 Telegram API 响应失败：{err}"))?;
        if !status.is_success() || !decoded.ok {
            return Err(format!(
                "Telegram API {method} 失败：{}",
                decoded.description.unwrap_or_else(|| status.to_string())
            ));
        }
        Ok(decoded)
    }
}

fn thread_label(thread: &app_server::AppServerThread) -> String {
    let value = thread
        .title
        .as_deref()
        .or(thread.preview.as_deref())
        .unwrap_or(&thread.id)
        .trim();
    let label = truncate(value, 56);
    match thread
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| !status.is_empty())
    {
        Some(status) => format!("{} · {}", status_label(status), label),
        None => label,
    }
}

fn status_label(status: &str) -> String {
    match status {
        "running" => "运行中".to_string(),
        "in_progress" => "运行中".to_string(),
        "active" => "运行中".to_string(),
        "active:waitingOnApproval" => "等待确认".to_string(),
        "idle" => "空闲".to_string(),
        "queued" => "排队中".to_string(),
        "completed" => "已完成".to_string(),
        "failed" => "失败".to_string(),
        "cancelled" => "已取消".to_string(),
        value => value.to_string(),
    }
}

fn format_runtime_status(view: RuntimeStatusView) -> String {
    let mut lines = vec![
        format!(
            "状态：{}",
            if view.running {
                "运行中"
            } else {
                "已停止"
            }
        ),
        format!("当前对话：{}", view.current_context),
        format!(
            "当前线程：{}",
            view.thread_status
                .as_deref()
                .map(status_label)
                .unwrap_or_else(|| "未知".to_string())
        ),
    ];
    if let Some(usage) = view.usage {
        let bucket = usage.bucket.unwrap_or_else(|| "额度".to_string());
        let plan = usage.plan.unwrap_or_else(|| "未知套餐".to_string());
        let percent = usage
            .used_percent
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "未知".to_string());
        lines.push(format!("额度：{bucket} · {plan} · {percent}"));
    }
    if let Some(token_usage) = view.token_usage {
        let context = match token_usage.context_window {
            Some(window) if window > 0 => format!("{}/{}", token_usage.used_tokens, window),
            _ => token_usage.used_tokens.to_string(),
        };
        let percent = token_usage
            .used_percent
            .map(|value| format!(" · {value}%"))
            .unwrap_or_default();
        lines.push(format!("上下文：{context}{percent}"));
    }
    lines.join("\n")
}

fn format_turn_history(turns: &[Value]) -> String {
    let mut lines = Vec::new();
    for turn in turns {
        for item in ArrayItems::new(turn) {
            if let Some((role, text)) = summarize_history_item(item) {
                let text = truncate(text.trim(), 700);
                if !text.is_empty() {
                    lines.push(format!("{}：{}", role, text));
                }
            }
        }
    }
    lines.join("\n\n")
}

struct ArrayItems<'a> {
    items: Vec<&'a Value>,
    index: usize,
}

impl<'a> ArrayItems<'a> {
    fn new(turn: &'a Value) -> Self {
        let items = turn
            .get("items")
            .or_else(|| turn.get("messages"))
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_else(|| vec![turn]);
        Self { items, index: 0 }
    }
}

impl<'a> Iterator for ArrayItems<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.items.get(self.index).copied();
        self.index += 1;
        item
    }
}

fn summarize_history_item(item: &Value) -> Option<(&'static str, &str)> {
    let role = item.get("role").and_then(Value::as_str).unwrap_or_default();
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let text = item
        .get("text")
        .or_else(|| item.get("content"))
        .or_else(|| item.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("content")
                .and_then(Value::as_array)
                .and_then(|content| {
                    content
                        .iter()
                        .find_map(|part| part.get("text").and_then(Value::as_str))
                })
        })?;
    if text.trim().is_empty() {
        return None;
    }
    if role == "user" || item_type.contains("user") {
        return Some(("你", text));
    }
    if role == "assistant"
        || role == "agent"
        || item_type.contains("agent")
        || item_type.contains("assistant")
    {
        return Some(("Codex", text));
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectGroup {
    cwd: String,
    thread_count: usize,
    latest_updated_at: i64,
}

fn group_projects(threads: &[app_server::AppServerThread]) -> Vec<ProjectGroup> {
    let mut projects: HashMap<String, ProjectGroup> = HashMap::new();
    for thread in threads {
        let Some(cwd) = thread
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
        else {
            continue;
        };
        if app_server::normalize_thread_cwd(Some(cwd.to_string())).is_none() {
            continue;
        }
        let entry = projects
            .entry(cwd.to_string())
            .or_insert_with(|| ProjectGroup {
                cwd: cwd.to_string(),
                thread_count: 0,
                latest_updated_at: 0,
            });
        entry.thread_count += 1;
        entry.latest_updated_at = entry
            .latest_updated_at
            .max(thread.updated_at.unwrap_or_default());
    }
    let mut groups: Vec<_> = projects.into_values().collect();
    groups.sort_by(|left, right| {
        right
            .latest_updated_at
            .cmp(&left.latest_updated_at)
            .then_with(|| project_label(&left.cwd).cmp(&project_label(&right.cwd)))
    });
    groups
}

fn project_label(value: &str) -> String {
    value
        .trim_end_matches('/')
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn current_context_label(thread_id: Option<&str>, project_dir: Option<&str>) -> String {
    match (thread_id, project_dir) {
        (Some(thread_id), Some(project_dir)) => {
            format!("{} · {}", project_label(project_dir), thread_id)
        }
        (Some(thread_id), None) => thread_id.to_string(),
        (None, Some(project_dir)) => format!("{} · 新对话", project_label(project_dir)),
        (None, None) => "未绑定".to_string(),
    }
}

fn new_thread_notice(thread_id: Option<&str>, project_dir: Option<&str>) -> Option<&'static str> {
    if thread_id.is_none() && project_dir.is_none() {
        Some("已切换到独立新对话。直接发送消息即可开始。")
    } else {
        None
    }
}

fn should_resume_thread(thread_id: Option<&str>, loaded_thread_ids: &HashSet<String>) -> bool {
    thread_id
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .is_some_and(|thread_id| !loaded_thread_ids.contains(thread_id))
}

fn truncate(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max {
        value.to_string()
    } else {
        chars
            .into_iter()
            .take(max.saturating_sub(3))
            .collect::<String>()
            + "..."
    }
}

fn split_long_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.len() >= TELEGRAM_TEXT_CHUNK_LIMIT {
            chunks.push(current);
            current = String::new();
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn slash_command(value: &str) -> Option<&str> {
    let command = value
        .strip_prefix('/')?
        .split_whitespace()
        .next()
        .unwrap_or_default();
    command.split('@').next().filter(|name| !name.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Menu,
    NewThread,
    Projects,
    Sessions,
    Status,
    Stop,
    UnsupportedCommand,
}

fn menu_action(value: &str) -> Option<MenuAction> {
    match slash_command(value) {
        Some("start") => return Some(MenuAction::Menu),
        Some("new") => return Some(MenuAction::NewThread),
        Some("projects") => return Some(MenuAction::Projects),
        Some("sessions") => return Some(MenuAction::Sessions),
        Some("status") => return Some(MenuAction::Status),
        Some(_) => return Some(MenuAction::UnsupportedCommand),
        None => {}
    }

    match value {
        "菜单" => Some(MenuAction::Menu),
        "新对话" => Some(MenuAction::NewThread),
        "项目" => Some(MenuAction::Projects),
        "对话" => Some(MenuAction::Sessions),
        "状态" => Some(MenuAction::Status),
        "停止" => Some(MenuAction::Stop),
        _ => None,
    }
}

fn bot_commands() -> Value {
    json!({
        "commands": [
            { "command": "start", "description": "打开菜单" },
            { "command": "new", "description": "开启独立新对话" },
            { "command": "projects", "description": "列出项目" },
            { "command": "sessions", "description": "列出独立对话" },
            { "command": "status", "description": "查看状态" }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_command_accepts_plain_and_mentioned_commands() {
        assert_eq!(slash_command("/start"), Some("start"));
        assert_eq!(slash_command("/start@nova_bot"), Some("start"));
        assert_eq!(slash_command("菜单"), None);
    }

    #[test]
    fn menu_action_handles_reply_keyboard_text_before_codex_prompt() {
        assert_eq!(menu_action("项目"), Some(MenuAction::Projects));
        assert_eq!(menu_action("对话"), Some(MenuAction::Sessions));
        assert_eq!(menu_action("新对话"), Some(MenuAction::NewThread));
        assert_eq!(menu_action("状态"), Some(MenuAction::Status));
        assert_eq!(menu_action("停止"), Some(MenuAction::Stop));
        assert_eq!(menu_action("帮我改代码"), None);
    }

    #[test]
    fn menu_action_supports_projects_command() {
        assert_eq!(menu_action("/projects"), Some(MenuAction::Projects));
        assert_eq!(
            menu_action("/projects@nova_bot"),
            Some(MenuAction::Projects)
        );
    }

    #[test]
    fn bot_commands_omit_menu_to_avoid_duplicate_menu_entry() {
        let commands = bot_commands();
        let values = commands["commands"].as_array().expect("commands");

        assert!(values.iter().any(|command| command["command"] == "start"));
        assert!(!values.iter().any(|command| command["command"] == "menu"));
    }

    #[test]
    fn current_context_label_shows_project_new_thread_state() {
        assert_eq!(current_context_label(None, None), "未绑定");
        assert_eq!(current_context_label(Some("thread-1"), None), "thread-1");
        assert_eq!(
            current_context_label(None, Some("/Users/example/workspaces/codex-bot")),
            "codex-bot · 新对话"
        );
        assert_eq!(
            current_context_label(
                Some("thread-1"),
                Some("/Users/example/workspaces/codex-bot")
            ),
            "codex-bot · thread-1"
        );
    }

    #[test]
    fn thread_label_includes_status_when_present() {
        let thread = app_server::AppServerThread {
            id: "thread-1".to_string(),
            title: Some("修复问题".to_string()),
            cwd: None,
            preview: None,
            rollout_path: None,
            updated_at: None,
            status: Some("running".to_string()),
        };

        assert_eq!(thread_label(&thread), "运行中 · 修复问题");
    }

    #[test]
    fn status_label_handles_structured_active_status() {
        assert_eq!(status_label("active:waitingOnApproval"), "等待确认");
        assert_eq!(status_label("idle"), "空闲");
    }

    #[test]
    fn format_turn_history_renders_last_page_messages() {
        let history = format_turn_history(&[
            json!({
                "items": [
                    { "role": "user", "text": "帮我看看状态" },
                    { "role": "assistant", "text": "当前状态正常" }
                ]
            }),
            json!({
                "items": [
                    { "type": "user_message", "content": [{ "type": "input_text", "text": "继续" }] },
                    { "type": "agent_message", "text": "已经继续处理" }
                ]
            }),
        ]);

        assert_eq!(
            history,
            "你：帮我看看状态\n\nCodex：当前状态正常\n\n你：继续\n\nCodex：已经继续处理"
        );
    }

    #[test]
    fn split_long_text_keeps_first_chunk_for_processing_message() {
        let chunks = split_long_text(&"a".repeat(TELEGRAM_TEXT_CHUNK_LIMIT + 2));

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), TELEGRAM_TEXT_CHUNK_LIMIT);
        assert_eq!(chunks[1].len(), 2);
    }

    #[test]
    fn stream_preview_flushes_only_after_min_delta_or_interval() {
        let mut preview = StreamPreviewState::new(1500, 8, 120);

        assert_eq!(preview.push("短", 0), None);
        assert_eq!(preview.push("消息", 200), None);
        assert_eq!(
            preview.push("，这次足够长", 400).as_deref(),
            Some("短消息，这次足够长")
        );
        assert_eq!(preview.push("追加", 600), None);
        assert_eq!(
            preview.push("内容", 2100).as_deref(),
            Some("短消息，这次足够长追加内容")
        );
    }

    #[test]
    fn format_runtime_status_includes_usage_and_context() {
        let text = format_runtime_status(RuntimeStatusView {
            running: true,
            current_context: "demo · thread-1".to_string(),
            thread_status: Some("active:waitingOnApproval".to_string()),
            usage: Some(app_server::RateLimitSummary {
                plan: Some("plus".to_string()),
                bucket: Some("Codex".to_string()),
                used_percent: Some(70),
                resets_at: Some(3000),
            }),
            token_usage: Some(app_server::ThreadTokenUsageSummary {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                used_tokens: 2500,
                context_window: Some(10000),
                used_percent: Some(25),
            }),
        });

        assert!(text.contains("状态：运行中"));
        assert!(text.contains("当前线程：等待确认"));
        assert!(text.contains("额度：Codex · plus · 70%"));
        assert!(text.contains("上下文：2500/10000 · 25%"));
    }

    #[test]
    fn new_thread_notice_respects_project_context() {
        assert_eq!(new_thread_notice(None, Some("/work/codex-manager")), None);
        assert_eq!(
            new_thread_notice(None, None),
            Some("已切换到独立新对话。直接发送消息即可开始。")
        );
        assert_eq!(new_thread_notice(Some("thread-1"), None), None);
    }

    #[test]
    fn should_resume_thread_skips_loaded_threads() {
        let loaded = HashSet::from(["thread-precreated".to_string()]);

        assert!(!should_resume_thread(Some("thread-precreated"), &loaded));
        assert!(should_resume_thread(Some("thread-from-history"), &loaded));
        assert!(!should_resume_thread(None, &loaded));
    }

    #[test]
    fn group_projects_uses_project_threads_only_and_sorts_by_activity() {
        let groups = group_projects(&[
            app_server::AppServerThread {
                id: "solo".to_string(),
                title: None,
                cwd: None,
                preview: None,
                rollout_path: None,
                updated_at: Some(900),
                status: None,
            },
            app_server::AppServerThread {
                id: "codex-desktop-chat".to_string(),
                title: None,
                cwd: Some("/Users/example/Documents/Codex/2026-04-25/new-chat".to_string()),
                preview: None,
                rollout_path: None,
                updated_at: Some(800),
                status: None,
            },
            app_server::AppServerThread {
                id: "p1-old".to_string(),
                title: None,
                cwd: Some("/work/codex-bot".to_string()),
                preview: None,
                rollout_path: None,
                updated_at: Some(100),
                status: None,
            },
            app_server::AppServerThread {
                id: "p2".to_string(),
                title: None,
                cwd: Some("/work/test2".to_string()),
                preview: None,
                rollout_path: None,
                updated_at: Some(300),
                status: None,
            },
            app_server::AppServerThread {
                id: "p1-new".to_string(),
                title: None,
                cwd: Some("/work/codex-bot".to_string()),
                preview: None,
                rollout_path: None,
                updated_at: Some(500),
                status: None,
            },
        ]);

        assert_eq!(
            groups
                .iter()
                .map(|group| (
                    project_label(&group.cwd),
                    group.thread_count,
                    group.latest_updated_at
                ))
                .collect::<Vec<_>>(),
            vec![
                ("codex-bot".to_string(), 2, 500),
                ("test2".to_string(), 1, 300)
            ]
        );
    }
}
