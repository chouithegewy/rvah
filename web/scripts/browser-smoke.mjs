import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = fileURLToPath(new URL("..", import.meta.url));
const appPort = Number(process.env.RVAH_SMOKE_PORT || 4174);
const debugPort = Number(process.env.RVAH_CHROME_DEBUG_PORT || 9334);
const appUrl = `http://127.0.0.1:${appPort}/`;
const chrome =
  process.env.CHROME_BIN ||
  (process.platform === "linux" ? "google-chrome" : "chrome");
const profileDir = mkdtempSync(join(tmpdir(), "rvah-web-smoke-"));
const mockMediaPath = join(profileDir, "mock-media.mp4");
writeFileSync(mockMediaPath, Buffer.from("synthetic mock media; not a real account or stream"));
const children = [];

function launch(command, args, options = {}) {
  const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"], ...options });
  children.push(child);
  child.stdout?.on("data", (chunk) => process.stderr.write(chunk));
  child.stderr?.on("data", (chunk) => process.stderr.write(chunk));
  return child;
}

async function waitFor(url, attempts = 100) {
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return response;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw lastError || new Error(`Timed out waiting for ${url}`);
}

async function waitForFile(path, attempts = 100) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (existsSync(path)) return path;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Timed out waiting for downloaded file ${path}`);
}

class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.exceptions = [];
  }

  async open() {
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      } else if (message.method === "Runtime.exceptionThrown") {
        this.exceptions.push(message.params.exceptionDetails.text);
      }
    });
  }

  call(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
    }
    return result.result.value;
  }

  close() {
    this.socket.close();
  }
}

try {
  launch(process.execPath, [
    "node_modules/vite/bin/vite.js",
    "preview",
    "--host",
    "127.0.0.1",
    "--port",
    String(appPort),
  ], { cwd: webRoot });
  await waitFor(appUrl);

  launch(chrome, [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    "--disable-dev-shm-usage",
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profileDir}`,
    appUrl,
  ]);
  await waitFor(`http://127.0.0.1:${debugPort}/json/version`);
  const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
  const page = pages.find((candidate) => candidate.type === "page" && candidate.url === appUrl);
  if (!page) throw new Error("Chrome did not open the RVAH page");

  const cdp = new Cdp(page.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.call("Runtime.enable");
  await cdp.call("Page.enable");
  await cdp.call("Browser.setDownloadBehavior", {
    behavior: "allow",
    downloadPath: profileDir,
    eventsEnabled: true,
  });

  const firstPass = await cdp.evaluate(`(async () => {
    const waitFor = async (predicate, label) => {
      for (let index = 0; index < 100; index += 1) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      throw new Error('timed out waiting for ' + label);
    };
    await waitFor(() => document.querySelector('#wasm-state')?.textContent.includes('0.1.0'), 'WASM');
    document.querySelector('#create-profile').click();
    await waitFor(() => document.querySelector('#profile-badge')?.textContent.includes('0 add-ons'), 'profile creation');
    const form = document.querySelector('#service-form');
    form.elements.addonId.value = 'com.example.browser-smoke';
    form.elements.sourceVersion.value = '1.0.0';
    form.elements.settings.value = JSON.stringify({ region: 'test' });
    form.elements.secrets.value = JSON.stringify({ api_token: 'never-render-browser-smoke-secret' });
    form.requestSubmit();
    await waitFor(() => document.querySelector('#profile-summary')?.textContent.includes('com.example.browser-smoke'), 'encrypted service');
    return {
      wasm: document.querySelector('#wasm-state').textContent,
      badge: document.querySelector('#profile-badge').textContent,
      summary: document.querySelector('#profile-summary').textContent,
      plaintextVisible: document.body.innerText.includes('never-render-browser-smoke-secret'),
    };
  })()`);
  if (!firstPass.wasm.includes("0.1.0")) throw new Error("WASM version was not rendered");
  if (!firstPass.badge.includes("1 add-on")) throw new Error("Encrypted add-on was not saved");
  if (!firstPass.summary.includes("api_token")) throw new Error("Redacted secret name is missing");
  if (firstPass.plaintextVisible) throw new Error("Secret plaintext leaked into the DOM");

  const mockNetworkPass = await cdp.evaluate(`(async () => {
    const waitFor = async (predicate, label) => {
      for (let index = 0; index < 200; index += 1) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      throw new Error('timed out waiting for ' + label);
    };
    document.querySelector('#bootstrap-url').value = 'mock://browser-smoke';
    document.querySelector('#veilid-connect').click();
    await waitFor(() => !document.querySelector('#veilid-publish').disabled, 'mock connection');
    document.querySelector('#veilid-publish').click();
    await waitFor(() => document.querySelector('#notice')?.textContent.includes('Publishing encrypted profile complete'), 'mock publish');
    const recordKey = document.querySelector('#record-key').value;
    if (!recordKey.startsWith('mock-record-')) throw new Error('mock record key was not assigned');
    document.querySelector('#veilid-fetch').click();
    await waitFor(() => document.querySelector('#notice')?.textContent.includes('Fetching encrypted profile complete'), 'mock fetch');
    return {
      connected: document.querySelector('#veilid-state').textContent,
      recordKey,
      profile: document.querySelector('#profile-summary').textContent,
    };
  })()`);
  if (!mockNetworkPass.connected.includes("ready") || !mockNetworkPass.profile.includes("epoch 1")) {
    throw new Error("Mock DHT publish/fetch did not complete");
  }

  await cdp.evaluate(`document.querySelector('#pair-request').click()`);
  const requestPath = await waitForFile(join(profileDir, "rvah-pair-request.json"));
  const requestInput = await cdp.call("Runtime.evaluate", {
    expression: "document.querySelector('#pair-request-file')",
  });
  await cdp.call("DOM.setFileInputFiles", {
    files: [requestPath],
    objectId: requestInput.result.objectId,
  });
  await cdp.evaluate(`(async () => {
    document.querySelector('#pair-request-file').dispatchEvent(new Event('change', { bubbles: true }));
    for (let index = 0; index < 100; index += 1) {
      if (!document.querySelector('#pair-respond').disabled) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    if (document.querySelector('#pair-respond').disabled) {
      throw new Error('pair response button stayed disabled');
    }
    document.querySelector('#pair-respond').click();
  })()`);
  await new Promise((resolve) => setTimeout(resolve, 250));
  const responseNotice = await cdp.evaluate(`document.querySelector('#notice')?.textContent || ''`);
  if (responseNotice.includes("failed:")) throw new Error(responseNotice);
  const responsePath = await waitForFile(join(profileDir, "rvah-pair-response.json"));
  const responseInput = await cdp.call("Runtime.evaluate", {
    expression: "document.querySelector('#pair-response-file')",
  });
  await cdp.call("DOM.setFileInputFiles", {
    files: [responsePath],
    objectId: responseInput.result.objectId,
  });
  const pairing = await cdp.evaluate(`(async () => {
    document.querySelector('#pair-response-file').dispatchEvent(new Event('change', { bubbles: true }));
    document.querySelector('#pair-accept').click();
    for (let index = 0; index < 100; index += 1) {
      const notice = document.querySelector('#notice')?.textContent || '';
      if (notice.includes('Accepting pairing response complete')) {
        return { accepted: true, notice };
      }
      if (notice.includes('failed:')) throw new Error(notice);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return { accepted: false, notice: document.querySelector('#notice')?.textContent || '' };
  })()`);
  if (!pairing.accepted) throw new Error("ECDH pairing response was not accepted");

  await cdp.call("Page.reload", { ignoreCache: true });
  const secondPass = await cdp.evaluate(`(async () => {
    for (let index = 0; index < 100; index += 1) {
      if (document.querySelector('#profile-summary')?.textContent.includes('com.example.browser-smoke')) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    await navigator.serviceWorker.ready;
    return {
      restored: document.querySelector('#profile-summary')?.textContent.includes('com.example.browser-smoke'),
      serviceWorkers: (await navigator.serviceWorker.getRegistrations()).length,
      notice: document.querySelector('#notice')?.textContent,
    };
  })()`);
  if (!secondPass.restored) throw new Error("IndexedDB enrollment did not survive reload");
  if (secondPass.serviceWorkers < 1) throw new Error("PWA service worker was not registered");

  await cdp.call("Page.navigate", { url: `${appUrl}relay/` });
  await new Promise((resolve) => setTimeout(resolve, 500));
  const relayBoot = await cdp.evaluate(`(async () => {
    for (let index = 0; index < 200; index += 1) {
      if (document.querySelector('#relay-create') && document.readyState === 'complete') {
        document.querySelector('#relay-bootstrap').value = 'mock://relay-smoke';
        document.querySelector('#relay-connect').click();
        for (let connected = 0; connected < 200; connected += 1) {
          if (!document.querySelector('#relay-create').disabled) return { loaded: true };
          const notice = document.querySelector('#relay-notice')?.textContent || '';
          if (notice.includes('failed:')) throw new Error(notice);
          await new Promise((resolve) => setTimeout(resolve, 25));
        }
        throw new Error('mock relay did not connect');
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return { loaded: false };
  })()`);
  if (!relayBoot.loaded) throw new Error("Relay operator did not initialize");
  const relayFile = await cdp.call("Runtime.evaluate", {
    expression: "document.querySelector('#relay-file')",
  });
  await cdp.call("DOM.setFileInputFiles", {
    files: [mockMediaPath],
    objectId: relayFile.result.objectId,
  });
  const relayPass = await cdp.evaluate(`(async () => {
    document.querySelector('#relay-asset').value = 'browser-smoke';
    document.querySelector('#relay-minutes').value = '1';
    document.querySelector('#relay-create').click();
    for (let index = 0; index < 200; index += 1) {
      const summary = document.querySelector('#relay-summary')?.textContent || '';
      const notice = document.querySelector('#relay-notice')?.textContent || '';
      if (summary.includes('Media ID: browser-smoke')) {
        return { active: true, state: document.querySelector('#relay-state').textContent, summary };
      }
      if (notice.includes('failed:')) throw new Error(notice);
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    return { active: false };
  })()`);
  if (!relayPass.active) throw new Error("Mock relay session was not created");
  if (cdp.exceptions.length) throw new Error(`Browser exceptions: ${cdp.exceptions.join("; ")}`);
  cdp.close();
  console.log(JSON.stringify({ firstPass, mockNetworkPass, pairing, secondPass, relayPass }, null, 2));
} finally {
  for (const child of children.reverse()) child.kill("SIGTERM");
  rmSync(profileDir, { recursive: true, force: true });
}
