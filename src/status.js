import { invoke as tauriInvoke } from "@tauri-apps/api/core";

const STANDALONE_CWD_MARKER = "/Application Support/CodexManager/standalone";
const COMPACT_STANDALONE_CWD_MARKER = "/ApplicationSupport/CodexManager/standalone";
const CODEX_DESKTOP_CHAT_MARKER = "/Documents/Codex/";

async function invokeTauri(command, args = {}) {
  if (!globalThis.__TAURI_INTERNALS__) {
    return null;
  }
  return tauriInvoke(command, args);
}

export function summarizeAppServerThreads(threads) {
  return (Array.isArray(threads) ? threads : []).map((thread) => ({
    id: thread.id,
    title: thread.name || thread.title || firstLine(thread.preview) || thread.id,
    cwd: normalizeThreadCwd(thread.cwd),
    preview: thread.preview || "",
    updatedAt: thread.updatedAt || thread.updated_at || null
  }));
}

export function summarizeAppServerSession(thread) {
  const sessionId = String(thread?.id || "");
  return {
    sessionId,
    title: thread?.name || thread?.title || firstLine(thread?.preview) || sessionId,
    summary: thread?.preview || "",
    projectDir: normalizeThreadCwd(thread?.cwd),
    createdAt: thread?.createdAt ?? thread?.created_at ?? null,
    lastActiveAt: thread?.updatedAt ?? thread?.updated_at ?? thread?.createdAt ?? thread?.created_at ?? null,
    resumeCommand: sessionId ? `codex resume ${sessionId}` : "",
    source: thread?.source || "",
    status: normalizeThreadStatus(thread?.status)
  };
}

export function summarizeAppServerStatus(threads, botStatus = null, error = null) {
  const items = Array.isArray(threads) ? threads : [];
  const projects = new Set(items.map((thread) => normalizeThreadCwd(thread?.cwd)).filter(Boolean));
  const latestUpdatedAt = items.reduce((latest, thread) => {
    const value = Number(thread?.updatedAt ?? thread?.updated_at ?? 0);
    return value > latest ? value : latest;
  }, 0);

  return {
    connected: !error,
    threadCount: items.length,
    projectCount: projects.size,
    latestUpdatedAt: latestUpdatedAt || null,
    error: error ? String(error.message || error) : null,
    botConfigured: botStatus?.configured ?? false,
    botRunning: botStatus?.running ?? false,
    botDetail: botStatus?.detail || ""
  };
}

export function normalizeThreadStatus(status) {
  if (!status) return null;
  if (typeof status === "string") return status.trim() || null;
  if (typeof status === "object") {
    const type = String(status.type || "").trim();
    if (!type) return null;
    const flags = Array.isArray(status.activeFlags)
      ? status.activeFlags.map((flag) => String(flag).trim()).filter(Boolean)
      : [];
    return flags.length ? `${type}:${flags.join(",")}` : type;
  }
  return String(status);
}

export function summarizeAppServerThread(payload) {
  const thread = payload?.thread ?? payload ?? {};
  const turns = Array.isArray(thread.turns) ? thread.turns : [];

  return {
    id: thread.id || "",
    title: thread.name || thread.title || firstLine(thread.preview) || thread.id || "未命名对话",
    cwd: normalizeThreadCwd(thread.cwd),
    preview: thread.preview || "",
    updatedAt: thread.updatedAt || thread.updated_at || null,
    turns,
    messages: messagesFromTurns(turns)
  };
}

export function groupThreadsForMenu(threads) {
  const projectMap = new Map();
  const standalone = [];

  for (const thread of Array.isArray(threads) ? threads : []) {
    const normalizedCwd = normalizeThreadCwd(thread?.cwd);
    if (!normalizedCwd) {
      standalone.push(thread);
      continue;
    }

    const cwd = String(normalizedCwd);
    if (!projectMap.has(cwd)) {
      projectMap.set(cwd, {
        cwd,
        name: lastPathPart(cwd),
        threads: []
      });
    }
    projectMap.get(cwd).threads.push(thread);
  }

  return {
    projects: [...projectMap.values()],
    standalone
  };
}

export async function fetchAppServerThreads(limit = 25) {
  return summarizeAppServerThreads((await invokeTauri("list_app_server_threads", { limit })) ?? []);
}

export async function fetchAppServerThread(threadId) {
  if (!threadId) return null;
  const result = await invokeTauri("read_app_server_thread", { threadId });
  if (!result) return null;
  return summarizeAppServerThread(result);
}

export async function fetchCodexProviders() {
  const providers = (await invokeTauri("list_codex_providers")) ?? [];
  return providers.map(summarizeCodexProvider);
}

export async function saveCodexProvider(provider) {
  return invokeTauri("save_codex_provider", { provider });
}

export async function deleteCodexProvider(id) {
  return invokeTauri("delete_codex_provider", { id });
}

export async function activateCodexProvider(id) {
  return invokeTauri("activate_codex_provider", { id });
}

export async function fetchCodexLiveConfig() {
  return (await invokeTauri("read_codex_live_config")) ?? "";
}

export async function fetchCodexSessions(limit = 100) {
  return ((await invokeTauri("list_app_server_threads", { limit })) ?? []).map(summarizeAppServerSession);
}

export async function fetchCodexSessionMessages(sessionId) {
  const result = await invokeTauri("list_app_server_thread_turns", { threadId: sessionId, limit: 20 });
  return summarizeThreadTurns(result).messages.map((message) => ({
    role: message.role,
    content: message.text,
    ts: message.ts
  }));
}

export async function deleteCodexSession(sessionId) {
  return invokeTauri("archive_app_server_thread", { threadId: sessionId });
}

export function summarizeThreadTurns(payload) {
  const turns = normalizeTurnsPayload(payload);
  const orderedTurns = [...turns].reverse();
  return {
    turns: orderedTurns,
    messages: messagesFromTurns(orderedTurns)
  };
}

export async function fetchBotSettings() {
  return invokeTauri("get_bot_settings");
}

export async function saveBotSettings(settings) {
  return invokeTauri("save_bot_settings", { settings });
}

export async function restartTelegramBot() {
  return invokeTauri("restart_telegram_bot");
}

export async function fetchTelegramBotStatus() {
  return invokeTauri("get_telegram_bot_status");
}

export function maskSecret(value) {
  const text = String(value || "");
  if (!text) return "";
  if (text.length <= 8) return "*".repeat(text.length);
  return `${"*".repeat(Math.max(8, text.length - 4))}${text.slice(-4)}`;
}

export function buildCodexAuthText(apiKey = "") {
  const key = String(apiKey || "").trim();
  if (!key) return "{}";
  return JSON.stringify({ OPENAI_API_KEY: key }, null, 2);
}

export function buildCodexConfigText({
  baseUrl = "",
  model = "gpt-5.4",
  contextWindow1m = false,
  autoCompactTokenLimit = 900000
} = {}) {
  const lines = [
    'model_provider = "custom"',
    `model = "${escapeTomlString(model)}"`,
    'model_reasoning_effort = "high"',
    "disable_response_storage = true"
  ];
  if (contextWindow1m) {
    lines.push("model_context_window = 1000000");
    lines.push(`model_auto_compact_token_limit = ${positiveInt(autoCompactTokenLimit, 900000)}`);
  }
  lines.push(
    "",
    "[model_providers.custom]",
    'name = "custom"',
    `base_url = "${escapeTomlString(baseUrl)}"`,
    'env_key = "OPENAI_API_KEY"',
    'wire_api = "responses"',
    "requires_openai_auth = true"
  );
  return `${lines.join("\n")}\n`;
}

export function syncCodexConfigBaseUrl(configText, baseUrl) {
  const value = String(baseUrl || "").trim();
  return setCodexTomlString(configText, "base_url", value, { providerScoped: true });
}

export function syncCodexConfigModel(configText, model) {
  return setCodexTomlString(configText, "model", String(model || "").trim(), { providerScoped: false });
}

export function syncCodexConfigContextWindow(configText, enabled, autoCompactTokenLimit = 900000) {
  let text = String(configText || "");
  if (enabled) {
    text = setCodexTopLevelInt(text, "model_context_window", 1000000);
    text = setCodexTopLevelInt(text, "model_auto_compact_token_limit", positiveInt(autoCompactTokenLimit, 900000));
    return text;
  }
  text = removeCodexTopLevelField(text, "model_context_window");
  return removeCodexTopLevelField(text, "model_auto_compact_token_limit");
}

export function summarizeCodexProvider(provider) {
  return {
    id: provider?.id || "",
    name: provider?.name || "未命名供应商",
    baseUrl: provider?.baseUrl || provider?.base_url || "",
    model: provider?.model || "",
    authText: provider?.authText || provider?.auth_text || "",
    renderedAuthText: provider?.renderedAuthText || provider?.rendered_auth_text || provider?.authText || "",
    configText: provider?.configText || provider?.config_text || "",
    renderedConfigText: provider?.renderedConfigText || provider?.rendered_config_text || provider?.configText || "",
    apiKeyMasked: provider?.apiKeyMasked || provider?.api_key_masked || maskSecret(provider?.apiKey),
    hasApiKey: Boolean(provider?.hasApiKey ?? provider?.has_api_key ?? provider?.apiKey),
    contextWindow1m: Boolean(provider?.contextWindow1m ?? provider?.context_window_1m),
    autoCompactTokenLimit: provider?.autoCompactTokenLimit ?? provider?.auto_compact_token_limit ?? null,
    active: Boolean(provider?.active),
    isOfficial: Boolean(provider?.isOfficial ?? provider?.is_official)
  };
}

export function codexProviderPresets() {
  return [
    {
      id: "openai",
      name: "OpenAI 官方",
      baseUrl: "",
      model: "",
      apiKey: "",
      authText: "{}",
      configText: "",
      isOfficial: true
    },
    {
      id: "custom",
      name: "自定义 Responses API",
      baseUrl: "https://example.com/v1",
      model: "gpt-5.4",
      apiKey: "",
      authText: buildCodexAuthText(""),
      configText: buildCodexConfigText({
        baseUrl: "https://example.com/v1",
        model: "gpt-5.4"
      }),
      isOfficial: false,
      contextWindow1m: false,
      autoCompactTokenLimit: null
    }
  ];
}

function escapeTomlString(value) {
  return String(value || "").replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function positiveInt(value, fallback) {
  const number = Number.parseInt(String(value ?? ""), 10);
  return Number.isFinite(number) && number > 0 ? number : fallback;
}

function setCodexTomlString(configText, fieldName, value, { providerScoped }) {
  const lines = String(configText || "").split("\n");
  if (lines.length === 1 && lines[0] === "") lines.pop();
  const replacement = `${fieldName} = "${escapeTomlString(value)}"`;
  const providerName = providerScoped ? codexModelProviderName(lines) : "";
  const range = providerName ? tomlSectionRange(lines, `model_providers.${providerName}`) : null;

  if (range) {
    const existing = assignmentLineIndex(lines, fieldName, range.start, range.end);
    if (!value) {
      if (existing !== -1) lines.splice(existing, 1);
      return finalizeTomlLines(lines);
    }
    if (existing !== -1) {
      lines[existing] = replacement;
    } else {
      lines.splice(range.end, 0, replacement);
    }
    return finalizeTomlLines(lines);
  }

  const topEnd = topLevelEndIndex(lines);
  const existing = assignmentLineIndex(lines, fieldName, 0, topEnd);
  if (!value) {
    if (existing !== -1) lines.splice(existing, 1);
    return finalizeTomlLines(lines);
  }
  if (existing !== -1) {
    lines[existing] = replacement;
  } else {
    lines.splice(topEnd, 0, replacement);
  }
  return finalizeTomlLines(lines);
}

function setCodexTopLevelInt(configText, fieldName, value) {
  const lines = String(configText || "").split("\n");
  if (lines.length === 1 && lines[0] === "") lines.pop();
  const topEnd = topLevelEndIndex(lines);
  const existing = assignmentLineIndex(lines, fieldName, 0, topEnd);
  const replacement = `${fieldName} = ${positiveInt(value, 900000)}`;
  if (existing !== -1) {
    lines[existing] = replacement;
  } else {
    lines.splice(topEnd, 0, replacement);
  }
  return finalizeTomlLines(lines);
}

function removeCodexTopLevelField(configText, fieldName) {
  const lines = String(configText || "").split("\n");
  const existing = assignmentLineIndex(lines, fieldName, 0, topLevelEndIndex(lines));
  if (existing !== -1) lines.splice(existing, 1);
  return finalizeTomlLines(lines);
}

function codexModelProviderName(lines) {
  const index = assignmentLineIndex(lines, "model_provider", 0, topLevelEndIndex(lines));
  if (index === -1) return "";
  const match = lines[index].match(/^\s*model_provider\s*=\s*["']([^"']+)["']/);
  return match?.[1] || "";
}

function tomlSectionRange(lines, sectionName) {
  const header = `[${sectionName}]`;
  const start = lines.findIndex((line) => line.trim() === header);
  if (start === -1) return null;
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (/^\s*\[[^\]]+\]\s*$/.test(lines[index])) {
      end = index;
      break;
    }
  }
  return { start: start + 1, end };
}

function topLevelEndIndex(lines) {
  const index = lines.findIndex((line) => /^\s*\[[^\]]+\]\s*$/.test(line));
  return index === -1 ? lines.length : index;
}

function assignmentLineIndex(lines, fieldName, start, end) {
  const pattern = new RegExp(`^\\s*${fieldName}\\s*=`);
  for (let index = start; index < end; index += 1) {
    if (pattern.test(lines[index])) return index;
  }
  return -1;
}

function finalizeTomlLines(lines) {
  while (lines.length && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines.length ? `${lines.join("\n")}\n` : "";
}

export function providerRowActions(provider) {
  const active = Boolean(provider?.active);
  return {
    canActivate: !active,
    canDelete: !active,
    deleteDisabledReason: active ? "使用中的供应商不能删除" : ""
  };
}

export function groupSessionsByProject(sessions) {
  const groups = new Map();
  for (const session of sortSessionsByActivity(sessions)) {
    const projectDir = normalizeThreadCwd(session?.projectDir);
    const key = projectDir || "__standalone__";
    if (!groups.has(key)) {
      groups.set(key, {
        key,
        name: projectDir ? lastPathPart(projectDir) : "独立会话",
        projectDir,
        sessions: []
      });
    }
    groups.get(key).sessions.push(session);
  }
  return [...groups.values()];
}

export function sortSessionsByActivity(sessions) {
  return [...(Array.isArray(sessions) ? sessions : [])].sort((left, right) => {
    const rightTime = sessionActivityTime(right);
    const leftTime = sessionActivityTime(left);
    if (rightTime !== leftTime) return rightTime - leftTime;
    return String(right?.sessionId || "").localeCompare(String(left?.sessionId || ""));
  });
}

function sessionActivityTime(session) {
  return Number(session?.lastActiveAt ?? session?.last_active_at ?? session?.updatedAt ?? session?.updated_at ?? session?.createdAt ?? session?.created_at ?? 0) || 0;
}

export function filterSessions(sessions, query) {
  const keyword = String(query || "").trim().toLowerCase();
  const sorted = sortSessionsByActivity(sessions);
  if (!keyword) return sorted;
  return sorted.filter((session) => {
    const haystack = [
      session?.sessionId,
      session?.title,
      session?.summary,
      session?.projectDir,
      session?.resumeCommand
    ]
      .filter(Boolean)
      .join("\n")
      .toLowerCase();
    return haystack.includes(keyword);
  });
}

export function mergeBotSettings(form, current = {}) {
  const token = String(form?.telegramBotToken || "").trim();
  return {
    telegramBotToken: token ? token : null,
    telegramAllowedUserId: String(form?.telegramAllowedUserId || current.telegramAllowedUserId || "").trim(),
    codexPath: String(form?.codexPath || current.codexPath || "codex").trim()
  };
}

function firstLine(value) {
  return String(value || "")
    .trim()
    .split(/\r?\n/, 1)[0]
    ?.trim();
}

function lastPathPart(value) {
  const parts = String(value || "").split("/").filter(Boolean);
  return parts.at(-1) || "未命名项目";
}

function normalizeThreadCwd(value) {
  const cwd = String(value || "").trim().replaceAll("\\", "/").replace(/\/+$/, "");
  if (!cwd || cwd.endsWith(STANDALONE_CWD_MARKER) || cwd.endsWith(COMPACT_STANDALONE_CWD_MARKER)) return "";
  if (isCodexDesktopChatCwd(cwd)) return "";
  return cwd;
}

function isCodexDesktopChatCwd(cwd) {
  const markerIndex = cwd.indexOf(CODEX_DESKTOP_CHAT_MARKER);
  if (markerIndex < 0) return false;
  const parts = cwd.slice(markerIndex + CODEX_DESKTOP_CHAT_MARKER.length).split("/").filter(Boolean);
  return parts.length === 2 && /^\d{4}-\d{2}-\d{2}$/.test(parts[0]);
}

function normalizeTurnsPayload(payload) {
  if (Array.isArray(payload)) return payload;
  return payload?.turns || payload?.items || payload?.data || [];
}

function messagesFromTurns(turns) {
  const messages = [];
  for (const turn of Array.isArray(turns) ? turns : []) {
    for (const item of Array.isArray(turn.items) ? turn.items : []) {
      const message = summarizeThreadItem(item, turn);
      if (message) {
        messages.push(message);
      }
    }
  }
  return messages;
}

function summarizeThreadItem(item, turn) {
  const type = String(item?.type || "");
  const role = String(item?.role || "");
  if (type === "userMessage" || role === "user" || type === "message" && role === "user") {
    return {
      id: item.id || `${turn?.id || "turn"}-user`,
      turnId: turn?.id || "",
      role: "user",
      text: textFromContent(item.content ?? item.text ?? item.message),
      ts: turn?.startedAt ?? turn?.started_at ?? null
    };
  }
  if (type === "agentMessage" || role === "assistant" || role === "agent" || type === "message" && role === "assistant") {
    return {
      id: item.id || `${turn?.id || "turn"}-assistant`,
      turnId: turn?.id || "",
      role: "assistant",
      text: textFromContent(item.text ?? item.content ?? item.message),
      ts: turn?.completedAt ?? turn?.completed_at ?? turn?.startedAt ?? turn?.started_at ?? null
    };
  }
  return null;
}

function textFromContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((entry) => entry?.text || entry?.content || entry?.url || entry?.path || "")
    .filter(Boolean)
    .join("\n")
    .trim();
}
