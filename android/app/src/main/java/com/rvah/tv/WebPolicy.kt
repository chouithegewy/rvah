package com.rvah.tv

import java.net.URI
import java.util.Locale

/** Days a downloaded filter list is used before it is fetched again. */
private const val LIST_MAX_AGE_MS = 7 * 24 * 3600 * 1000L

/**
 * The request type the ad blocker matches rules against, which WebView does
 * not report: the main frame is the document; otherwise the file extension
 * decides, then the `Accept` header (an html request below the main frame is
 * an iframe's page, `sub_frame`, which `$subdocument` rules match).
 */
fun requestType(url: String, isMainFrame: Boolean, accept: String?): String {
    if (isMainFrame) return "document"
    val name = url.substringBefore('#').substringBefore('?').substringAfterLast('/')
    when (name.substringAfterLast('.', "").lowercase(Locale.ROOT)) {
        "js", "mjs" -> return "script"
        "css" -> return "stylesheet"
        "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico" -> return "image"
        "m3u8", "ts", "mp4", "m4s", "aac", "mp3", "webm" -> return "media"
        "woff", "woff2", "ttf", "otf" -> return "font"
    }
    val wants = accept.orEmpty().lowercase(Locale.ROOT)
    return when {
        wants.startsWith("text/html") -> "sub_frame"
        wants.startsWith("image/") -> "image"
        wants.startsWith("text/css") -> "stylesheet"
        "json" in wants -> "xmlhttprequest"
        else -> "other"
    }
}

/** Whether [host] is one the source asked never to block, or under one. */
fun neverBlocked(host: String, neverBlock: List<String>): Boolean {
    val name = host.lowercase(Locale.ROOT)
    return neverBlock.any {
        val entry = it.lowercase(Locale.ROOT)
        name == entry || name.endsWith(".$entry")
    }
}

/**
 * Whether the web player may follow a navigation: iframes load freely (they
 * are the video's player), but the page itself stays on its own host.
 */
fun navigationAllowed(sourceUrl: String, targetUrl: String, isMainFrame: Boolean): Boolean {
    if (!isMainFrame) return true
    val source = webHost(sourceUrl) ?: return false
    return webHost(targetUrl) == source
}

/** Whether a filter list last written at [lastModifiedMs] (0: never) is due again. */
fun listsStale(lastModifiedMs: Long, nowMs: Long): Boolean =
    lastModifiedMs == 0L || nowMs - lastModifiedMs >= LIST_MAX_AGE_MS

private fun webHost(url: String): String? {
    val uri = runCatching { URI(url) }.getOrNull() ?: return null
    if (uri.scheme?.lowercase(Locale.ROOT) !in setOf("http", "https")) return null
    return uri.host?.lowercase(Locale.ROOT)
}
