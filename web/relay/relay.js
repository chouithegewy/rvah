import initWasm, { BrowserRelaySegment } from "../pkg/rvah_web.js";
import {
  connectVeilid,
  disconnectVeilid,
  isVeilidConnected,
  veilidTransport,
} from "../src/veilid.js";
import {
  FRAGMENT_BYTES,
  MAX_MEDIA_BYTES,
  decodeRequest,
  packFrame,
  requestKey,
  toBase64,
} from "../src/stream-transport.js";

const allowedMime = new Set(["video/mp4", "video/webm", "audio/mp4", "audio/webm", "audio/mpeg"]);
const elements = Object.fromEntries(
  [
    "relay-state", "relay-notice", "relay-bootstrap", "relay-connect", "relay-disconnect",
    "relay-asset", "relay-file", "relay-minutes", "relay-create", "relay-download",
    "relay-stop", "relay-summary",
  ].map((id) => [id, document.getElementById(id)]),
);

let segment = null;
let route = null;
let descriptor = null;
let sessionKey = null;
let requestAuthKey = null;
let expiryTimer = null;
let activeCalls = 0;

function notice(text, kind = "info") {
  elements["relay-notice"].textContent = text;
  elements["relay-notice"].dataset.kind = kind;
}

function errorText(error) {
  return error instanceof Error ? error.message : String(error);
}

async function run(label, action) {
  notice(`${label}…`);
  try {
    await action();
    notice(`${label} complete.`, "success");
  } catch (error) {
    console.error(label, error);
    notice(`${label} failed: ${errorText(error)}`, "error");
  }
}

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) =>
    byte.toString(16).padStart(2, "0")).join("");
}

function setConnected(connected) {
  elements["relay-connect"].disabled = connected;
  elements["relay-disconnect"].disabled = !connected;
  elements["relay-create"].disabled = !connected || Boolean(segment);
  elements["relay-state"].textContent = connected ? "Veilid connected" : "Offline";
  elements["relay-state"].className = `pill ${connected ? "ready" : "neutral"}`;
}

function refreshSession() {
  const active = Boolean(segment && descriptor);
  elements["relay-download"].disabled = !active;
  elements["relay-stop"].disabled = !active;
  elements["relay-create"].disabled = !isVeilidConnected() || active;
  elements["relay-summary"].textContent = active
    ? `Media ID: ${descriptor.assetId}\nBytes: ${descriptor.byteLength}\nSHA-256: ${descriptor.sha256}\nExpires: ${new Date(descriptor.expiresAt).toLocaleString()}\nRetention: memory only; waiting for verified receipt`
    : "No active session.";
}

function releaseSession(reason = "Session stopped; encrypted frames discarded.") {
  clearTimeout(expiryTimer);
  expiryTimer = null;
  segment?.free();
  segment = null;
  if (route && isVeilidConnected()) {
    try {
      veilidTransport().client.releasePrivateRoute(route.routeId);
    } catch (error) {
      console.warn("Could not release private route", error);
    }
  }
  route = null;
  descriptor = null;
  sessionKey?.fill(0);
  sessionKey = null;
  requestAuthKey = null;
  refreshSession();
  notice(reason, "success");
}

function replyBytes(payload) {
  const response = new Uint8Array(payload.length + 1);
  response[0] = 1;
  response.set(payload, 1);
  return response;
}

async function rejectCall(callId) {
  try {
    await veilidTransport().client.appCallReply(callId, new Uint8Array([0]));
  } catch (error) {
    console.warn("Could not reject relay call", error);
  }
}

async function handleCall(update) {
  if (!segment || !descriptor || !requestAuthKey || activeCalls >= 8) {
    await rejectCall(update.callId);
    return;
  }
  activeCalls += 1;
  let delivered = false;
  try {
    if (Date.now() >= descriptor.expiresAt) throw new Error("Session expired");
    if (update.routeId && update.routeId !== route.routeId.toString()) {
      throw new Error("Call arrived on a different route");
    }
    const request = await decodeRequest(update.message, requestAuthKey);
    if (!request || request.sessionId !== descriptor.sessionId) throw new Error("Wrong session");
    let payload;
    if (request.op === "frame") {
      if (!Number.isSafeInteger(request.index) || request.index < 0 || request.index >= segment.frame_count()
          || !Number.isSafeInteger(request.fragment) || request.fragment < 0 || request.fragment > 3) {
        throw new Error("Invalid frame request");
      }
      const packed = packFrame(JSON.parse(segment.frame(request.index)));
      const start = request.fragment * FRAGMENT_BYTES;
      payload = packed.slice(start, Math.min(start + FRAGMENT_BYTES, packed.length));
    } else if (request.op === "receipt") {
      if (typeof request.receipt !== "string" || request.receipt.length > 2048) {
        throw new Error("Invalid receipt");
      }
      segment.acknowledge(request.receipt);
      payload = new Uint8Array();
      delivered = true;
    } else {
      throw new Error("Unknown relay operation");
    }
    await veilidTransport().client.appCallReply(update.callId, replyBytes(payload));
    if (delivered) releaseSession("Client verified the media; relay memory discarded.");
  } catch (error) {
    console.warn("Rejected relay call", error);
    await rejectCall(update.callId);
    if (Date.now() >= (descriptor?.expiresAt || 0)) releaseSession("Session expired; relay memory discarded.");
  } finally {
    activeCalls -= 1;
  }
}

function onVeilidUpdate(update) {
  if (update.kind === "AppCall") void handleCall(update);
  if (update.kind === "Shutdown") setConnected(false);
  if (update.kind === "Attachment") {
    elements["relay-state"].textContent = update.publicInternetReady
      ? "Veilid ready"
      : `Veilid ${update.state}`;
  }
}

elements["relay-connect"].addEventListener("click", () =>
  run("Connecting to Veilid", async () => {
    const bootstrap = elements["relay-bootstrap"].value.trim();
    if (!bootstrap) throw new Error("Enter a direct WebSocket bootstrap URL");
    localStorage.setItem("rvah.veilid.bootstrap", bootstrap);
    await connectVeilid(bootstrap, onVeilidUpdate);
    setConnected(true);
  }),
);

elements["relay-disconnect"].addEventListener("click", () =>
  run("Disconnecting Veilid", async () => {
    if (segment) releaseSession();
    await disconnectVeilid();
    setConnected(false);
  }),
);

elements["relay-create"].addEventListener("click", () =>
  run("Creating private relay session", async () => {
    if (!isVeilidConnected()) throw new Error("Connect to Veilid first");
    if (segment) throw new Error("Stop the current session first");
    const file = elements["relay-file"].files?.[0];
    const assetId = elements["relay-asset"].value.trim();
    const minutes = Number(elements["relay-minutes"].value);
    if (!file || !file.size || file.size > MAX_MEDIA_BYTES) {
      throw new Error("Choose a nonempty media file no larger than 16 MiB");
    }
    if (!allowedMime.has(file.type)) throw new Error("Choose finite MP4, WebM, MP3, or MP4/WebM audio");
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(assetId)) throw new Error("Media ID must use 1–128 letters, digits, _ or -");
    if (!Number.isInteger(minutes) || minutes < 1 || minutes > 30) throw new Error("Lifetime must be 1–30 minutes");

    const bytes = new Uint8Array(await file.arrayBuffer());
    const key = crypto.getRandomValues(new Uint8Array(32));
    const sessionId = randomHex(16);
    try {
      segment = new BrowserRelaySegment(sessionId, key, bytes);
    } finally {
      bytes.fill(0);
    }
    try {
      route = await veilidTransport().client.newPrivateRoute();
      requestAuthKey = await requestKey(key);
      sessionKey = key;
      descriptor = {
        version: 1,
        assetId,
        sessionId,
        sha256: segment.digest(),
        byteLength: file.size,
        expiresAt: Date.now() + minutes * 60_000,
        mime: file.type,
        key: toBase64(key),
        routeBlob: toBase64(route.blob),
      };
      elements["relay-file"].value = "";
      expiryTimer = setTimeout(
        () => releaseSession("Session expired; relay memory discarded."),
        descriptor.expiresAt - Date.now(),
      );
      refreshSession();
    } catch (error) {
      segment?.free();
      segment = null;
      key.fill(0);
      if (route) {
        try {
          veilidTransport().client.releasePrivateRoute(route.routeId);
        } catch (releaseError) {
          console.warn("Could not release failed private route", releaseError);
        }
      }
      route = null;
      throw error;
    }
  }),
);

elements["relay-download"].addEventListener("click", () => {
  if (!descriptor) return;
  const blob = new Blob([JSON.stringify(descriptor)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `rvah-media-${descriptor.assetId}.json`;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1_000);
});

elements["relay-stop"].addEventListener("click", () => releaseSession());

window.addEventListener("beforeunload", () => {
  if (segment) releaseSession();
  if (isVeilidConnected()) void disconnectVeilid();
});

await initWasm();
elements["relay-bootstrap"].value = import.meta.env.VITE_VEILID_BOOTSTRAP
  || localStorage.getItem("rvah.veilid.bootstrap") || "";
setConnected(false);
refreshSession();
