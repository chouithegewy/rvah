package com.rvah.tv

import android.annotation.SuppressLint
import android.os.SystemClock
import android.view.ViewGroup
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.tv.material3.Text
import java.net.URI
import kotlin.coroutines.resume
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject

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
 * The embed for [videoId] with YouTube's own player options for a bare
 * video: no controls, keys, fullscreen button, annotations or related
 * videos from other channels. It starts muted; the sound comes up with the
 * picture.
 */
internal fun trailerEmbed(videoId: String) =
    "https://www.youtube-nocookie.com/embed/$videoId" +
        "?autoplay=1&mute=1&controls=0&disablekb=1&fs=0&iv_load_policy=3&rel=0&playsinline=1"

/**
 * YouTube refuses embeds that don't identify the app embedding them (player
 * error 153), and a WebView's own navigation sends no Referer.
 */
private fun identity(packageName: String) = mapOf("Referer" to "https://$packageName/")

/** How long the picture and sound take to come up, and to go at the end. */
private const val FADE_IN_MS = 2500
private const val FADE_OUT_MS = 2000

/** A trailer that has not started by now is not going to. */
private const val GIVE_UP_MS = 15_000L

/** Where the embedded player stands; the embed page is the WebView's own page. */
private const val PLAYER_STATE = """(function () {
  var p = document.getElementById('movie_player');
  return {
    state: p && p.getPlayerState ? p.getPlayerState() : -2,
    time: p && p.getCurrentTime ? p.getCurrentTime() : 0,
    duration: p && p.getDuration ? p.getDuration() : 0,
    error: !!document.querySelector('.ytp-error')
  };
})()"""

private data class PlayerState(
    val state: Int,
    val time: Double,
    val duration: Double,
    val error: Boolean,
) {
    val playing get() = state == 1

    /**
     * Over, or close enough that fading out now ends the fade with the
     * video, before YouTube puts up its end screen.
     */
    val ending get() = state == 0 || (duration > 0 && time >= duration - FADE_OUT_MS / 1000.0)
}

private suspend fun WebView.playerState(): PlayerState? = suspendCancellableCoroutine { done ->
    evaluateJavascript(PLAYER_STATE) { raw ->
        val json = runCatching { JSONObject(raw) }.getOrNull()
        done.resume(
            json?.let {
                PlayerState(
                    state = it.optInt("state", -2),
                    time = it.optDouble("time", 0.0),
                    duration = it.optDouble("duration", 0.0),
                    error = it.optBoolean("error"),
                )
            },
        )
    }
}

/**
 * A YouTube trailer: black while it loads, then, as soon as it plays,
 * picture and sound fade in together over [FADE_IN_MS] while it plays on.
 * YouTube shows its own title and logo over the first seconds of playback
 * whatever the embed options say, so they show through the fade. At the end
 * both fade out and the trailer closes itself, before YouTube's end screen.
 */
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
    val embed = remember(videoId) { trailerEmbed(videoId) }
    val webView = remember { arrayOfNulls<WebView>(1) }
    var shown by remember(embed) { mutableStateOf(false) }
    var failed by remember(embed) { mutableStateOf(false) }
    val alpha by animateFloatAsState(
        targetValue = if (shown) 1f else 0f,
        // Even steps of opacity look like an even fade; an eased curve
        // that front-loads the change just looks like the picture appearing.
        animationSpec = tween(if (shown) FADE_IN_MS else FADE_OUT_MS, easing = LinearEasing),
        label = "trailer fade",
    )

    LaunchedEffect(embed) {
        val opened = SystemClock.uptimeMillis()
        while (true) {
            // Quick to notice the start, so the fade begins with the video.
            delay(if (shown) 250 else 100)
            val player = webView[0]?.playerState() ?: continue
            when {
                player.error -> {
                    failed = true
                    return@LaunchedEffect
                }
                shown && player.ending -> {
                    shown = false
                    delay(FADE_OUT_MS.toLong())
                    onExit()
                    return@LaunchedEffect
                }
                // Over before it ever showed (a very short video): just close.
                !shown && player.state == 0 -> {
                    onExit()
                    return@LaunchedEffect
                }
                // Playing: picture and sound fade in while it plays.
                !shown && player.playing -> shown = true
                !shown && SystemClock.uptimeMillis() - opened > GIVE_UP_MS -> {
                    failed = true
                    return@LaunchedEffect
                }
            }
        }
    }
    // The sound follows the picture, up and down, in small steps.
    var unmuted by remember(embed) { mutableStateOf(false) }
    LaunchedEffect(shown) {
        val view = webView[0] ?: return@LaunchedEffect
        if (!shown && !unmuted) return@LaunchedEffect
        if (shown) {
            view.evaluateJavascript("(function(){var p=document.getElementById('movie_player');if(p){p.setVolume(0);p.unMute();}})()", null)
            unmuted = true
        }
        val steps = 40
        val fade = (if (shown) FADE_IN_MS else FADE_OUT_MS).toLong()
        for (step in 1..steps) {
            delay(fade / steps)
            // Loudness follows the square of the level better than the level
            // itself: a straight ramp sounds as if it jumps up at once.
            val level = (if (shown) step else steps - step).toDouble() / steps
            val volume = (level * level * 100).toInt()
            view.evaluateJavascript("(function(){var p=document.getElementById('movie_player');if(p)p.setVolume($volume);})()", null)
        }
    }

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
                    view.alpha = 0f
                    view.setBackgroundColor(android.graphics.Color.BLACK)
                    view.settings.javaScriptEnabled = true
                    view.settings.mediaPlaybackRequiresUserGesture = false
                    view.webViewClient = WebViewClient()
                    view.loadUrl(embed, identity(context.packageName))
                }
            },
            // Runs every frame of a fade: only the alpha changes here.
            update = { view -> view.alpha = alpha },
            modifier = Modifier.fillMaxSize(),
        )
        if (failed) {
            Text(
                "This trailer can't play here.",
                color = Secondary,
                fontSize = 20.sp,
                modifier = Modifier.align(Alignment.Center),
            )
        }
    }
    DisposableEffect(Unit) {
        onDispose {
            webView[0]?.stopLoading()
            webView[0]?.destroy()
            webView[0] = null
        }
    }
}
