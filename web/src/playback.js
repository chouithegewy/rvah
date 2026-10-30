export const MAX_MEDIA_BYTES = 16 * 1024 * 1024;

export function validateSource(source, network = [], now = Date.now() / 1000) {
  if (!source || typeof source.url !== "string") throw new Error("Invalid playback source");
  if (source.drm) throw new Error("DRM playback is unsupported");
  if (source.expires_at != null && (!Number.isFinite(source.expires_at) || source.expires_at <= now)) {
    throw new Error("Playback source has expired");
  }
  if (Object.keys(source.headers || {}).length) throw new Error("Custom playback headers are unsupported in this browser host");
  if (source.subtitles?.length) throw new Error("External subtitle tracks are unsupported in this browser slice");
  if (source.protocol === "veilid") {
    const match = /^rvah\+veilid:\/\/media\/([A-Za-z0-9_-]{1,128})$/.exec(source.url);
    if (!match) throw new Error("Invalid Veilid media identifier");
    return { kind: "veilid", mediaId: match[1] };
  }
  if (!["http", "https"].includes(source.protocol)) {
    throw new Error("This browser host supports finite HTTP(S) and Veilid media only; HLS, DASH and local paths are unsupported");
  }
  const url = new URL(source.url);
  if (url.protocol !== `${source.protocol}:` || url.username || url.password || url.hash) {
    throw new Error("Invalid HTTP(S) playback URL");
  }
  const host = url.hostname.toLowerCase();
  const allowed = network.some((rule) => rule.startsWith("*.")
    ? host.endsWith(`.${rule.slice(2)}`) && host !== rule.slice(2) && !/^[\d.]+$/.test(host)
    : host === rule);
  if (!allowed) throw new Error("Playback domain is not declared by the signed add-on");
  return { kind: "http", url: url.href };
}

export async function readBoundedMedia(response, signal) {
  if (!response.ok || !response.body) throw new Error("Media upstream refused the request");
  const length = response.headers.get("content-length");
  if (length && (!/^\d+$/.test(length) || Number(length) > MAX_MEDIA_BYTES)) {
    await response.body.cancel();
    throw new Error("Media exceeds the 16 MiB browser limit");
  }
  const mime = (response.headers.get("content-type") || "").split(";")[0].trim();
  if (!["video/mp4", "video/webm", "audio/mp4", "audio/webm", "audio/mpeg", "audio/ogg", "video/ogg"].includes(mime)) {
    await response.body.cancel();
    throw new Error("Unsupported media Content-Type; provide a finite MP4, WebM, MP3 or Ogg file");
  }
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      signal?.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > MAX_MEDIA_BYTES) throw new Error("Media exceeds the 16 MiB browser limit");
      chunks.push(value);
    }
    if (!size) throw new Error("Media is empty");
    return new Blob(chunks, { type: mime });
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function fetchMedia(url, signal) {
  const timeout = AbortSignal.timeout(60_000);
  const combined = signal ? AbortSignal.any([signal, timeout]) : timeout;
  // Browser CORS is mandatory. Redirects are rejected so a signed domain grant
  // cannot authorize a hidden request to an undeclared destination.
  let response;
  try {
    response = await fetch(url, { signal: combined, credentials: "omit", redirect: "error", cache: "no-store", referrerPolicy: "no-referrer" });
  } catch (error) {
    if (combined.aborted) throw new Error("Media download cancelled or timed out");
    throw new Error("Media request failed: check browser CORS, HTTPS and redirect policy", { cause: error });
  }
  return readBoundedMedia(response, combined);
}
