let veilid = null;
let routing = null;
let started = false;

function validateBootstrap(url) {
  const parsed = new URL(url);
  if (parsed.protocol === "mock:") {
    const localHost = ["localhost", "127.0.0.1", "::1"].includes(location.hostname);
    if (!import.meta.env.DEV && import.meta.env.VITE_ENABLE_MOCK_VEILID !== "1" && !localHost) {
      throw new Error("Mock Veilid is limited to development or loopback origins");
    }
    return parsed;
  }
  if (!['ws:', 'wss:'].includes(parsed.protocol)) {
    throw new Error("Veilid bootstrap must use ws:// or wss://");
  }
  if (location.protocol === "https:" && parsed.protocol !== "wss:") {
    throw new Error("HTTPS deployments require a wss:// Veilid bootstrap");
  }
  return parsed;
}

export async function connectVeilid(bootstrapUrl, onUpdate) {
  if (started) return;
  const bootstrap = validateBootstrap(bootstrapUrl);
  if (bootstrap.protocol === "mock:") {
    const { sharedMockVeilidNetwork } = await import("./mock-veilid.js");
    const node = sharedMockVeilidNetwork(bootstrap.host || "local").createNode(onUpdate);
    veilid = node.veilid;
    routing = node.routing;
    started = true;
    // Match the real core's asynchronous attachment update so callers finish
    // their immediate "attaching" UI transition first.
    setTimeout(() => onUpdate?.({ kind: "Attachment", state: "Mock", publicInternetReady: true }), 0);
    return;
  }
  veilid = await import("@bmv437/veilid-wasm");
  try {
    await veilid.default();
    await veilid.veilidClient.initializeCore({
      logging: {
        performance: {
          enabled: true,
          level: "Warn",
          console: { enabled: true, color: false, timestamp: true },
        },
      },
    });
    const config = veilid.veilidClient.defaultConfig();
    config.programName = "rvah_web";
    config.namespace = location.hostname.replace(/[^a-zA-Z0-9_. -]/g, "_") || "local";
    config.network.routingTable.bootstrap = [bootstrap.toString()];
    config.network.udp.enabled = false;
    config.network.tcp.connect = false;
    config.network.tcp.listen = false;
    config.network.ws.connect = true;
    config.network.ws.listen = false;
    await veilid.veilidClient.startupCore((update) => onUpdate?.(update), config);
    await veilid.veilidClient.attach();
    routing = new veilid.VeilidRoutingContext();
    started = true;
  } catch (error) {
    routing?.free();
    routing = null;
    await veilid.veilidClient.detach().catch(() => {});
    await veilid.veilidClient.shutdownCore().catch(() => {});
    veilid = null;
    throw error;
  }
}

export async function disconnectVeilid() {
  if (!started) return;
  routing?.free();
  routing = null;
  try {
    await veilid.veilidClient.detach();
  } finally {
    try {
      await veilid.veilidClient.shutdownCore();
    } finally {
      started = false;
      veilid = null;
    }
  }
}

export function isVeilidConnected() {
  return started;
}

// Host-only access. These handles are never sent to the add-on worker.
export function veilidTransport() {
  if (!started || !routing || !veilid) throw new Error("Connect to Veilid before playing");
  return { client: veilid.veilidClient, routing };
}

export async function publishSnapshot(headBytes, chunks, enrollment, subkeyCount) {
  if (!routing || !veilid) throw new Error("Connect to Veilid first");
  if (!Number.isInteger(subkeyCount) || subkeyCount < chunks.length + 1) {
    throw new Error("The snapshot schema cannot hold every encrypted chunk");
  }
  let recordKey;
  let writerKeyPair = enrollment.writerKeyPair || "";
  if (enrollment.recordKey && writerKeyPair) {
    recordKey = veilid.RecordKey.parse(enrollment.recordKey);
    const writer = veilid.KeyPair.parse(writerKeyPair);
    await routing.openDHTRecord(recordKey, writer);
  } else {
    const descriptor = await routing.createDHTRecord(
      "VLD0",
      { DFLT: { oCnt: subkeyCount } },
      undefined,
    );
    recordKey = descriptor.key;
    writerKeyPair = descriptor.owner.toString();
  }

  try {
    for (let index = 0; index < chunks.length; index += 1) {
      await routing.setDHTValue(recordKey, index + 1, chunks[index], {
        allowOffline: false,
      });
    }
    // Advance the head only after every ciphertext chunk has been accepted.
    await routing.setDHTValue(recordKey, 0, headBytes, { allowOffline: false });
    const flushed = await routing.flushDHTRecord(recordKey, 30_000);
    if (!flushed) throw new Error("Timed out while flushing the Veilid profile record");
    return { recordKey: recordKey.toString(), writerKeyPair };
  } finally {
    await routing.closeDHTRecord(recordKey);
  }
}

export async function fetchSnapshot(recordKeyText) {
  if (!routing || !veilid) throw new Error("Connect to Veilid first");
  const recordKey = veilid.RecordKey.parse(recordKeyText.trim());
  await routing.openDHTRecord(recordKey);
  try {
    const headValue = await routing.getDHTValue(recordKey, 0, true);
    if (!headValue) throw new Error("Veilid record has no profile head");
    const headBytes = new Uint8Array(headValue.data);
    const head = JSON.parse(new TextDecoder().decode(headBytes));
    if (!Number.isInteger(head.chunk_count) || head.chunk_count < 1 || head.chunk_count > 22) {
      throw new Error("Veilid profile head declares an invalid chunk count");
    }
    const chunks = [];
    for (let subkey = 1; subkey <= head.chunk_count; subkey += 1) {
      const value = await routing.getDHTValue(recordKey, subkey, true);
      if (!value) throw new Error(`Veilid profile is missing ciphertext chunk ${subkey}`);
      chunks.push(new Uint8Array(value.data));
    }
    return { headBytes, chunks };
  } finally {
    await routing.closeDHTRecord(recordKey);
  }
}
