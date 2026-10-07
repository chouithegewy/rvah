package com.rvah.tv

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.NativePlaybackEvent

/** What the player screen renders; updated from ExoPlayer callbacks (main thread). */
data class PlayerUi(
    val isPlaying: Boolean = false,
    val status: String = "",
    val failure: String? = null,
)

/**
 * Activity-scoped Media3 player. Rust prepares and validates the source;
 * this class plays it and reports session-scoped state back to the core.
 */
class RvahPlayerOwner(
    context: Context,
    private val scope: CoroutineScope,
) : Player.Listener {
    /** Set once the Rust core has opened; callbacks before that are dropped. */
    @Volatile var core: AndroidCore? = null

    var ui by mutableStateOf(PlayerUi())
        private set

    private val requestHeaders = AtomicReference<Map<String, String>>(emptyMap())
    private val httpFactory = DefaultHttpDataSource.Factory().setAllowCrossProtocolRedirects(true)
    private val dataSourceFactory = ResolvingDataSource.Factory(
        DefaultDataSource.Factory(context, httpFactory),
    ) { dataSpec -> dataSpec.withAdditionalHeaders(requestHeaders.get()) }
    val player: ExoPlayer = ExoPlayer.Builder(context)
        .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory))
        .build()
        .also { it.addListener(this) }
    private val mediaSession = MediaSession.Builder(context, player).build()
    private var active: PlaybackSpec? = null
    private var progressJob: Job? = null

    fun play(spec: PlaybackSpec) {
        active = spec
        ui = PlayerUi(status = "Buffering…")
        requestHeaders.set(spec.headers)
        report(NativePlaybackEvent.Preparing)
        player.setMediaItem(spec.toMediaItem(), spec.resumePositionMs)
        player.prepare()
        player.play()
        progressJob?.cancel()
        progressJob = scope.launch {
            while (isActive) {
                delay(1_000)
                if (player.isPlaying) reportPlaying()
            }
        }
    }

    /** Show a failure that happened before the player got a source. */
    fun fail(message: String) {
        ui = PlayerUi(failure = message)
    }

    fun togglePlayPause() {
        if (player.isPlaying) player.pause() else player.play()
    }

    fun seekBy(deltaMs: Long) {
        val duration = player.duration.takeIf { it != C.TIME_UNSET } ?: Long.MAX_VALUE
        player.seekTo((player.currentPosition + deltaMs).coerceIn(0L, duration))
    }

    fun stop() {
        progressJob?.cancel()
        val session = active
        active = null
        player.stop()
        ui = PlayerUi()
        if (session != null) {
            val core = core ?: return
            scope.launch(Dispatchers.IO) { runCatching { core.stopPlayback(session.sessionId) } }
        }
    }

    override fun onPlaybackStateChanged(state: Int) {
        when (state) {
            Player.STATE_BUFFERING -> {
                ui = ui.copy(status = "Buffering…")
                report(NativePlaybackEvent.Buffering)
            }
            Player.STATE_READY -> {
                ui = ui.copy(status = if (player.isPlaying) "" else "Paused")
                if (player.isPlaying) reportPlaying() else reportPaused()
            }
            Player.STATE_ENDED -> {
                ui = ui.copy(isPlaying = false, status = "Finished")
                report(NativePlaybackEvent.Ended(player.currentPosition.toULong(), durationMs()))
            }
        }
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
        val ready = player.playbackState == Player.STATE_READY
        val status = when {
            isPlaying -> ""
            ready -> "Paused"
            else -> ui.status
        }
        ui = ui.copy(isPlaying = isPlaying, status = status)
        if (isPlaying) reportPlaying() else if (ready) reportPaused()
    }

    override fun onPlayerError(error: PlaybackException) {
        val message = when (error.errorCode) {
            PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS -> "The video server rejected the request."
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED -> "The video network connection failed."
            PlaybackException.ERROR_CODE_DECODING_FAILED -> "This device could not decode the video."
            else -> "Playback failed: ${error.errorCodeName}"
        }
        ui = ui.copy(isPlaying = false, failure = message)
        report(NativePlaybackEvent.Error(message))
    }

    private fun durationMs(): ULong? = player.duration
        .takeIf { it != C.TIME_UNSET && it >= 0 }
        ?.toULong()

    private fun reportPlaying() = report(
        NativePlaybackEvent.Playing(player.currentPosition.toULong(), durationMs()),
    )

    private fun reportPaused() = report(
        NativePlaybackEvent.Paused(player.currentPosition.toULong(), durationMs()),
    )

    private fun report(event: NativePlaybackEvent) {
        val session = active ?: return
        val core = core ?: return
        scope.launch(Dispatchers.IO) {
            // Losing one progress report is harmless; the next one carries the position.
            runCatching { core.reportPlaybackEvent(session.sessionId, event) }
        }
    }

    fun release() {
        progressJob?.cancel()
        active = null
        mediaSession.release()
        player.release()
    }
}
