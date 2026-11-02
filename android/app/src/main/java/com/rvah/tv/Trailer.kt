package com.rvah.tv

import android.annotation.SuppressLint
import android.view.ViewGroup
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.viewinterop.AndroidView
import java.net.URI

private val YouTubeId = Regex("^[A-Za-z0-9_-]{6,20}$")

/** Accept ordinary, short, Shorts and embed YouTube links without trusting arbitrary HTML. */
fun youtubeVideoId(url: String): String? {
    val uri = runCatching { URI(url) }.getOrNull() ?: return null
    val host = uri.host?.lowercase()?.removePrefix("www.")?.removePrefix("m.") ?: return null
    val path = uri.path.orEmpty()
    val candidate = when (host) {
        "youtu.be" -> path.trimStart('/').substringBefore('/')
        "youtube.com" -> when {
            path == "/watch" -> uri.rawQuery.orEmpty().split('&')
                .firstOrNull { it.substringBefore('=') == "v" }
                ?.substringAfter('=', "")
            path.startsWith("/embed/") -> path.removePrefix("/embed/").substringBefore('/')
            path.startsWith("/shorts/") -> path.removePrefix("/shorts/").substringBefore('/')
            else -> null
        }
        else -> null
    }
    return candidate?.takeIf { YouTubeId.matches(it) }
}

/**
 * YouTube refuses embeds that don't identify the app embedding them (player
 * error 153), and a WebView's own navigation sends no Referer.
 */
private fun identity(packageName: String) = mapOf("Referer" to "https://$packageName/")

/** YouTube's supported embed player, kept inside the TV app and streamed by the device. */
@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun TrailerScreen(url: String, onExit: () -> Unit) {
    BackHandler(onBack = onExit)
    val videoId = remember(url) { youtubeVideoId(url) }
    if (videoId == null) {
        Page("Trailer unavailable") {
            Body("This trailer is not a valid YouTube link.", ErrorText)
        }
        return
    }
    val embed = remember(videoId) {
        "https://www.youtube-nocookie.com/embed/$videoId?autoplay=1&rel=0&playsinline=1"
    }
    val webView = remember { arrayOfNulls<WebView>(1) }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        AndroidView(
            factory = { context ->
                WebView(context).also { view ->
                    webView[0] = view
                    // Without explicit params the view is WRAP_CONTENT, and WebView
                    // then lays pages out at zero height (100vh = 0): YouTube's
                    // player collapses and only the sound plays.
                    view.layoutParams = ViewGroup.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT,
                    )
                    view.setBackgroundColor(android.graphics.Color.BLACK)
                    view.settings.javaScriptEnabled = true
                    view.settings.mediaPlaybackRequiresUserGesture = false
                    view.webViewClient = WebViewClient()
                    view.loadUrl(embed, identity(context.packageName))
                }
            },
            update = { view -> if (view.url != embed) view.loadUrl(embed, identity(view.context.packageName)) },
            modifier = Modifier.fillMaxSize(),
        )
    }
    DisposableEffect(Unit) {
        onDispose {
            webView[0]?.stopLoading()
            webView[0]?.destroy()
            webView[0] = null
        }
    }
}
