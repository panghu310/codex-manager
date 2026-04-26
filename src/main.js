import {
  activateCodexProvider,
  codexProviderPresets,
  deleteCodexProvider,
  deleteCodexSession,
  fetchAppServerThreads,
  fetchBotSettings,
  fetchCodexLiveConfig,
  fetchCodexProviders,
  fetchCodexSessions,
  fetchTelegramBotStatus,
  filterSessions,
  groupSessionsByProject,
  mergeBotSettings,
  providerRowActions,
  restartTelegramBot,
  saveBotSettings,
  saveCodexProvider,
  summarizeAppServerStatus
} from "./status.js";
import { bindWindowDragging } from "./window-drag.js";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import "./styles.css";

const app = document.querySelector("#app");

const state = {
  view: "providers",
  loading: false,
  status: summarizeAppServerStatus([]),
  providers: [],
  editingProviderId: "",
  liveCodexConfig: "",
  sessions: [],
  selectedSessionIds: new Set(),
  sessionListScrollTop: 0,
  botSettings: null,
  botStatus: null,
  message: "",
  error: ""
};

function render() {
  rememberSessionListScroll();
  app.innerHTML = `
    <main class="cc-window">
      <div class="window-drag-region" data-window-drag></div>
      ${renderSidebar()}
      <section class="app-content">
        ${renderNotice()}
        ${renderView()}
      </section>
    </main>
  `;
  bindEvents();
  restoreSessionListScroll();
}

function renderView() {
  if (state.view === "provider-edit") return renderProviderEdit();
  if (state.view === "sessions") return renderSessions();
  if (state.view === "settings") return renderSettings();
  if (state.view === "status") return renderStatusPage();
  return renderProviderHome();
}

function rememberSessionListScroll() {
  const list = document.querySelector(".session-list");
  if (list) {
    state.sessionListScrollTop = list.scrollTop;
  }
}

function restoreSessionListScroll() {
  if (state.view !== "sessions") return;
  const list = document.querySelector(".session-list");
  if (list) {
    list.scrollTop = state.sessionListScrollTop;
  }
}

function renderNotice() {
  return `
    ${state.message ? `<p class="toast success">${escapeHtml(state.message)}</p>` : ""}
    ${state.error ? `<p class="toast danger-toast">${escapeHtml(state.error)}</p>` : ""}
  `;
}

function renderSidebar() {
  return `
    <aside class="app-sidebar">
      <div class="sidebar-drag-layer" data-window-drag></div>
      <nav class="side-nav" aria-label="主导航">
        <button type="button" class="${state.view === "status" ? "selected" : ""}" data-action="status"><span>⌂</span>状态</button>
        <button type="button" class="${state.view === "providers" || state.view === "provider-edit" ? "selected" : ""}" data-action="home"><span>▣</span>供应</button>
        <button type="button" class="${state.view === "sessions" ? "selected" : ""}" data-action="sessions"><span>▱</span>会话</button>
        <button type="button" class="${state.view === "settings" ? "selected" : ""}" data-action="settings"><span>⚙</span>设置</button>
      </nav>
      <div class="runtime-card">
        <span><i></i>运行中</span>
        <small>v1.0.0</small>
      </div>
    </aside>
  `;
}

function renderPageHeader(title, subtitle = "", action = "") {
  return `
    <header class="page-head">
      <div class="page-title-region" data-window-drag>
        <h1>${escapeHtml(title)}</h1>
        ${subtitle ? `<p>${escapeHtml(subtitle)}</p>` : ""}
      </div>
      <div class="page-head-drag-fill" data-window-drag></div>
      ${action}
    </header>
  `;
}

function renderProviderHome() {
  const activeProvider = state.providers.find((provider) => provider.active);
  return `
    ${renderPageHeader("供应", "管理 Codex 请求使用的供应商。", `<button type="button" class="primary-button" data-action="new-provider">＋ 新增</button>`)}
    <section class="provider-page">
      <article class="summary-card">
        <div>
          <span>默认供应商</span>
          <strong>${escapeHtml(activeProvider?.name || "未配置")}</strong>
        </div>
        <p>${activeProvider ? "这是当前用于所有请求的默认供应商，可在下方管理与切换。" : "还没有可用供应商，请先新增一个供应商。"}</p>
      </article>
      <div class="table-card provider-table">
        <div class="table-head">
          <span>名称</span>
          <span>端点 URL</span>
          <span>状态</span>
          <span>认证</span>
          <span>操作</span>
        </div>
        ${
          state.providers.length
            ? state.providers.map(renderProviderCard).join("")
            : `<div class="empty-state">暂无 Codex 供应商。</div>`
        }
        <footer>共 ${state.providers.length} 个供应商</footer>
      </div>
    </section>
  `;
}

function renderStatusPage() {
  const status = state.status;
  return `
    ${renderPageHeader("状态", "Codex app-server 与 Bot 的运行概览。", `<button type="button" class="secondary-button" data-action="refresh">↻ 刷新状态</button>`)}
    <section class="status-page">
      <article class="status-card ${status.connected ? "ok" : "bad"}">
        <div class="pulse-icon">⌁</div>
        <div class="status-main">
          <div>
            <span>Codex app-server</span>
            <strong>${status.connected ? "运行中" : "离线"}</strong>
          </div>
          <p>端口 8765 · 最近更新 ${formatDateTime(status.latestUpdatedAt)}</p>
        </div>
      </article>
      <div class="status-grid">
        <div><span>对话</span><strong>${status.threadCount}</strong><small>次</small></div>
        <div><span>项目</span><strong>${status.projectCount}</strong><small>个</small></div>
        <div><span>最近更新</span><strong>${formatShortTime(status.latestUpdatedAt)}</strong><small>${formatShortDate(status.latestUpdatedAt)}</small></div>
      </div>
      <article class="status-card" style="margin-top:10px">
        <div class="pulse-icon">◎</div>
        <div class="status-main">
          <div>
            <span>Telegram Bot</span>
            <strong>${status.botRunning ? "运行中" : status.botConfigured ? "已停止" : "未配置"}</strong>
          </div>
          <p>${escapeHtml(status.botDetail)}</p>
        </div>
      </article>
      <div class="lower-grid" style="margin-top:10px">
        <article class="panel-card">
          <header><strong>最近活动</strong></header>
          <ul class="activity-list">
            <li><i></i><span>${formatDateTime(status.latestUpdatedAt)}</span><p>状态检查完成</p></li>
            <li><i></i><span>当前</span><p>会话列表已就绪</p></li>
            <li><i></i><span>当前</span><p>配置已读取</p></li>
          </ul>
        </article>
        <article class="panel-card">
          <header><strong>健康检查</strong></header>
          <div class="health-list">
            <p><span>● Admin 连接</span><b>${status.connected ? "正常" : "异常"}</b></p>
            <p><span>● App-server</span><b>${status.connected ? "在线" : "离线"}</b></p>
            <p><span>● Telegram Bot</span><b>${status.botRunning ? "正常" : status.botConfigured ? "已停止" : "未配置"}</b></p>
            <p><span>● 配置读取</span><b>成功</b></p>
          </div>
        </article>
      </div>
      ${status.error ? `<p class="status-error">${escapeHtml(status.error)}</p>` : ""}
    </section>
  `;
}

function renderProviderCard(provider) {
  const actions = providerRowActions(provider);
  return `
    <article class="provider-row ${provider.active ? "active" : ""}">
      <div class="avatar">${escapeHtml(initialLetter(provider.name))}</div>
      <div class="provider-main">
        <strong>${escapeHtml(provider.name)}</strong>
      </div>
      <span class="provider-url">${escapeHtml(provider.baseUrl || "未配置 API 地址")}</span>
      ${
        provider.active
          ? `<span class="status-pill ok">✓ 使用中</span>`
          : `<button type="button" class="ghost-button compact-button status-action" data-action="activate-provider" data-id="${escapeAttr(provider.id)}">使用</button>`
      }
      <span class="status-pill ${provider.hasApiKey ? "ok-soft" : "muted-pill"}">${provider.hasApiKey ? "OK" : "未设置"}</span>
      <div class="provider-actions">
        <button type="button" class="icon-button" data-action="edit-provider" data-id="${escapeAttr(provider.id)}" aria-label="编辑 ${escapeAttr(provider.name)}" title="编辑">✎</button>
        <button
          type="button"
          class="icon-button danger-icon-button"
          data-action="delete-provider"
          data-id="${escapeAttr(provider.id)}"
          aria-label="删除 ${escapeAttr(provider.name)}"
          title="${escapeAttr(actions.deleteDisabledReason || "删除")}"
          ${actions.canDelete ? "" : "disabled"}
        >⌫</button>
      </div>
    </article>
  `;
}

function renderProviderEdit() {
  const provider = selectedProviderForForm();
  const presets = codexProviderPresets();
  return `
    ${renderPageHeader(provider ? "编辑供应商" : "新增供应商", "配置 Codex 使用的模型、端点和凭据。", `<button type="button" class="secondary-button" data-action="home">← 返回</button>`)}
    <section class="edit-card provider-edit-card">
      <div class="large-avatar">${escapeHtml(initialLetter(provider?.name || "C"))}</div>
      <div class="preset-strip">
        ${presets.map((preset) => `<button type="button" data-action="use-provider-preset" data-id="${escapeAttr(preset.id)}">${escapeHtml(preset.name)}</button>`).join("")}
      </div>
      <form id="provider-form" class="cc-form">
        <input type="hidden" name="id" value="${escapeAttr(provider?.id || "")}">
        <div class="form-grid two">
          <label>供应商名称<input name="name" value="${escapeAttr(provider?.name || "default")}" required></label>
          <label>备注<input name="note" placeholder="例如：公司专用账号"></label>
        </div>
        <label>官网链接<input name="homepage" placeholder="https://example.com（可选）"></label>
        <label>API Key<input name="apiKey" type="password" placeholder="${provider?.hasApiKey ? provider.apiKeyMasked : "OPENAI_API_KEY"}"></label>
        <div class="field-row">
          <label>API 请求地址<input name="baseUrl" value="${escapeAttr(provider?.baseUrl || "https://api.openai.com/v1")}" required></label>
          <button type="button" class="link-button" data-action="read-config">管理与测试</button>
        </div>
        <p class="hint">填写兼容 OpenAI Responses 格式的服务端点地址。</p>
        <label>模型名称<input name="model" value="${escapeAttr(provider?.model || "gpt-5.5")}" required></label>
        <label>自定义 config.toml<textarea name="configText" rows="7" placeholder="${escapeAttr(provider?.renderedConfigText || "留空时自动按 API 地址和模型生成")}">${escapeHtml(provider?.configText || "")}</textarea></label>
      </form>
    </section>
    <footer class="save-bar">
      <button type="submit" form="provider-form" class="primary-button">▣ 保存</button>
    </footer>
  `;
}

function renderSessions() {
  const filtered = filterSessions(state.sessions, "");
  const groups = groupSessionsByProject(filtered);
  const flatSessions = groups.flatMap((group) => group.sessions);
  const selectedCount = flatSessions.filter((session) => state.selectedSessionIds.has(session.sessionId)).length;
  return `
    ${renderPageHeader("会话管理", `${flatSessions.length} 个 Codex 会话`, `<button type="button" class="secondary-button" data-action="refresh-sessions">↻ 刷新</button>`)}
    <section class="session-list-page">
      <div class="session-toolbar">
        <label class="session-check-all">
          <input id="select-all-sessions" type="checkbox" ${flatSessions.length && selectedCount === flatSessions.length ? "checked" : ""}>
          全选
        </label>
        <button type="button" class="red-button" data-action="delete-selected-sessions" ${selectedCount ? "" : "disabled"}>⌫ 删除选中 ${selectedCount ? `(${selectedCount})` : ""}</button>
      </div>
      <div class="session-list">
        ${
          groups.length
            ? groups.map(renderSessionGroup).join("")
            : `<div class="empty-state">没有读取到 Codex 会话。</div>`
        }
      </div>
    </section>
  `;
}

function renderSessionGroup(group) {
  return `
    <section class="session-group">
      <h2>${escapeHtml(group.name)} <span>${group.sessions.length}</span></h2>
      ${group.sessions.map(renderSessionItem).join("")}
    </section>
  `;
}

function renderSessionItem(session) {
  const selected = state.selectedSessionIds.has(session.sessionId);
  return `
    <article class="session-row ${selected ? "selected" : ""}">
      <label class="session-select">
        <input type="checkbox" data-session-check="${escapeAttr(session.sessionId)}" ${selected ? "checked" : ""}>
      </label>
      <div class="codex-mark">◉</div>
      <div class="session-main">
        <strong>${escapeHtml(session.title || session.sessionId)}</strong>
        <span>${escapeHtml(session.sessionId)}</span>
      </div>
      <div class="session-meta">
        <span>${escapeHtml(formatRelative(session.lastActiveAt || session.createdAt))}</span>
        <span>${escapeHtml(projectName(session.projectDir))}</span>
      </div>
      <button type="button" class="icon-button danger-icon-button" data-action="delete-session" data-id="${escapeAttr(session.sessionId)}" aria-label="删除 ${escapeAttr(session.title || session.sessionId)}" title="删除">⌫</button>
    </article>
  `;
}

function renderSettings() {
  const settings = state.botSettings || {};
  return `
    ${renderPageHeader("设置", "Telegram Bot 与当前 Codex 配置。")}
    <section class="settings-layout">
      <article class="settings-panel">
        <h2>Telegram Bot</h2>
        <form id="bot-form" class="cc-form">
          <label>Bot Token<input name="telegramBotToken" type="password" placeholder="${settings.hasTelegramBotToken ? settings.telegramBotTokenMasked : "必填"}"></label>
          <label>允许的用户 ID<input name="telegramAllowedUserId" value="${escapeAttr(settings.telegramAllowedUserId || "")}"></label>
          <label>Codex 命令<input name="codexPath" value="${escapeAttr(settings.codexPath || "codex")}"></label>
          <div class="button-row">
            <button type="submit" class="primary-button">▣ 保存设置</button>
            <button type="button" class="secondary-button" id="restart-bot">↻ 重启 Bot</button>
          </div>
        </form>
      </article>
      <article class="settings-panel config-panel">
        <h2>当前 Codex 配置</h2>
        <pre class="config-preview">${escapeHtml(state.liveCodexConfig || "尚未读取 live config。")}</pre>
        <div class="config-path">配置文件路径：~/.codex/config.toml</div>
      </article>
    </section>
  `;
}

function bindEvents() {
  bindWindowDragging(document);
  document.querySelectorAll("[data-action]").forEach((element) => {
    element.addEventListener("click", () => handleAction(element.dataset.action, element.dataset));
  });
  document.querySelector("#provider-form")?.addEventListener("submit", handleProviderSubmit);
  document.querySelector("#bot-form")?.addEventListener("submit", handleBotSubmit);
  document.querySelector("#restart-bot")?.addEventListener("click", handleRestartBot);
  document.querySelector("#select-all-sessions")?.addEventListener("change", handleSelectAllSessions);
  document.querySelectorAll("[data-session-check]").forEach((checkbox) => {
    checkbox.addEventListener("change", (event) => {
      event.stopPropagation();
      toggleSessionSelection(checkbox.dataset.sessionCheck, checkbox.checked);
    });
  });
}

async function handleAction(action, dataset = {}) {
  if (action === "home") return showProviders();
  if (action === "status") return showStatus();
  if (action === "settings") return showSettings();
  if (action === "sessions") return showSessions();
  if (action === "refresh") return refreshCurrent();
  if (action === "refresh-sessions") return loadSessions();
  if (action === "new-provider") return openProviderEditor("");
  if (action === "edit-provider") return openProviderEditor(dataset.id || "");
  if (action === "delete-provider") return handleDeleteProvider(dataset.id || "");
  if (action === "activate-provider") return handleActivateProvider(dataset.id || "");
  if (action === "delete-session") return handleDeleteSession(dataset.id || "");
  if (action === "delete-selected-sessions") return handleDeleteSelectedSessions();
  if (action === "use-provider-preset") return applyProviderPreset(dataset.id || "");
  if (action === "read-config") return loadLiveCodexConfig();
}

async function showProviders() {
  state.view = "providers";
  state.message = "";
  state.error = "";
  render();
  await Promise.all([refreshStatus(), loadProviders()]);
}

async function showStatus() {
  state.view = "status";
  state.message = "";
  state.error = "";
  render();
  await refreshStatus();
}

async function showSessions() {
  state.view = "sessions";
  state.message = "";
  state.error = "";
  render();
  await loadSessions();
}

async function showSettings() {
  state.view = "settings";
  state.message = "";
  state.error = "";
  render();
  await Promise.all([loadBot(), loadLiveCodexConfig()]);
}

async function refreshCurrent() {
  if (state.view === "sessions") return loadSessions();
  if (state.view === "settings") return showSettings();
  return Promise.all([refreshStatus(), loadProviders()]);
}

function openProviderEditor(id) {
  state.editingProviderId = id;
  state.view = "provider-edit";
  state.message = "";
  state.error = "";
  render();
}

async function refreshStatus() {
  state.loading = true;
  try {
    const [threads, botStatus] = await Promise.all([
      fetchAppServerThreads(100),
      fetchTelegramBotStatus()
    ]);
    state.status = summarizeAppServerStatus(threads, botStatus);
    clearMessage();
  } catch (error) {
    state.status = summarizeAppServerStatus([], null, error);
    setError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function loadProviders() {
  try {
    state.providers = await fetchCodexProviders();
    if (state.editingProviderId && !state.providers.some((provider) => provider.id === state.editingProviderId)) {
      state.editingProviderId = "";
    }
    clearMessage();
  } catch (error) {
    setError(error);
  }
  render();
}

async function loadLiveCodexConfig() {
  try {
    state.liveCodexConfig = await fetchCodexLiveConfig();
    clearMessage();
  } catch (error) {
    setError(error);
  }
  render();
}

async function handleProviderSubmit(event) {
  event.preventDefault();
  const form = new FormData(event.currentTarget);
  const id = form.get("id") || "";
  try {
    const saved = await saveCodexProvider({
      id,
      name: form.get("name") || "",
      baseUrl: form.get("baseUrl") || "",
      model: form.get("model") || "",
      apiKey: form.get("apiKey") || "",
      configText: form.get("configText") || ""
    });
    state.editingProviderId = saved?.id || id;
    state.message = "供应商已保存。";
    state.view = "providers";
    await loadProviders();
  } catch (error) {
    setError(error);
    render();
  }
}

async function handleActivateProvider(id) {
  if (!id) return;
  try {
    await activateCodexProvider(id);
    state.editingProviderId = id;
    state.message = "供应商已激活。";
    await Promise.all([loadProviders(), loadLiveCodexConfig()]);
    state.view = "providers";
  } catch (error) {
    setError(error);
  }
  render();
}

async function handleDeleteProvider(id) {
  const provider = state.providers.find((item) => item.id === id);
  if (!provider || !providerRowActions(provider).canDelete) return;
  if (!confirm(`删除供应商「${provider.name}」？`)) return;
  try {
    await deleteCodexProvider(id);
    if (state.editingProviderId === id) state.editingProviderId = "";
    state.message = "供应商已删除。";
    await loadProviders();
  } catch (error) {
    setError(error);
    render();
  }
}

function selectedProviderForForm() {
  if (!state.editingProviderId) return null;
  return state.providers.find((provider) => provider.id === state.editingProviderId) || null;
}

function applyProviderPreset(id) {
  const preset = codexProviderPresets().find((item) => item.id === id);
  if (!preset) return;
  const form = document.querySelector("#provider-form");
  if (!form) return;
  form.elements.id.value = preset.id;
  form.elements.name.value = preset.name;
  form.elements.baseUrl.value = preset.baseUrl;
  form.elements.model.value = preset.model;
  form.elements.apiKey.value = "";
  form.elements.configText.value = preset.configText || "";
}

async function loadSessions() {
  try {
    state.sessions = await fetchCodexSessions(100);
    const validIds = new Set(state.sessions.map((session) => session.sessionId));
    state.selectedSessionIds = new Set([...state.selectedSessionIds].filter((id) => validIds.has(id)));
    clearMessage();
  } catch (error) {
    setError(error);
  }
  render();
}

function toggleSessionSelection(sessionId, checked) {
  if (!sessionId) return;
  if (checked) {
    state.selectedSessionIds.add(sessionId);
  } else {
    state.selectedSessionIds.delete(sessionId);
  }
  render();
}

function handleSelectAllSessions(event) {
  const filtered = filterSessions(state.sessions, "");
  const ids = filtered.map((session) => session.sessionId);
  if (event.currentTarget.checked) {
    ids.forEach((id) => state.selectedSessionIds.add(id));
  } else {
    ids.forEach((id) => state.selectedSessionIds.delete(id));
  }
  render();
}

async function handleDeleteSession(sessionId) {
  const session = state.sessions.find((item) => item.sessionId === sessionId);
  if (!session) return;
  try {
    await deleteCodexSession(sessionId);
    state.selectedSessionIds.delete(sessionId);
    state.message = "会话已删除。";
    await loadSessions();
  } catch (error) {
    setError(error);
    render();
  }
}

async function handleDeleteSelectedSessions() {
  const ids = [...state.selectedSessionIds].filter((id) => state.sessions.some((session) => session.sessionId === id));
  if (!ids.length) return;
  try {
    await Promise.all(ids.map((id) => deleteCodexSession(id)));
    ids.forEach((id) => state.selectedSessionIds.delete(id));
    state.message = `已删除 ${ids.length} 个会话。`;
    await loadSessions();
  } catch (error) {
    setError(error);
    render();
  }
}

async function loadBot() {
  try {
    const [settings, status] = await Promise.all([fetchBotSettings(), fetchTelegramBotStatus()]);
    state.botSettings = settings;
    state.botStatus = status;
    clearMessage();
  } catch (error) {
    setError(error);
  }
  render();
}

async function handleBotSubmit(event) {
  event.preventDefault();
  const form = Object.fromEntries(new FormData(event.currentTarget).entries());
  try {
    state.botSettings = await saveBotSettings(mergeBotSettings(form, state.botSettings));
    state.message = "TG Bot 设置已保存。";
    await loadBot();
  } catch (error) {
    setError(error);
    render();
  }
}

async function handleRestartBot() {
  try {
    state.botStatus = await restartTelegramBot();
    state.message = "TG Bot 已请求重启。";
    render();
  } catch (error) {
    setError(error);
    render();
  }
}

function clearMessage() {
  state.error = "";
}

function setError(error) {
  state.error = String(error?.message || error);
}

function initialLetter(value) {
  return String(value || "C").trim().charAt(0).toUpperCase() || "C";
}

function projectName(value) {
  if (!value) return "无项目";
  return String(value).split("/").filter(Boolean).at(-1) || value;
}

function formatDateTime(value) {
  if (!value) return "暂无";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(new Date(Number(value) > 10_000_000_000 ? Number(value) : Number(value) * 1000));
}

function formatShortDate(value) {
  if (!value) return "暂无";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit"
  }).format(new Date(Number(value) > 10_000_000_000 ? Number(value) : Number(value) * 1000));
}

function formatShortTime(value) {
  if (!value) return "暂无";
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(Number(value) > 10_000_000_000 ? Number(value) : Number(value) * 1000));
}

function formatRelative(value) {
  if (!value) return "暂无";
  const ms = Number(value) > 10_000_000_000 ? Number(value) : Number(value) * 1000;
  const minutes = Math.max(1, Math.round((Date.now() - ms) / 60000));
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return `${Math.round(hours / 24)} 天前`;
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function escapeAttr(value) {
  return escapeHtml(value).replaceAll("'", "&#39;");
}

async function checkForUpdate() {
  try {
    const update = await check();
    if (update?.available) {
      const yes = confirm(`发现新版本 ${update.version}，是否立即下载并安装？`);
      if (yes) {
        await update.downloadAndInstall();
        await relaunch();
      }
    }
  } catch (err) {
    console.error("检查更新失败:", err);
  }
}

checkForUpdate();
render();
showProviders();
