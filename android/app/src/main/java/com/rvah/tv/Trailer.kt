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
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEvent
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.type
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
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

/** A trailer opened from its button that has not started by now is not going to. */
private const val GIVE_UP_MS = 15_000L

/** A title in focus this long starts loading its trailer, unseen. */
internal const val PREVIEW_LOAD_AFTER_MS = 3_000L

/** ...and this long, its trailer may show (once it plays without YouTube's controls). */
internal const val PREVIEW_AT_MS = 10_000L

/** A preview not playing by now (from when it began loading) is dropped quietly. */
private const val PREVIEW_GIVE_UP_MS = 30_000L

/**
 * YouTube lays its title, a pause button, "More videos" and its logo over
 * the first seconds of playback whatever the embed options say. A preview
 * waits for them to go; past this much playing, it stops waiting.
 */
private const val CONTROLS_WAIT_MS = 8_000L

/** Where the embedded player stands; the embed page is the WebView's own page. */
private const val PLAYER_STATE = """(function () {
  var p = document.getElementById('movie_player');
  var c = document.querySelector('#player-control-overlay .player-controls-content');
  return {
    state: p && p.getPlayerState ? p.getPlayerState() : -2,
    time: p && p.getCurrentTime ? p.getCurrentTime() : 0,
    duration: p && p.getDuration ? p.getDuration() : 0,
    error: !!document.querySelector('.ytp-error'),
    controls: c ? getComputedStyle(c).visibility === 'visible' : null,
    captions: !!document.querySelector('.caption-window')
  };
})()"""

/**
 * YouTube turns captions on for a video that starts muted, as these do, and
 * leaves them on once the sound is up.
 */
private const val NO_CAPTIONS =
    "(function(){var p=document.getElementById('movie_player');if(p&&p.unloadModule)p.unloadModule('captions');})()"

/** [controls]: whether YouTube's own controls show; null when that can't be told. */
private data class PlayerState(
    val state: Int,
    val time: Double,
    val duration: Double,
    val error: Boolean,
    val controls: Boolean?,
    val captions: Boolean,
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
                    controls = if (it.isNull("controls")) null else it.optBoolean("controls"),
                    captions = it.optBoolean("captions"),
                )
            },
        )
    }
}

/**
 * A YouTube trailer, invisible and silent while it loads and starts, and
 * without the captions YouTube adds to a muted start. Once
 * it plays (and, with [showAt], not before that uptime; with [bare], once
 * YouTube's own controls have gone), picture and sound fade in together
 * over [FADE_IN_MS] while it plays on. Near the end both fade out and
 * [onEnd] runs, before YouTube's end screen; it runs too when the app goes
 * out of sight.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun TrailerPlayer(
    videoId: String,
    showAt: Long?,
    bare: Boolean,
    giveUpMs: Long,
    focusable: Boolean,
    onShown: () -> Unit,
    onEnd: () -> Unit,
    onFailed: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val embed = remember(videoId) { trailerEmbed(videoId) }
    val webView = remember { arrayOfNulls<WebView>(1) }
    var shown by remember(embed) { mutableStateOf(false) }
    val alpha by animateFloatAsState(
        targetValue = if (shown) 1f else 0f,
        // Even steps of opacity look like an even fade; an eased curve
        // that front-loads the change just looks like the picture appearing.
        animationSpec = tween(if (shown) FADE_IN_MS else FADE_OUT_MS, easing = LinearEasing),
        label = "trailer fade",
    )
    val shownNow by rememberUpdatedState(onShown)
    val endNow by rememberUpdatedState(onEnd)
    val failedNow by rememberUpdatedState(onFailed)

    LaunchedEffect(embed) {
        val opened = SystemClock.uptimeMillis()
        var playingSince: Long? = null
        while (true) {
            // Quick to notice the start, so the fade begins with the video.
            delay(if (shown) 250 else 100)
            val player = webView[0]?.playerState() ?: continue
            if (player.captions) webView[0]?.evaluateJavascript(NO_CAPTIONS, null)
            val now = SystemClock.uptimeMillis()
            playingSince = if (player.playing) playingSince ?: now else null
            when {
                player.error -> {
                    failedNow()
                    return@LaunchedEffect
                }
                shown && player.ending -> {
                    shown = false
                    delay(FADE_OUT_MS.toLong())
                    endNow()
                    return@LaunchedEffect
                }
                // Over before it ever showed (a very short video): just close.
                !shown && player.state == 0 -> {
                    endNow()
                    return@LaunchedEffect
                }
                // Playing (and due, and bare): picture and sound fade in while it plays.
                !shown && playingSince != null &&
                    (showAt == null || now >= showAt) &&
                    (!bare || player.controls != true || now - playingSince >= CONTROLS_WAIT_MS) -> {
                    shown = true
                    shownNow()
                }
                !shown && playingSince == null && now - opened > giveUpMs -> {
                    failedNow()
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

    // Out of sight (the Home button, the screensaver), it would play on
    // unheard, or fade in with sound: it ends instead.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) endNow() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }

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
                if (!focusable) {
                    // Over a board, focus stays on the board's titles.
                    view.isFocusable = false
                    view.isFocusableInTouchMode = false
                    view.descendantFocusability = ViewGroup.FOCUS_BLOCK_DESCENDANTS
                }
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
        modifier = modifier.fillMaxSize(),
    )
    DisposableEffect(Unit) {
        onDispose {
            webView[0]?.stopLoading()
            webView[0]?.destroy()
            webView[0] = null
        }
    }
}

/**
 * A trailer opened from its button: black while it loads, then, as soon as
 * it plays, picture and sound fade in. YouTube's own title and logo show
 * through the first seconds. It closes itself at the end.
 */
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
    var failed by remember(videoId) { mutableStateOf(false) }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        TrailerPlayer(
            videoId = videoId,
            showAt = null,
            bare = false,
            giveUpMs = GIVE_UP_MS,
            focusable = true,
            onShown = {},
            onEnd = onExit,
            onFailed = { failed = true },
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
}

/**
 * The trailer a board plays over the whole screen when a title stays in
 * focus: [load] starts it unseen, and it shows only once it plays bare and
 * is due. Moving on cancels it; once it shows, any key dismisses it (and
 * does nothing else).
 */
@Stable
internal class TrailerPreview {
    class Request(val url: String, val showAt: Long, val onShown: () -> Unit)

    var request by mutableStateOf<Request?>(null)
        private set

    /** Fading in or playing in view (not just loading unseen). */
    var shown by mutableStateOf(false)
        private set

    /** The key that dismissed a shown trailer, whose release is swallowed too. */
    private var dismissedBy: Key? = null

    fun load(url: String, showAt: Long, onShown: () -> Unit): Request =
        Request(url, showAt, onShown).also {
            request = it
            shown = false
        }

    /** Stop [which], if it is still the one loading or playing. */
    fun cancel(which: Request) {
        if (request === which) {
            request = null
            shown = false
        }
    }

    fun markShown(which: Request) {
        if (request === which) {
            shown = true
            which.onShown()
        }
    }

    /** For the screen's key handler: whether [event] was the preview's. */
    fun onKey(event: KeyEvent): Boolean = onKey(event.type, event.key)

    internal fun onKey(type: KeyEventType, key: Key): Boolean {
        val playing = request
        if (shown && playing != null) {
            if (type == KeyEventType.KeyDown) {
                dismissedBy = key
                cancel(playing)
            }
            return true
        }
        if (type == KeyEventType.KeyUp && key == dismissedBy) {
            dismissedBy = null
            return true
        }
        return false
    }
}

internal val LocalTrailerPreview = staticCompositionLocalOf { TrailerPreview() }

/** Where [TrailerPreview]'s trailer plays: above everything else on screen. */
@Composable
internal fun TrailerPreviewLayer(preview: TrailerPreview) {
    val request = preview.request ?: return
    val videoId = remember(request) { youtubeVideoId(request.url) } ?: return
    key(request) {
        TrailerPlayer(
            videoId = videoId,
            showAt = request.showAt,
            bare = true,
            giveUpMs = PREVIEW_GIVE_UP_MS,
            focusable = false,
            onShown = { preview.markShown(request) },
            onEnd = { preview.cancel(request) },
            onFailed = { preview.cancel(request) },
        )
    }
}
