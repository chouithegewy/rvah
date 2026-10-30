// Host-owned private-route transport. Never pass descriptors/keys to add-ons.
export const MAX_MEDIA_BYTES = 16 * 1024 * 1024;
export const FRAGMENT_BYTES = 16 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
export const toBase64 = (bytes) => btoa(Array.from(bytes, (b) => String.fromCharCode(b)).join(""));
export function fromBase64(text, max = MAX_MEDIA_BYTES) {
  if (typeof text !== "string" || text.length > Math.ceil(max / 3) * 4) throw new Error("Oversized binary field");
  const bytes = Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  if (bytes.length > max) throw new Error("Oversized binary field");
  return bytes;
}

export function validateDescriptor(d) {
  if (!d || d.version !== 1 || !/^[a-zA-Z0-9._-]{1,128}$/.test(d.assetId)
      || !/^[0-9a-f]{32}$/.test(d.sessionId) || !/^[0-9a-f]{64}$/.test(d.sha256)
      || !Number.isSafeInteger(d.byteLength) || d.byteLength < 1 || d.byteLength > MAX_MEDIA_BYTES
      || !Number.isSafeInteger(d.expiresAt) || d.expiresAt <= Date.now()
      || !["video/mp4", "video/webm", "audio/mp4", "audio/webm", "audio/mpeg"].includes(d.mime)
      || fromBase64(d.key, 32).length !== 32 || !fromBase64(d.routeBlob, 8192).length) {
    throw new Error("Invalid or expired relay enrollment");
  }
  return d;
}

export async function requestKey(raw) {
  const prefix = encoder.encode("rvah-stream-request-key-v1\0");
  const material = new Uint8Array(prefix.length + raw.length);
  material.set(prefix); material.set(raw, prefix.length);
  return crypto.subtle.importKey("raw", await crypto.subtle.digest("SHA-256", material), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
}

export async function encodeRequest(body, key) {
  const payload = encoder.encode(JSON.stringify(body));
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, payload));
  const result = new Uint8Array(32 + payload.length);
  result.set(mac); result.set(payload, 32);
  if (result.length > 4096) throw new Error("Relay request too large");
  return result;
}

export async function decodeRequest(bytes, key) {
  if (!(bytes instanceof Uint8Array) || bytes.length < 33 || bytes.length > 4096
      || !await crypto.subtle.verify("HMAC", key, bytes.subarray(0, 32), bytes.subarray(32))) {
    throw new Error("Unauthenticated relay request");
  }
  return JSON.parse(decoder.decode(bytes.subarray(32)));
}

// JSON metadata + raw nonce/ciphertext, never JSON arrays on the network.
export function packFrame(frame) {
  const { nonce, ciphertext, ...metadata } = frame;
  const header = encoder.encode(JSON.stringify(metadata));
  if (header.length > 1024 || nonce.length !== 12 || ciphertext.length > 49168) throw new Error("Invalid relay frame");
  const packed = new Uint8Array(2 + header.length + 12 + ciphertext.length);
  new DataView(packed.buffer).setUint16(0, header.length);
  packed.set(header, 2); packed.set(nonce, 2 + header.length); packed.set(ciphertext, 14 + header.length);
  return packed;
}

export function unpackFrame(packed) {
  if (packed.length < 31 || packed.length > 50206) throw new Error("Invalid packed relay frame");
  const length = new DataView(packed.buffer, packed.byteOffset, packed.byteLength).getUint16(0);
  if (length > 1024 || length + 30 > packed.length) throw new Error("Invalid relay metadata");
  const metadata = JSON.parse(decoder.decode(packed.subarray(2, 2 + length)));
  return { ...metadata, nonce: Array.from(packed.subarray(2 + length, 14 + length)), ciphertext: Array.from(packed.subarray(14 + length)) };
}

function boundedCall(promise, signal, timeoutMs) {
  return new Promise((resolve, reject) => {
    const cancel = () => finish(reject, new DOMException("Playback cancelled", "AbortError"));
    const timer = setTimeout(() => finish(reject, new Error("Veilid relay request timed out")), timeoutMs);
    function finish(fn, value) { clearTimeout(timer); signal?.removeEventListener("abort", cancel); fn(value); }
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    promise.then((value) => finish(resolve, value), (error) => finish(reject, error));
  });
}

/** One in-flight request: pull backpressure, bounded retries, route cleanup.
 * Veilid does not expose per-appCall cancellation; timeout releases the route
 * and never starts overlapping retries. Core's own timeout permits a retry.
 */
export async function receiveMedia(descriptor, { client, routing, codec, signal, onProgress, timeoutMs = 35000 }) {
  const d = validateDescriptor(descriptor);
  const rawKey = fromBase64(d.key, 32);
  const key = await requestKey(rawKey);
  const routeId = client.importRemotePrivateRoute(fromBase64(d.routeBlob, 8192));
  const call = async (body) => {
    const request = await encodeRequest({ ...body, sessionId: d.sessionId }, key);
    for (let attempt = 0; ; attempt++) {
      signal?.throwIfAborted();
      if (Date.now() >= d.expiresAt) throw new Error("Relay enrollment expired");
      try {
        const response = await boundedCall(routing.appCall({ RouteId: routeId }, request), signal, timeoutMs);
        if (!(response instanceof Uint8Array) || response.length < 1 || response.length > FRAGMENT_BYTES + 1 || response[0] !== 1) throw new Error("Relay rejected request or segment expired");
        return response.subarray(1);
      } catch (error) {
        if (signal?.aborted || error.message?.includes("timed out") || attempt >= 2) throw error;
      }
    }
  };
  try {
    const count = Math.ceil(d.byteLength / 49152);
    const frames = [];
    for (let index = 0; index < count; index++) {
      const fragments = [];
      let total = 0;
      // A 48 KiB encrypted frame plus metadata occupies at most four fragments.
      for (let fragment = 0; fragment < 4; fragment++) {
        const bytes = await call({ op: "frame", index, fragment });
        fragments.push(bytes); total += bytes.length;
        if (bytes.length < FRAGMENT_BYTES) break;
      }
      const packed = new Uint8Array(total);
      let offset = 0;
      for (const part of fragments) { packed.set(part, offset); offset += part.length; }
      const frame = unpackFrame(packed);
      if (frame.session_id !== d.sessionId || frame.segment !== 0 || frame.chunk_index !== index
          || frame.chunk_count !== count || frame.segment_byte_length !== d.byteLength || frame.segment_sha256 !== d.sha256) throw new Error("Relay frame does not match enrollment");
      frames.push(frame);
      onProgress?.(Math.min(d.byteLength, (index + 1) * 49152), d.byteLength);
    }
    const received = codec.receive_relay_segment(JSON.stringify(frames), rawKey);
    // Rust authenticates every frame and complete digest before this receipt.
    await call({ op: "receipt", receipt: received.receipt });
    return { bytes: received.bytes, mime: d.mime };
  } finally {
    rawKey.fill(0);
    try {
      client.releasePrivateRoute(routeId);
    } catch (error) {
      console.warn("Could not release imported private route", error);
    }
  }
}
