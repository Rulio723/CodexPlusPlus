/** Real-browser navigation regression without a Playwright/Python dependency. */
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { tmpdir } from "node:os";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const VITE = join(ROOT, "node_modules", "vite", "bin", "vite.js");
const BROWSERS = [
  process.env.CODEX_PLUS_BROWSER,
  join(process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft", "Edge", "Application", "msedge.exe"),
  join(process.env.ProgramFiles ?? "C:\\Program Files", "Microsoft", "Edge", "Application", "msedge.exe"),
  join(process.env.ProgramFiles ?? "C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe"),
];

const TAURI_FIXTURE = String.raw`
window.__codexPlusRouteErrors = [];
addEventListener("error", (event) => window.__codexPlusRouteErrors.push(String(event.error || event.message)));
addEventListener("unhandledrejection", (event) => window.__codexPlusRouteErrors.push(String(event.reason)));
window.__TAURI_INTERNALS__ = {
  invoke(command) {
    if (command === "read_live_context_entries") {
      return Promise.resolve({ status: "ok", message: "fixture", entries: { mcpServers: [], plugins: [] } });
    }
    if (command === "list_skills") {
      return Promise.resolve({ status: "ok", message: "fixture", codexHome: "", userSkillsDir: "", disabledSkillsDir: "", skills: [] });
    }
    if (command === "list_installed_skills") {
      return Promise.resolve({ status: "ok", message: "fixture", skills: [], repos: [] });
    }
    return Promise.reject(new Error("fixture has no response for " + command));
  },
  transformCallback() { return 1; },
  unregisterCallback() {},
  convertFileSrc(path) { return path; },
};
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
`;

function freePort() {
  return 41000 + Math.floor(Math.random() * 10000);
}

function browserPath() {
  const path = BROWSERS.find((candidate) => candidate && existsSync(candidate));
  if (!path) throw new Error("Windows Chrome or Edge is required; set CODEX_PLUS_BROWSER to its executable");
  return path;
}

async function eventually(label, fn, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`);
}

async function json(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return response.json();
}

async function reachable(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return true;
}

async function connectCdp(webSocketDebuggerUrl) {
  const socket = new WebSocket(webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", () => reject(new Error("CDP WebSocket connection failed")), { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(String(data));
    if (!message.id) return;
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    if (message.error) request.reject(new Error(`${message.error.message} (${request.method})`));
    else request.resolve(message.result);
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { method, resolve, reject });
    socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
  return { send, close: () => socket.close() };
}

async function evaluate(cdp, sessionId, expression) {
  const response = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  }, sessionId);
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.text ?? "page evaluation failed");
  return response.result.value;
}

async function stop(child) {
  if (!child || child.exitCode !== null) return;
  const exited = once(child, "exit");
  child.kill();
  await Promise.race([
    exited,
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
}

async function main() {
  const vitePort = freePort();
  const cdpPort = freePort();
  const browserProfile = await mkdtemp(join(tmpdir(), "codex-plus-manager-ui-navigation-cdp-"));
  const vite = spawn(process.execPath, [VITE, "--host", "127.0.0.1", "--port", String(vitePort), "--strictPort"], {
    cwd: ROOT, stdio: "ignore", windowsHide: true,
  });
  let browser;
  let cdp;
  try {
    await eventually("Vite", () => reachable(`http://127.0.0.1:${vitePort}/`));
    browser = spawn(browserPath(), [
      "--headless=new", `--remote-debugging-port=${cdpPort}`, "--remote-debugging-address=127.0.0.1",
      "--remote-allow-origins=*", "--no-first-run", "--no-default-browser-check", "--disable-gpu",
      `--user-data-dir=${browserProfile}`,
    ], { stdio: "ignore", windowsHide: true });
    const version = await eventually("browser CDP", () => json(`http://127.0.0.1:${cdpPort}/json/version`));
    cdp = await connectCdp(version.webSocketDebuggerUrl);
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    await cdp.send("Page.enable", {}, sessionId);
    await cdp.send("Runtime.enable", {}, sessionId);
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      width: 1440,
      height: 960,
      deviceScaleFactor: 1,
      mobile: false,
    }, sessionId);
    await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: TAURI_FIXTURE }, sessionId);
    await cdp.send("Page.navigate", { url: `http://127.0.0.1:${vitePort}` }, sessionId);
    try {
      await eventually("manager navigation", async () => (await evaluate(cdp, sessionId, "document.body?.innerText?.includes('工具与插件')")) === true);
    } catch (error) {
      const snapshot = await evaluate(cdp, sessionId, "({ text: document.body?.innerText?.slice(0, 2000), errors: window.__codexPlusRouteErrors, tauri: Boolean(window.__TAURI_INTERNALS__) })");
      throw new Error(`${error.message}; snapshot=${JSON.stringify(snapshot)}`);
    }
    for (const [label, expected, output] of [["工具与插件", "Codex 工具与插件", "context"], ["Skills 技能", "技能列表", "skills"]]) {
      const clicked = await evaluate(cdp, sessionId, `(() => {
        const button = [...document.querySelectorAll("button")].find((node) => node.innerText.trim() === ${JSON.stringify(label)} || node.getAttribute("aria-label") === ${JSON.stringify(label)});
        if (!button) throw new Error("missing navigation button: " + ${JSON.stringify(label)});
        button.click();
        return true;
      })()`);
      if (!clicked) throw new Error(`Could not click ${label}`);
      await eventually(expected, async () => (await evaluate(cdp, sessionId, `document.body?.innerText?.includes(${JSON.stringify(expected)})`)) === true);
      const errors = await evaluate(cdp, sessionId, "window.__codexPlusRouteErrors");
      if (errors?.length) throw new Error(`${label} raised: ${JSON.stringify(errors)}`);
      console.log(`${output}: ${expected}`);
    }
  } finally {
    try {
      await cdp?.send("Browser.close");
    } catch {
      // The browser may already be gone after an earlier harness failure.
    }
    await stop(browser);
    cdp?.close();
    await stop(vite);
    await rm(browserProfile, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}

await main();
