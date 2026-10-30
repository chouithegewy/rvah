// Deterministic, same-process Veilid test double. It never creates a socket,
// calls fetch, or imports the real Veilid WASM binding. Use only with synthetic
// development data.
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

function cloneBytes(value, label, maximum = 1024 * 1024) {
  if (!(value instanceof Uint8Array) || value.length > maximum) {
    throw new Error(`${label} must be a bounded Uint8Array`);
  }
  return value.slice();
}

function token(bytes = 16) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) =>
    byte.toString(16).padStart(2, "0")).join("");
}

class MockStringId {
  constructor(kind, value) {
    if (typeof value !== "string" || !value.startsWith(`mock-${kind}-`)) {
      throw new Error(`Invalid mock ${kind}`);
    }
    this.kind = kind;
    this.value = value;
  }

  toString() {
    return this.value;
  }
}

class MockRouteId extends MockStringId {
  constructor(value, handleId) {
    super("route", value);
    this.handleId = handleId;
  }
}

class MockRoutingContext {
  constructor(network, node) {
    this.network = network;
    this.node = node;
    this.openRecords = new Map();
    this.freed = false;
  }

  assertOpen() {
    if (this.freed || this.node.closed) throw new Error("Mock Veilid node is closed");
  }

  async createDHTRecord(_kind, schema) {
    this.assertOpen();
    const count = schema?.DFLT?.oCnt;
    if (!Number.isInteger(count) || count < 1 || count > 256) {
      throw new Error("Invalid mock DHT schema");
    }
    const key = new MockStringId("record", `mock-record-${token()}`);
    const owner = new MockStringId("owner", `mock-owner-${token(32)}`);
    this.network.records.set(key.toString(), { owner: owner.toString(), count, values: new Map() });
    this.openRecords.set(key.toString(), true);
    return { key, owner };
  }

  async openDHTRecord(key, writer) {
    this.assertOpen();
    const record = this.network.records.get(key.toString());
    if (!record) throw new Error("Mock DHT record does not exist");
    if (writer && writer.toString() !== record.owner) throw new Error("Mock DHT writer rejected");
    this.openRecords.set(key.toString(), Boolean(writer));
  }

  async setDHTValue(key, subkey, bytes) {
    this.assertOpen();
    const record = this.network.records.get(key.toString());
    if (!record || this.openRecords.get(key.toString()) !== true) {
      throw new Error("Mock DHT record is not open for writing");
    }
    if (!Number.isInteger(subkey) || subkey < 0 || subkey >= record.count) {
      throw new Error("Mock DHT subkey is outside its schema");
    }
    record.values.set(subkey, cloneBytes(bytes, "Mock DHT value"));
  }

  async getDHTValue(key, subkey) {
    this.assertOpen();
    const record = this.network.records.get(key.toString());
    if (!record || !this.openRecords.has(key.toString())) {
      throw new Error("Mock DHT record is not open");
    }
    const value = record.values.get(subkey);
    return value ? { data: value.slice() } : undefined;
  }

  async flushDHTRecord(key) {
    this.assertOpen();
    if (!this.openRecords.has(key.toString())) throw new Error("Mock DHT record is not open");
    return true;
  }

  async closeDHTRecord(key) {
    this.openRecords.delete(key.toString());
  }

  async appCall(target, request) {
    this.assertOpen();
    const routeId = target?.RouteId;
    if (!(routeId instanceof MockRouteId)) throw new Error("Mock AppCall needs an imported route");
    return this.network.appCall(this.node, routeId, cloneBytes(request, "Mock AppCall", 32 * 1024));
  }

  free() {
    this.openRecords.clear();
    this.freed = true;
  }
}

class MockNode {
  constructor(network, onUpdate) {
    this.network = network;
    this.id = `mock-node-${token()}`;
    this.onUpdate = onUpdate;
    this.handles = new Map();
    this.closed = false;
    this.routing = new MockRoutingContext(network, this);
    this.client = {
      newPrivateRoute: () => this.newPrivateRoute(),
      importRemotePrivateRoute: (blob) => this.importRemotePrivateRoute(blob),
      releasePrivateRoute: (routeId) => this.releasePrivateRoute(routeId),
      appCallReply: (callId, message) => this.appCallReply(callId, message),
      detach: async () => {},
      shutdownCore: async () => this.close(),
    };
    this.veilid = {
      veilidClient: this.client,
      RecordKey: { parse: (value) => new MockStringId("record", String(value)) },
      KeyPair: { parse: (value) => new MockStringId("owner", String(value)) },
    };
  }

  assertOpen() {
    if (this.closed) throw new Error("Mock Veilid node is closed");
  }

  newHandle(route, owner) {
    const handleId = `mock-handle-${token()}`;
    const routeId = new MockRouteId(route.id, handleId);
    this.handles.set(handleId, { route, owner });
    return routeId;
  }

  async newPrivateRoute() {
    this.assertOpen();
    const route = { id: `mock-route-${token()}`, owner: this, active: true };
    this.network.routes.set(route.id, route);
    const routeId = this.newHandle(route, true);
    const blob = encoder.encode(JSON.stringify({ format: "rvah-mock-route-v1", route: route.id }));
    return { routeId, blob };
  }

  importRemotePrivateRoute(blob) {
    this.assertOpen();
    const bytes = cloneBytes(blob, "Mock route blob", 8192);
    const descriptor = JSON.parse(decoder.decode(bytes));
    const route = descriptor?.format === "rvah-mock-route-v1"
      ? this.network.routes.get(descriptor.route)
      : undefined;
    if (!route?.active) throw new Error("Mock private route is unavailable");
    return this.newHandle(route, false);
  }

  releasePrivateRoute(routeId) {
    this.assertOpen();
    const handle = routeId instanceof MockRouteId ? this.handles.get(routeId.handleId) : undefined;
    if (!handle) throw new Error("Unknown mock private-route handle");
    this.handles.delete(routeId.handleId);
    if (handle.owner) {
      handle.route.active = false;
      this.network.routes.delete(handle.route.id);
    }
  }

  async appCallReply(callId, message) {
    this.assertOpen();
    this.network.appCallReply(this, callId, cloneBytes(message, "Mock AppCall reply", 32 * 1024));
  }

  close() {
    if (this.closed) return;
    for (const [handleId, handle] of this.handles) {
      if (handle.owner) {
        handle.route.active = false;
        this.network.routes.delete(handle.route.id);
      }
      this.handles.delete(handleId);
    }
    this.closed = true;
    this.network.nodes.delete(this.id);
  }
}

export class MockVeilidNetwork {
  constructor(name = "local") {
    this.name = String(name).slice(0, 80);
    this.nodes = new Map();
    this.records = new Map();
    this.routes = new Map();
    this.calls = new Map();
    this.externalIoCount = 0;
  }

  createNode(onUpdate = () => {}) {
    const node = new MockNode(this, onUpdate);
    this.nodes.set(node.id, node);
    return node;
  }

  appCall(caller, routeId, request) {
    const handle = caller.handles.get(routeId.handleId);
    if (!handle || !handle.route.active) return Promise.reject(new Error("Mock private route is unavailable"));
    const callId = `mock-call-${token()}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.calls.delete(callId);
        reject(new Error("Mock AppCall timed out"));
      }, 5_000);
      this.calls.set(callId, { receiver: handle.route.owner, resolve, reject, timer });
      queueMicrotask(() => handle.route.owner.onUpdate({
        kind: "AppCall",
        callId,
        routeId: handle.route.id,
        message: request.slice(),
      }));
    });
  }

  appCallReply(node, callId, message) {
    const call = this.calls.get(callId);
    if (!call || call.receiver !== node) throw new Error("Unknown mock AppCall reply");
    clearTimeout(call.timer);
    this.calls.delete(callId);
    call.resolve(message.slice());
  }
}

const sharedNetworks = new Map();

export function sharedMockVeilidNetwork(name = "local") {
  if (!sharedNetworks.has(name)) sharedNetworks.set(name, new MockVeilidNetwork(name));
  return sharedNetworks.get(name);
}
