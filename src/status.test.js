import test from "node:test";
import assert from "node:assert/strict";
import {
  buildCodexAuthText,
  buildCodexConfigText,
  codexProviderPresets,
  fetchAppServerThread,
  fetchAppServerThreads,
  filterSessions,
  groupSessionsByProject,
  groupThreadsForMenu,
  maskSecret,
  mergeBotSettings,
  normalizeThreadStatus,
  providerRowActions,
  summarizeAppServerSession,
  summarizeCodexProvider,
  sortSessionsByActivity,
  summarizeAppServerStatus,
  summarizeAppServerThread,
  summarizeAppServerThreads,
  syncCodexConfigBaseUrl,
  syncCodexConfigContextWindow,
  syncCodexConfigModel,
  summarizeThreadTurns
} from "./status.js";
import {
  bindWindowDragging,
  nextWindowDragPosition,
  shouldStartWindowDrag,
  WINDOW_DRAG_SELECTOR,
  WINDOW_NO_DRAG_SELECTOR
} from "./window-drag.js";

function fakeTarget(matchesNoDrag = false) {
  return {
    closest(selector) {
      if (selector === WINDOW_NO_DRAG_SELECTOR && matchesNoDrag) return {};
      return null;
    }
  };
}

test("shouldStartWindowDrag 只允许左键从非控件区域拖动", () => {
  assert.equal(shouldStartWindowDrag({ button: 0, target: fakeTarget(false) }), true);
  assert.equal(shouldStartWindowDrag({ button: 1, target: fakeTarget(false) }), false);
  assert.equal(shouldStartWindowDrag({ button: 0, target: fakeTarget(true) }), false);
});

test("normalizeThreadStatus 兼容官方对象状态", () => {
  assert.equal(normalizeThreadStatus("running"), "running");
  assert.equal(normalizeThreadStatus({ type: "active", activeFlags: ["waitingOnApproval"] }), "active:waitingOnApproval");
  assert.equal(normalizeThreadStatus({ type: "idle" }), "idle");
});

test("nextWindowDragPosition 根据鼠标屏幕坐标偏移计算窗口位置", () => {
  assert.deepEqual(
    nextWindowDragPosition({ pointerX: 100, pointerY: 200, windowX: 20, windowY: 30 }, { screenX: 135, screenY: 210 }),
    { x: 55, y: 40 }
  );
});

test("bindWindowDragging 绑定统一拖动区域并忽略控件", async () => {
  const listeners = [];
  const root = {
    querySelectorAll(selector) {
      assert.equal(selector, WINDOW_DRAG_SELECTOR);
      return [
        {
          addEventListener(type, listener) {
            assert.equal(type, "mousedown");
            listeners.push(listener);
          }
        }
      ];
    }
  };
  let dragCount = 0;
  let moveCount = 0;
  let preventDefaultCount = 0;
  const originalAddEventListener = globalThis.addEventListener;
  const originalRemoveEventListener = globalThis.removeEventListener;
  const globalListeners = new Map();

  globalThis.addEventListener = (type, listener) => {
    globalListeners.set(type, listener);
  };
  globalThis.removeEventListener = (type) => {
    globalListeners.delete(type);
  };

  bindWindowDragging(root, {
    start: () => {
      dragCount += 1;
      return Promise.resolve({ windowX: 20, windowY: 30 });
    },
    move: (position) => {
      moveCount += 1;
      assert.deepEqual(position, { x: 30, y: 40 });
      return Promise.resolve();
    }
  });
  await listeners[0]({
    button: 0,
    screenX: 100,
    screenY: 200,
    target: fakeTarget(false),
    preventDefault: () => { preventDefaultCount += 1; }
  });
  await listeners[0]({
    button: 0,
    screenX: 100,
    screenY: 200,
    target: fakeTarget(true),
    preventDefault: () => { preventDefaultCount += 1; }
  });
  globalListeners.get("mousemove")({ screenX: 110, screenY: 210 });
  globalListeners.get("mouseup")();

  globalThis.addEventListener = originalAddEventListener;
  globalThis.removeEventListener = originalRemoveEventListener;

  assert.equal(dragCount, 1);
  assert.equal(moveCount, 1);
  assert.equal(preventDefaultCount, 1);
});

test("fetchAppServerThreads 在非 Tauri 环境返回空列表", async () => {
  const threads = await fetchAppServerThreads();

  assert.deepEqual(threads, []);
});

test("fetchAppServerThread 在非 Tauri 环境返回 null", async () => {
  const thread = await fetchAppServerThread("thread-1");

  assert.equal(thread, null);
});

test("summarizeAppServerThreads 使用 preview 首行作为标题", () => {
  const threads = summarizeAppServerThreads([
    {
      id: "thread-1",
      cwd: "/work/demo",
      preview: "第一行\n第二行",
      updatedAt: 100
    }
  ]);

  assert.equal(threads[0].title, "第一行");
  assert.equal(threads[0].cwd, "/work/demo");
  assert.equal(threads[0].updatedAt, 100);
});

test("summarizeAppServerSession 使用 app-server 字段生成会话条目", () => {
  const session = summarizeAppServerSession({
    id: "thread-1",
    name: "用户命名",
    preview: "第一条消息",
    cwd: "/Users/me/Documents/Codex/2026-04-25/new-chat",
    createdAt: 100,
    updatedAt: 300
  });

  assert.deepEqual(
    {
      sessionId: session.sessionId,
      title: session.title,
      summary: session.summary,
      projectDir: session.projectDir,
      createdAt: session.createdAt,
      lastActiveAt: session.lastActiveAt,
      resumeCommand: session.resumeCommand
    },
    {
      sessionId: "thread-1",
      title: "用户命名",
      summary: "第一条消息",
      projectDir: "",
      createdAt: 100,
      lastActiveAt: 300,
      resumeCommand: "codex resume thread-1"
    }
  );
});

test("summarizeAppServerStatus 汇总 app-server 状态", () => {
  const status = summarizeAppServerStatus([
    { id: "thread-1", cwd: "/work/a", updatedAt: 100 },
    { id: "thread-2", cwd: "/work/a", updatedAt: 300 },
    { id: "thread-3", cwd: "", updatedAt: 200 }
  ]);

  assert.equal(status.connected, true);
  assert.equal(status.threadCount, 3);
  assert.equal(status.projectCount, 1);
  assert.equal(status.latestUpdatedAt, 300);
});


test("groupThreadsForMenu 按项目和独立对话分组", () => {
  const menu = groupThreadsForMenu([
    { id: "p1-s1", title: "session-1", cwd: "/work/codex-bot", updatedAt: 3 },
    { id: "solo-1", title: "session1", cwd: "", updatedAt: 4 },
    {
      id: "solo-hidden",
      title: "session-hidden",
      cwd: "/Users/me/Library/Application Support/CodexManager/standalone",
      updatedAt: 5
    },
    {
      id: "solo-codex-desktop",
      title: "session-desktop",
      cwd: "/Users/me/Documents/Codex/2026-04-25/new-chat",
      updatedAt: 6
    },
    { id: "p2-s1", title: "session-1", cwd: "/work/test2", updatedAt: 2 },
    { id: "p1-s2", title: "session-2", cwd: "/work/codex-bot", updatedAt: 1 },
    { id: "solo-2", title: "session2", cwd: null, updatedAt: 0 }
  ]);

  assert.deepEqual(
    menu.projects.map((project) => [project.name, project.threads.map((thread) => thread.id)]),
    [
      ["codex-bot", ["p1-s1", "p1-s2"]],
      ["test2", ["p2-s1"]]
    ]
  );
  assert.deepEqual(menu.standalone.map((thread) => thread.id), ["solo-1", "solo-hidden", "solo-codex-desktop", "solo-2"]);
});

test("summarizeAppServerThread 会提取用户和 Codex 消息流", () => {
  const thread = summarizeAppServerThread({
    thread: {
      id: "thread-1",
      cwd: "/work/demo",
      preview: "继续改 UI",
      turns: [
        {
          id: "turn-1",
          status: "completed",
          items: [
            {
              id: "item-1",
              type: "userMessage",
              content: [{ type: "text", text: "你好" }]
            },
            {
              id: "item-2",
              type: "agentMessage",
              text: "你好，我在。"
            },
            {
              id: "item-3",
              type: "reasoning",
              summary: "内部推理摘要"
            }
          ]
        }
      ]
    }
  });

  assert.equal(thread.id, "thread-1");
  assert.equal(thread.messages.length, 2);
  assert.deepEqual(
    thread.messages.map((message) => [message.role, message.text]),
    [
      ["user", "你好"],
      ["assistant", "你好，我在。"]
    ]
  );
});

test("summarizeThreadTurns 支持分页 turns 和 role 内容结构", () => {
  const page = summarizeThreadTurns({
    turns: [
      {
        id: "turn-new",
        items: [
          { id: "item-3", role: "user", content: [{ type: "input_text", text: "继续" }] },
          { id: "item-4", role: "assistant", content: [{ type: "output_text", text: "继续完成" }] }
        ]
      },
      {
        id: "turn-old",
        items: [
          { id: "item-1", type: "userMessage", content: [{ type: "text", text: "你好" }] },
          { id: "item-2", type: "agentMessage", text: "你好，我在。" }
        ]
      }
    ]
  });

  assert.deepEqual(
    page.messages.map((message) => [message.role, message.text]),
    [
      ["user", "你好"],
      ["assistant", "你好，我在。"],
      ["user", "继续"],
      ["assistant", "继续完成"]
    ]
  );
});

test("maskSecret 会遮蔽敏感值但保留可识别尾部", () => {
  assert.equal(maskSecret(""), "");
  assert.equal(maskSecret("123456"), "******");
  assert.equal(maskSecret("1234567890abcdef"), "************cdef");
});

test("summarizeCodexProvider 不暴露 apiKey 明文", () => {
  const provider = summarizeCodexProvider({
    id: "demo",
    name: "Demo",
    baseUrl: "https://example.com/v1",
    model: "gpt-5.4",
    apiKey: "sk-secret-value",
    apiKeyMasked: "********alue",
    active: true
  });

  assert.equal(provider.apiKey, undefined);
  assert.equal(provider.apiKeyMasked, "********alue");
  assert.equal(provider.active, true);
});

test("providerRowActions 禁止删除使用中的供应商", () => {
  assert.deepEqual(providerRowActions({ active: true }), {
    canActivate: false,
    canDelete: false,
    deleteDisabledReason: "使用中的供应商不能删除"
  });
});

test("providerRowActions 允许删除未使用的供应商", () => {
  assert.deepEqual(providerRowActions({ active: false }), {
    canActivate: true,
    canDelete: true,
    deleteDisabledReason: ""
  });
});

test("codexProviderPresets 提供基础 Codex 供应商模板", () => {
  const presets = codexProviderPresets();

  assert.deepEqual(presets.map((preset) => preset.id), ["openai", "custom"]);
  assert.equal(presets[0].isOfficial, true);
  assert.equal(presets[1].isOfficial, false);
  assert.ok(presets.every((preset) => preset.name));
});

test("buildCodexAuthText 生成 Codex auth.json 文本", () => {
  assert.equal(buildCodexAuthText("sk-demo"), "{\n  \"OPENAI_API_KEY\": \"sk-demo\"\n}");
  assert.equal(buildCodexAuthText(""), "{}");
});

test("buildCodexConfigText 生成自定义 Responses 配置", () => {
  const text = buildCodexConfigText({
    baseUrl: "https://example.com/v1",
    model: "gpt-5.4",
    contextWindow1m: true,
    autoCompactTokenLimit: 850000
  });

  assert.match(text, /model_provider = "custom"/);
  assert.match(text, /model = "gpt-5.4"/);
  assert.match(text, /model_context_window = 1000000/);
  assert.match(text, /model_auto_compact_token_limit = 850000/);
  assert.match(text, /\[model_providers\.custom\]/);
  assert.ok(text.includes('base_url = "https://example.com/v1"'));
});

test("Codex config 同步工具更新模型、地址和上下文窗口字段", () => {
  let text = buildCodexConfigText({
    baseUrl: "https://one.example/v1",
    model: "gpt-5.4"
  });

  text = syncCodexConfigBaseUrl(text, "https://two.example/v1");
  text = syncCodexConfigModel(text, "gpt-5.5");
  text = syncCodexConfigContextWindow(text, true, 900000);

  assert.match(text, /model = "gpt-5.5"/);
  assert.ok(text.includes('base_url = "https://two.example/v1"'));
  assert.match(text, /model_context_window = 1000000/);
  assert.match(text, /model_auto_compact_token_limit = 900000/);

  text = syncCodexConfigContextWindow(text, false, 900000);

  assert.doesNotMatch(text, /model_context_window/);
  assert.doesNotMatch(text, /model_auto_compact_token_limit/);
});

test("groupSessionsByProject 按项目目录组织 Codex 会话", () => {
  const groups = groupSessionsByProject([
    { sessionId: "s1", projectDir: "/work/codex-bot", createdAt: 30, lastActiveAt: 100 },
    { sessionId: "s2", projectDir: "", createdAt: 40, lastActiveAt: 300 },
    { sessionId: "s3", projectDir: "/work/codex-bot", createdAt: 10, lastActiveAt: 200 },
    {
      sessionId: "s4",
      projectDir: "/Users/me/Library/Application Support/CodexManager/standalone",
      createdAt: 50,
      lastActiveAt: 250
    },
    {
      sessionId: "s5",
      projectDir: "/Users/me/Documents/Codex/2026-04-25/new-chat",
      createdAt: 60,
      lastActiveAt: 350
    }
  ]);

  assert.deepEqual(
    groups.map((group) => [group.name, group.sessions.map((session) => session.sessionId)]),
    [
      ["独立会话", ["s5", "s2", "s4"]],
      ["codex-bot", ["s3", "s1"]]
    ]
  );
});

test("sortSessionsByActivity 按最后活跃时间而不是创建时间排序", () => {
  const sessions = sortSessionsByActivity([
    { sessionId: "old-created-new-active", createdAt: 1, lastActiveAt: 300 },
    { sessionId: "new-created-old-active", createdAt: 500, lastActiveAt: 100 },
    { sessionId: "fallback-created", createdAt: 200 }
  ]);

  assert.deepEqual(
    sessions.map((session) => session.sessionId),
    ["old-created-new-active", "fallback-created", "new-created-old-active"]
  );
});

test("filterSessions 可按标题、摘要和项目目录过滤", () => {
  const sessions = [
    { sessionId: "s1", title: "修复登录", summary: "done", projectDir: "/work/app" },
    { sessionId: "s2", title: "整理文档", summary: "readme", projectDir: "/work/docs" }
  ];

  assert.deepEqual(filterSessions(sessions, "readme").map((session) => session.sessionId), ["s2"]);
  assert.deepEqual(filterSessions(sessions, "app").map((session) => session.sessionId), ["s1"]);
});

test("mergeBotSettings 保存空 token 时沿用当前 token", () => {
  const merged = mergeBotSettings(
    {
      telegramBotToken: "",
      telegramAllowedUserId: "100",
      codexPath: "/usr/bin/codex"
    },
    {
      hasTelegramBotToken: true,
      telegramBotTokenMasked: "********abcd",
      telegramAllowedUserId: "1",
      codexPath: "/old/codex"
    }
  );

  assert.equal(merged.telegramBotToken, null);
  assert.equal(merged.telegramAllowedUserId, "100");
  assert.equal(merged.codexPath, "/usr/bin/codex");
});
