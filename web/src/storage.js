const DATABASE = "rvah-web";
const STORE = "state";
const DEVICE_KEY = "device-wrapping-key";
const ENROLLMENT = "enrollment";
const PROFILE = "profile-ciphertext";
const PAIRING = "pending-pairing";
const AAD = new TextEncoder().encode("rvah-browser-enrollment-v1");

function requestResult(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("IndexedDB request failed"));
  });
}

async function database() {
  const request = indexedDB.open(DATABASE, 1);
  request.onupgradeneeded = () => {
    if (!request.result.objectStoreNames.contains(STORE)) {
      request.result.createObjectStore(STORE);
    }
  };
  return requestResult(request);
}

async function transact(mode, action) {
  const db = await database();
  try {
    const transaction = db.transaction(STORE, mode);
    const store = transaction.objectStore(STORE);
    const result = await action(store);
    await new Promise((resolve, reject) => {
      transaction.oncomplete = resolve;
      transaction.onerror = () => reject(transaction.error || new Error("IndexedDB transaction failed"));
      transaction.onabort = () => reject(transaction.error || new Error("IndexedDB transaction aborted"));
    });
    return result;
  } finally {
    db.close();
  }
}

async function get(name) {
  return transact("readonly", (store) => requestResult(store.get(name)));
}

async function put(name, value) {
  return transact("readwrite", (store) => requestResult(store.put(value, name)));
}

async function remove(name) {
  return transact("readwrite", (store) => requestResult(store.delete(name)));
}

async function wrappingKey() {
  let key = await get(DEVICE_KEY);
  if (!key) {
    key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, [
      "encrypt",
      "decrypt",
    ]);
    await put(DEVICE_KEY, key);
  }
  return key;
}

async function wrap(value) {
  const key = await wrappingKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(value));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: AAD },
    key,
    plaintext,
  );
  return { iv, ciphertext: new Uint8Array(ciphertext) };
}

async function unwrap(value) {
  if (!value?.iv || !value?.ciphertext) return null;
  const key = await wrappingKey();
  const plaintext = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: new Uint8Array(value.iv),
      additionalData: AAD,
    },
    key,
    new Uint8Array(value.ciphertext),
  );
  return JSON.parse(new TextDecoder().decode(plaintext));
}

export async function saveDeviceProfile(bundle, enrollment) {
  await put(PROFILE, new Uint8Array(bundle));
  await put(ENROLLMENT, await wrap(enrollment));
}

export async function loadDeviceProfile() {
  const [bundle, wrappedEnrollment] = await Promise.all([get(PROFILE), get(ENROLLMENT)]);
  if (!bundle || !wrappedEnrollment) return null;
  return {
    bundle: new Uint8Array(bundle),
    enrollment: await unwrap(wrappedEnrollment),
  };
}

export async function forgetDeviceProfile() {
  await Promise.all([remove(PROFILE), remove(ENROLLMENT), remove(PAIRING)]);
}

export async function savePendingPairing(value) {
  await put(PAIRING, value);
}

export async function loadPendingPairing() {
  return get(PAIRING);
}

export async function clearPendingPairing() {
  await remove(PAIRING);
}
