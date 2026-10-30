import {
  clearPendingPairing,
  loadPendingPairing,
  savePendingPairing,
} from "./storage.js";

const REQUEST_FORMAT = "rvah-pair-request-v1";
const RESPONSE_FORMAT = "rvah-pair-response-v1";
const PAIR_LIFETIME_MS = 10 * 60 * 1000;
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function bytesToBase64(bytes) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

function base64ToBytes(value) {
  const normalized = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  const binary = atob(padded);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

function randomToken(bytes = 16) {
  return bytesToBase64(crypto.getRandomValues(new Uint8Array(bytes)))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}

function assertCurrent(value, format) {
  if (!value || value.format !== format || typeof value.id !== "string") {
    throw new Error(`Not a ${format} document`);
  }
  if (!Number.isSafeInteger(value.expiresAt) || Date.now() > value.expiresAt) {
    throw new Error("Pairing document expired; create a new request");
  }
}

async function deriveKey(privateKey, publicJwk, challenge) {
  const publicKey = await crypto.subtle.importKey(
    "jwk",
    publicJwk,
    { name: "ECDH", namedCurve: "P-256" },
    false,
    [],
  );
  const sharedBits = await crypto.subtle.deriveBits(
    { name: "ECDH", public: publicKey },
    privateKey,
    256,
  );
  const material = await crypto.subtle.importKey("raw", sharedBits, "HKDF", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: base64ToBytes(challenge),
      info: textEncoder.encode("rvah-device-pairing-v1"),
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

function additionalData(id, challenge) {
  return textEncoder.encode(`${REQUEST_FORMAT}\n${id}\n${challenge}`);
}

export async function createPairingRequest(deviceName) {
  const keyPair = await crypto.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    false,
    ["deriveBits"],
  );
  const request = {
    format: REQUEST_FORMAT,
    id: randomToken(),
    challenge: randomToken(32),
    expiresAt: Date.now() + PAIR_LIFETIME_MS,
    deviceName: String(deviceName || "Browser device").slice(0, 80),
    publicKey: await crypto.subtle.exportKey("jwk", keyPair.publicKey),
  };
  await savePendingPairing({ request, privateKey: keyPair.privateKey });
  return request;
}

export async function createPairingResponse(request, enrollment, bundle) {
  assertCurrent(request, REQUEST_FORMAT);
  const keyPair = await crypto.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    false,
    ["deriveBits"],
  );
  const key = await deriveKey(keyPair.privateKey, request.publicKey, request.challenge);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const payload = textEncoder.encode(
    JSON.stringify({
      profileKeyHex: enrollment.profileKeyHex,
      recordKey: enrollment.recordKey || "",
      epoch: enrollment.epoch || 0,
      bundle: bytesToBase64(new Uint8Array(bundle)),
    }),
  );
  const ciphertext = await crypto.subtle.encrypt(
    {
      name: "AES-GCM",
      iv,
      additionalData: additionalData(request.id, request.challenge),
    },
    key,
    payload,
  );
  return {
    format: RESPONSE_FORMAT,
    id: request.id,
    challenge: request.challenge,
    expiresAt: request.expiresAt,
    publicKey: await crypto.subtle.exportKey("jwk", keyPair.publicKey),
    iv: bytesToBase64(iv),
    ciphertext: bytesToBase64(new Uint8Array(ciphertext)),
  };
}

export async function acceptPairingResponse(response) {
  assertCurrent(response, RESPONSE_FORMAT);
  const pending = await loadPendingPairing();
  if (!pending || pending.request.id !== response.id) {
    throw new Error("Pairing response does not match this device's pending request");
  }
  if (pending.request.challenge !== response.challenge) {
    throw new Error("Pairing challenge mismatch");
  }
  const key = await deriveKey(
    pending.privateKey,
    response.publicKey,
    response.challenge,
  );
  const plaintext = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: base64ToBytes(response.iv),
      additionalData: additionalData(response.id, response.challenge),
    },
    key,
    base64ToBytes(response.ciphertext),
  );
  const payload = JSON.parse(textDecoder.decode(plaintext));
  if (!/^[0-9a-f]{64}$/.test(payload.profileKeyHex || "")) {
    throw new Error("Pairing response contains an invalid profile key");
  }
  const bundle = base64ToBytes(payload.bundle || "");
  if (!bundle.length || bundle.length > 512 * 1024) {
    throw new Error("Pairing response contains an invalid profile bundle");
  }
  await clearPendingPairing();
  return {
    bundle,
    enrollment: {
      profileKeyHex: payload.profileKeyHex,
      recordKey: String(payload.recordKey || ""),
      writerKeyPair: "",
      epoch: Number(payload.epoch || 0),
    },
  };
}
