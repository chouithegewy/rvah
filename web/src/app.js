import initWasm, {
  PreparedSnapshot,
  assemble_snapshot,
  create_empty_profile,
  format_version,
  generate_profile_key,
  inspect_profile,
  inspect_snapshot_head,
  put_addon_profile,
  addon_profile_settings,
  receive_relay_segment,
  rekey_profile,
  snapshot_subkey_count,
} from "../pkg/rvah_web.js";
import { AddonClient } from "./addon-client.js";
import { fetchMedia, validateSource } from "./playback.js";
import { receiveMedia, validateDescriptor } from "./stream-transport.js";
import {
  forgetDeviceProfile,
  loadDeviceProfile,
  saveDeviceProfile,
} from "./storage.js";
import {
  acceptPairingResponse,
  createPairingRequest,
  createPairingResponse,
} from "./pairing.js";
import {
  connectVeilid,
  disconnectVeilid,
  fetchSnapshot,
  isVeilidConnected,
  publishSnapshot,
  veilidTransport,
} from "./veilid.js";

const elements = Object.fromEntries(
  [
    "notice",
    "wasm-state",
    "veilid-state",
    "profile-badge",
    "profile-summary",
    "profile-file",
    "key-file",
    "create-profile",
    "import-profile",
    "download-profile",
    "download-key",
    "rotate-key",
    "forget-device",
    "service-form",
    "pair-request",
    "pair-request-file",
    "pair-respond",
    "pair-response-file",
    "pair-accept",
    "bootstrap-url",
    "record-key",
    "veilid-connect",
    "veilid-disconnect",
    "veilid-publish",
    "veilid-fetch",
    "addon-state",
    "addon-package",
    "addon-signature",
    "addon-pubkey",
    "addon-inspect",
    "addon-permissions",
    "addon-approve",
    "addon-unload",
    "addon-settings",
    "addon-profile-settings",
    "media-browse",
    "media-query",
    "media-search",
    "media-catalog",
    "media-more",
    "media-session",
    "media-import-session",
    "media-session-state",
    "media-status",
    "media-video",
    "media-stop",
  ].map((id) => [id, document.getElementById(id)]),
);

let current = null;
let summary = null;
let addonClient = null;
let addonManifest = null;
let addonGrants = [];
let catalogRequest = null;
let catalogCursor = null;
let mediaAbort = null;
let mediaObjectUrl = null;
const mediaSessions = new Map();

function message(text, kind = "info") {
  elements.notice.textContent = text;
  elements.notice.dataset.kind = kind;
}

function errorText(error) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error?.message) return String(error.message);
  return JSON.stringify(error);
}

async function run(label, action) {
  message(`${label}…`);
  try {
    const result = await action();
    message(`${label} complete.`, "success");
    return result;
  } catch (error) {
    console.error(label, error);
    message(`${label} failed: ${errorText(error)}`, "error");
    return undefined;
  }
}

function download(name, bytes, type = "application/octet-stream") {
  const blob = new Blob([bytes], { type });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1_000);
}

async function readJsonFile(input, maximumBytes = 2 * 1024 * 1024) {
  const file = input.files?.[0];
  if (!file) throw new Error("Choose a JSON file first");
  if (file.size > maximumBytes) throw new Error("Selected JSON file is too large");
  return JSON.parse(await file.text());
}

async function readHexFile(input, bytes, label) {
  const file = input.files?.[0];
  if (!file) throw new Error(`Choose the ${label} first`);
  if (file.size > bytes * 2 + 32) throw new Error(`${label} file is too large`);
  const value = (await file.text()).trim().toLowerCase();
  if (value.length !== bytes * 2 || !/^[0-9a-f]+$/.test(value)) {
    throw new Error(`${label} must contain exactly ${bytes * 2} hexadecimal characters`);
  }
  return Uint8Array.from(value.match(/../g), (pair) => Number.parseInt(pair, 16));
}

async function readPackage() {
  const file = elements["addon-package"].files?.[0];
  if (!file) throw new Error("Choose a signed .rvah package first");
  if (!file.size || file.size > 16 * 1024 * 1024) {
    throw new Error("Add-on package must be between 1 byte and 16 MiB");
  }
  return new Uint8Array(await file.arrayBuffer());
}

function renderProfile() {
  const enrolled = Boolean(current && summary);
  elements["profile-badge"].textContent = enrolled
    ? `${summary.addons.length} add-on${summary.addons.length === 1 ? "" : "s"}`
    : "No device profile";
  elements["profile-badge"].className = `pill ${enrolled ? "ready" : "neutral"}`;
  for (const id of ["download-profile", "download-key", "rotate-key", "forget-device"]) {
    elements[id].disabled = !enrolled;
  }
  elements["service-form"].querySelector("button[type=submit]").disabled = !enrolled;
  elements["pair-respond"].disabled = !enrolled || !elements["pair-request-file"].files?.length;
  elements["veilid-publish"].disabled = !enrolled || !isVeilidConnected();

  if (!enrolled) {
    elements["profile-summary"].className = "empty-state";
    elements["profile-summary"].textContent = "No profile is enrolled in this browser.";
    return;
  }

  elements["record-key"].value = current.enrollment.recordKey || "";
  elements["profile-summary"].className = "profile-list";
  elements["profile-summary"].replaceChildren();
  const meta = document.createElement("div");
  meta.className = "profile-meta";
  const created = new Date(Number(summary.createdAtUnix) * 1_000);
  meta.textContent = `Format ${summary.formatVersion} · created ${created.toLocaleString()} · epoch ${current.enrollment.epoch || 0}`;
  elements["profile-summary"].append(meta);

  if (!summary.addons.length) {
    const empty = document.createElement("p");
    empty.className = "muted";
    empty.textContent = "The encrypted profile is empty. Add a service configuration below.";
    elements["profile-summary"].append(empty);
    return;
  }

  for (const addon of summary.addons) {
    const article = document.createElement("article");
    const title = document.createElement("h3");
    title.textContent = addon.id;
    const version = document.createElement("span");
    version.textContent = addon.sourceVersion;
    const details = document.createElement("p");
    details.textContent = `${addon.settingNames.length} setting(s) · ${addon.secretNames.length} secret(s): ${addon.secretNames.join(", ") || "none"}`;
    article.append(title, version, details);
    elements["profile-summary"].append(article);
  }
}

async function refreshSummary() {
  if (!current) {
    summary = null;
  } else {
    summary = inspect_profile(current.bundle, current.enrollment.profileKeyHex);
  }
  renderProfile();
}

async function persist(bundle = current.bundle, enrollment = current.enrollment) {
  current = { bundle: new Uint8Array(bundle), enrollment };
  await saveDeviceProfile(current.bundle, current.enrollment);
  await refreshSummary();
}

function setAddonState(text, state = "neutral") {
  elements["addon-state"].textContent = text;
  elements["addon-state"].className = `pill ${state}`;
}

function setAddonActions(approved) {
  elements["addon-approve"].disabled = !addonManifest || approved;
  elements["addon-unload"].disabled = !addonManifest;
  for (const id of ["addon-profile-settings", "media-browse", "media-search"]) {
    elements[id].disabled = !approved;
  }
}

function stopPlayback() {
  mediaAbort?.abort();
  mediaAbort = null;
  const video = elements["media-video"];
  video.pause();
  video.removeAttribute("src");
  video.load();
  if (mediaObjectUrl) URL.revokeObjectURL(mediaObjectUrl);
  mediaObjectUrl = null;
  elements["media-status"].textContent = "Playback stopped; in-memory media released.";
}

function unloadAddon() {
  stopPlayback();
  addonClient?.close();
  addonClient = null;
  addonManifest = null;
  addonGrants = [];
  catalogRequest = null;
  catalogCursor = null;
  mediaSessions.clear();
  elements["addon-permissions"].textContent = "";
  elements["media-catalog"].replaceChildren();
  elements["media-more"].hidden = true;
  elements["media-session-state"].textContent = "";
  setAddonActions(false);
  setAddonState("No add-on loaded");
}

function addonSettings() {
  const parsed = JSON.parse(elements["addon-settings"].value || "{}");
  if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Add-on settings must be a JSON object");
  }
  return parsed;
}

async function invokeAddon(operation) {
  if (!addonClient || !addonManifest || !addonGrants.length) {
    throw new Error("Verify and approve a signed add-on first");
  }
  return addonClient.call({
    action: "invoke",
    operation,
    grants: addonGrants,
    settings: addonSettings(),
  });
}

function checkedPage(response) {
  if (response?.kind !== "page" || !Array.isArray(response.value?.items)) {
    throw new Error("Add-on returned an invalid catalog page");
  }
  if (response.value.items.length > 200) throw new Error("Add-on page exceeds 200 items");
  const items = response.value.items.map((item) => {
    if (!item || !["directory", "media"].includes(item.kind)
        || typeof item.id !== "string" || !item.id || item.id.length > 4096
        || typeof item.title !== "string" || !item.title.trim() || item.title.length > 512) {
      throw new Error("Add-on page contains an invalid item");
    }
    if (item.kind === "directory" && (typeof item.path !== "string" || item.path.length > 4096)) {
      throw new Error("Add-on page contains an invalid directory");
    }
    return item;
  });
  const cursor = response.value.cursor_next;
  if (cursor != null && (typeof cursor !== "string" || cursor.length > 4096)) {
    throw new Error("Add-on returned an invalid page cursor");
  }
  return { items, cursor: cursor || null };
}

function renderCatalog(items, append) {
  if (!append) elements["media-catalog"].replaceChildren();
  for (const item of items) {
    const article = document.createElement("article");
    const title = document.createElement("h3");
    title.textContent = item.title;
    const kind = document.createElement("span");
    kind.textContent = item.kind;
    const note = document.createElement("p");
    note.textContent = item.note || item.subtitle || item.content_type || "";
    const open = document.createElement("button");
    open.textContent = item.kind === "directory" ? "Open" : "Resolve and play";
    open.addEventListener("click", () => {
      if (item.kind === "directory") {
        void loadCatalog({ op: "list", path: item.path, cursor: null }, false);
      } else {
        void resolveAndPlay(item);
      }
    });
    article.append(title, kind, note, open);
    elements["media-catalog"].append(article);
  }
}

async function loadCatalog(request, append) {
  return run(append ? "Loading more catalog items" : "Loading catalog", async () => {
    const operation = { ...request, cursor: append ? catalogCursor : null };
    const page = checkedPage(await invokeAddon(operation));
    catalogRequest = { ...request, cursor: null };
    catalogCursor = page.cursor;
    renderCatalog(page.items, append);
    elements["media-more"].hidden = !catalogCursor;
    elements["media-status"].textContent = page.items.length
      ? `${append ? "Added" : "Loaded"} ${page.items.length} item(s).`
      : "The add-on returned no items.";
  });
}

async function playSource(source) {
  const checked = validateSource(source, addonManifest.permissions.network);
  stopPlayback();
  mediaAbort = new AbortController();
  elements["media-status"].textContent = "Loading bounded media into memory…";
  let blob;
  if (checked.kind === "http") {
    blob = await fetchMedia(checked.url, mediaAbort.signal);
  } else {
    const descriptor = mediaSessions.get(checked.mediaId);
    if (!descriptor) throw new Error(`Import the private session for media ID ${checked.mediaId}`);
    const received = await receiveMedia(descriptor, {
      ...veilidTransport(),
      codec: { receive_relay_segment },
      signal: mediaAbort.signal,
      onProgress: (loaded, total) => {
        elements["media-status"].textContent = `Receiving and verifying ${loaded} / ${total} bytes…`;
      },
    });
    blob = new Blob([received.bytes], { type: received.mime });
  }
  if (mediaAbort.signal.aborted) throw new DOMException("Playback cancelled", "AbortError");
  mediaObjectUrl = URL.createObjectURL(blob);
  const video = elements["media-video"];
  video.src = mediaObjectUrl;
  elements["media-status"].textContent = `Verified ${blob.size} bytes in memory. Playback is ready.`;
  await video.play().catch(() => {
    elements["media-status"].textContent = `Verified ${blob.size} bytes in memory. Press Play to begin.`;
  });
}

async function resolveAndPlay(item) {
  return run(`Resolving ${item.title}`, async () => {
    const response = await invokeAddon({ op: "resolve", item_id: item.id, source_id: null });
    if (response?.kind !== "resolve" || !response.value) {
      throw new Error("Add-on returned an invalid resolve response");
    }
    if (response.value.kind === "auth_required") {
      throw new Error(`Account required: ${response.value.value?.provider_hint || "link in a native host"}`);
    }
    if (response.value.kind === "unavailable") {
      throw new Error(response.value.value?.reason || "Media is unavailable");
    }
    const sources = response.value.kind === "playables" ? response.value.value : null;
    if (!Array.isArray(sources) || !sources.length || sources.length > 32) {
      throw new Error("Add-on returned no usable source or too many sources");
    }
    const rejected = [];
    for (const source of sources) {
      try {
        validateSource(source, addonManifest.permissions.network);
        await playSource(source);
        return;
      } catch (error) {
        rejected.push(errorText(error));
      }
    }
    throw new Error(`Every source was rejected: ${rejected.join("; ")}`);
  });
}

elements["create-profile"].addEventListener("click", () =>
  run("Creating profile", async () => {
    const profileKeyHex = generate_profile_key();
    const bundle = create_empty_profile(profileKeyHex);
    await persist(bundle, {
      profileKeyHex,
      recordKey: "",
      writerKeyPair: "",
      epoch: 0,
    });
    download("rvah-recovery.profile.key", `${profileKeyHex}\n`, "text/plain");
    message("Profile created and recovery key downloaded. Store that key separately.", "success");
  }),
);

elements["import-profile"].addEventListener("click", () =>
  run("Importing profile", async () => {
    const profileFile = elements["profile-file"].files?.[0];
    const keyFile = elements["key-file"].files?.[0];
    if (!profileFile || !keyFile) throw new Error("Choose both the encrypted profile and recovery key");
    if (profileFile.size > 512 * 1024) throw new Error("Profile exceeds the 512 KiB limit");
    const bundle = new Uint8Array(await profileFile.arrayBuffer());
    const profileKeyHex = (await keyFile.text()).trim().toLowerCase();
    inspect_profile(bundle, profileKeyHex);
    await persist(bundle, {
      profileKeyHex,
      recordKey: "",
      writerKeyPair: "",
      epoch: 0,
    });
  }),
);

elements["download-profile"].addEventListener("click", () => {
  if (current) download("household.rvah-profile", current.bundle);
});

elements["download-key"].addEventListener("click", () => {
  if (!current) return;
  if (!confirm("Export the recovery key? Anyone with this key and profile ciphertext can read linked-account credentials.")) return;
  download("rvah-recovery.profile.key", `${current.enrollment.profileKeyHex}\n`, "text/plain");
});

elements["rotate-key"].addEventListener("click", () =>
  run("Rotating profile key", async () => {
    if (!current) throw new Error("No profile is enrolled");
    if (!confirm("Rotate the profile key? Other devices must be paired again before they can read future snapshots.")) return;
    const newKey = generate_profile_key();
    const bundle = rekey_profile(
      current.bundle,
      current.enrollment.profileKeyHex,
      newKey,
    );
    const enrollment = {
      ...current.enrollment,
      profileKeyHex: newKey,
      epoch: Number(current.enrollment.epoch || 0),
    };
    await persist(bundle, enrollment);
    download("rvah-rotated.profile.key", `${newKey}\n`, "text/plain");
  }),
);

elements["forget-device"].addEventListener("click", () =>
  run("Forgetting device", async () => {
    if (!confirm("Remove the locally wrapped profile key and encrypted bundle from this browser?")) return;
    await forgetDeviceProfile();
    current = null;
    summary = null;
    renderProfile();
  }),
);

elements["service-form"].addEventListener("submit", (event) => {
  event.preventDefault();
  run("Encrypting service configuration", async () => {
    if (!current) throw new Error("Create or import a profile first");
    const data = new FormData(elements["service-form"]);
    const bundle = put_addon_profile(
      current.bundle,
      current.enrollment.profileKeyHex,
      String(data.get("addonId")),
      String(data.get("sourceVersion")),
      String(data.get("settings")),
      String(data.get("secrets")),
    );
    await persist(bundle, current.enrollment);
    elements["service-form"].elements.secrets.value = "{}";
  });
});

elements["addon-inspect"].addEventListener("click", () =>
  run("Verifying signed add-on", async () => {
    const archive = await readPackage();
    const signature = await readHexFile(elements["addon-signature"], 64, "detached signature");
    const publicKey = await readHexFile(elements["addon-pubkey"], 32, "trusted publisher key");
    const client = new AddonClient();
    let manifest;
    try {
      manifest = await client.call({ action: "load", archive, signature, publicKey });
    } catch (error) {
      client.close();
      throw error;
    }
    stopPlayback();
    addonClient?.close();
    addonClient = client;
    addonManifest = manifest;
    addonGrants = [];
    catalogRequest = null;
    catalogCursor = null;
    elements["media-catalog"].replaceChildren();
    elements["media-more"].hidden = true;
    const fingerprint = Array.from(
      new Uint8Array(await crypto.subtle.digest("SHA-256", publicKey)),
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("").slice(0, 16);
    elements["addon-permissions"].textContent = JSON.stringify({
      publisherFingerprint: fingerprint,
      addon: manifest.addon,
      permissions: manifest.permissions,
    }, null, 2);
    setAddonActions(false);
    setAddonState("Verified; approval required", "pending");
  }),
);

elements["addon-approve"].addEventListener("click", () => {
  if (!addonManifest) return;
  addonGrants = [...addonManifest.addon.capabilities];
  setAddonActions(true);
  setAddonState(`${addonManifest.addon.name} approved`, "ready");
  elements["media-status"].textContent = "Add-on ready. Browse or search its catalog.";
});

elements["addon-unload"].addEventListener("click", unloadAddon);

elements["addon-profile-settings"].addEventListener("click", () =>
  run("Loading enrolled add-on settings", async () => {
    if (!current || !addonManifest) throw new Error("Enroll a profile and approve an add-on first");
    elements["addon-settings"].value = addon_profile_settings(
      current.bundle,
      current.enrollment.profileKeyHex,
      addonManifest.addon.id,
    );
  }),
);

elements["media-browse"].addEventListener("click", () => {
  void loadCatalog({ op: "list", path: null, cursor: null }, false);
});

elements["media-search"].addEventListener("click", () => {
  const query = elements["media-query"].value.trim();
  if (!query || query.length > 512) {
    message("Search needs 1–512 characters.", "error");
    return;
  }
  void loadCatalog({ op: "search", query, filters: [], cursor: null }, false);
});

elements["media-more"].addEventListener("click", () => {
  if (catalogRequest && catalogCursor) void loadCatalog(catalogRequest, true);
});

elements["media-import-session"].addEventListener("click", () =>
  run("Importing private playback session", async () => {
    const descriptor = validateDescriptor(await readJsonFile(elements["media-session"], 64 * 1024));
    mediaSessions.set(descriptor.assetId, descriptor);
    elements["media-session"].value = "";
    elements["media-session-state"].textContent = `Session ready for ${descriptor.assetId}; expires ${new Date(descriptor.expiresAt).toLocaleTimeString()}.`;
  }),
);

elements["media-stop"].addEventListener("click", stopPlayback);

elements["pair-request"].addEventListener("click", () =>
  run("Creating pairing request", async () => {
    const request = await createPairingRequest(navigator.userAgentData?.platform || navigator.platform);
    download("rvah-pair-request.json", JSON.stringify(request, null, 2), "application/json");
  }),
);

elements["pair-request-file"].addEventListener("change", renderProfile);

elements["pair-respond"].addEventListener("click", () =>
  run("Creating pairing response", async () => {
    if (!current) throw new Error("This device has no enrolled profile");
    const request = await readJsonFile(elements["pair-request-file"]);
    const response = await createPairingResponse(request, current.enrollment, current.bundle);
    download("rvah-pair-response.json", JSON.stringify(response), "application/json");
  }),
);

elements["pair-accept"].addEventListener("click", () =>
  run("Accepting pairing response", async () => {
    const response = await readJsonFile(elements["pair-response-file"]);
    const paired = await acceptPairingResponse(response);
    inspect_profile(paired.bundle, paired.enrollment.profileKeyHex);
    await persist(paired.bundle, paired.enrollment);
  }),
);

function veilidUpdate(update) {
  if (update.kind === "Attachment") {
    const state = update.publicInternetReady ? "ready" : update.state;
    elements["veilid-state"].textContent = `Veilid ${state}`;
    elements["veilid-state"].className = `pill ${update.publicInternetReady ? "ready" : "pending"}`;
  }
  if (update.kind === "Shutdown") setVeilidButtons(false);
}

function setVeilidButtons(connected) {
  elements["veilid-connect"].disabled = connected;
  elements["veilid-disconnect"].disabled = !connected;
  elements["veilid-publish"].disabled = !connected || !current;
  elements["veilid-fetch"].disabled = !connected;
  if (!connected) {
    elements["veilid-state"].textContent = "Veilid offline";
    elements["veilid-state"].className = "pill neutral";
  }
}

elements["veilid-connect"].addEventListener("click", () =>
  run("Connecting to Veilid", async () => {
    const bootstrap = elements["bootstrap-url"].value.trim();
    if (!bootstrap) throw new Error("Enter a direct Veilid WebSocket bootstrap URL");
    localStorage.setItem("rvah.veilid.bootstrap", bootstrap);
    await connectVeilid(bootstrap, veilidUpdate);
    setVeilidButtons(true);
    elements["veilid-state"].textContent = "Veilid attaching";
    elements["veilid-state"].className = "pill pending";
  }),
);

elements["veilid-disconnect"].addEventListener("click", () =>
  run("Disconnecting Veilid", async () => {
    await disconnectVeilid();
    setVeilidButtons(false);
  }),
);

elements["veilid-publish"].addEventListener("click", () =>
  run("Publishing encrypted profile", async () => {
    if (!current) throw new Error("No profile is enrolled");
    const nextEpoch = Math.max(1, Number(current.enrollment.epoch || 0) + 1);
    const snapshot = new PreparedSnapshot(nextEpoch, current.bundle);
    try {
      const chunks = [];
      for (let index = 0; index < snapshot.chunkCount; index += 1) {
        chunks.push(snapshot.chunk(index));
      }
      const result = await publishSnapshot(
        snapshot.headBytes(),
        chunks,
        current.enrollment,
        snapshot_subkey_count(),
      );
      await persist(current.bundle, {
        ...current.enrollment,
        ...result,
        epoch: nextEpoch,
      });
      elements["record-key"].value = result.recordKey;
    } finally {
      snapshot.free();
    }
  }),
);

elements["veilid-fetch"].addEventListener("click", () =>
  run("Fetching encrypted profile", async () => {
    if (!current) throw new Error("Import a recovery key/profile or accept pairing before fetching");
    const recordKey = elements["record-key"].value.trim();
    if (!recordKey) throw new Error("Enter a Veilid profile record key");
    const snapshot = await fetchSnapshot(recordKey);
    const head = inspect_snapshot_head(snapshot.headBytes);
    const bundle = assemble_snapshot(
      snapshot.headBytes,
      snapshot.chunks,
      Number(current.enrollment.epoch || 0),
    );
    inspect_profile(bundle, current.enrollment.profileKeyHex);
    await persist(bundle, {
      ...current.enrollment,
      recordKey,
      epoch: Number(head.epoch),
    });
  }),
);

window.addEventListener("beforeunload", () => {
  mediaAbort?.abort();
  addonClient?.close();
  if (isVeilidConnected()) void disconnectVeilid();
});

async function boot() {
  await initWasm();
  elements["wasm-state"].textContent = `WASM ${format_version()}`;
  elements["wasm-state"].className = "pill ready";
  elements["bootstrap-url"].value =
    import.meta.env.VITE_VEILID_BOOTSTRAP ||
    localStorage.getItem("rvah.veilid.bootstrap") ||
    "";
  current = await loadDeviceProfile();
  if (current) {
    try {
      await refreshSummary();
      message("Device profile unlocked from browser-protected storage.", "success");
    } catch (error) {
      console.error("Stored enrollment could not be opened", error);
      current = null;
      summary = null;
      renderProfile();
      message("Stored enrollment could not be opened. Re-import the profile and recovery key.", "error");
    }
  } else {
    renderProfile();
    message("WASM ready. Create a profile or import an encrypted native profile.", "success");
  }
  if ("serviceWorker" in navigator && import.meta.env.PROD) {
    navigator.serviceWorker.register("/sw.js").catch((error) => {
      console.warn("Service worker registration failed", error);
    });
  }
}

boot().catch((error) => {
  console.error("RVAH web startup failed", error);
  elements["wasm-state"].textContent = "WASM failed";
  elements["wasm-state"].className = "pill error";
  message(`Startup failed: ${errorText(error)}`, "error");
});
