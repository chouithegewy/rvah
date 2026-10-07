package com.rvah.tv

import android.content.Context
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

class RvahPlayerOwner(
    context: Context,
    private val scope: CoroutineScope,
    private val core: AndroidCore,
) : Player.Listener {
    var onFailure: (String) -> Unit = {}
    private val requestHeaders = AtomicReference<Map<String, String>>(emptyMap())
    private val httpFactory = DefaultHttpDataSource.Factory().setAllowCrossProtocolRedirects(true)
    private val dataSourceFactory = ResolvingDataSource.Factory(
        DefaultDataSource.Factory(context, httpFactory),
    ) { dataSpec -> dataSpec.withAdditionalHttpRequestHeaders(requestHeaders.get()) }
    val player: ExoPlayer = ExoPlayer.Builder(context)
        .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory))
        .build()
        .also { it.addListener(this) }
    private val mediaSession = MediaSession.Builder(context, player).build()
    private var active: PlaybackSpec? = null
    private var progressJob: Job? = null

    fun play(spec: PlaybackSpec) {
        active = spec
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

    fun seekBy(deltaMs: Long) {
        val duration = player.duration.takeIf { it != C.TIME_UNSET } ?: Long.MAX_VALUE
        player.seekTo((player.currentPosition + deltaMs).coerceIn(0L, duration))
    }

    fun stop() {
        val session = active ?: return
        player.stop()
        scope.launch(Dispatchers.IO) { core.stopPlayback(session.sessionId) }
        active = null
    }

    override fun onPlaybackStateChanged(state: Int) {
        when (state) {
            Player.STATE_BUFFERING -> report(NativePlaybackEvent.Buffering)
            Player.STATE_READY -> if (player.isPlaying) reportPlaying() else reportPaused()
            Player.STATE_ENDED -> report(
                NativePlaybackEvent.Ended(
                    player.currentPosition.toULong(),
                    durationMs(),
                ),
            )
        }
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
        if (isPlaying) reportPlaying() else if (player.playbackState == Player.STATE_READY) reportPaused()
    }

    override fun onPlayerError(error: PlaybackException) {
        val message = when (error.errorCode) {
            PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS -> "The video server rejected the request."
            PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED -> "The video network connection failed."
            PlaybackException.ERROR_CODE_DECODING_FAILED -> "This device could not decode the video."
            else -> "Playback failed: ${error.errorCodeName}"
        }
        report(NativePlaybackEvent.Error(message))
        onFailure(message)
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
        scope.launch(Dispatchers.IO) {
            runCatching { core.reportPlaybackEvent(session.sessionId, event) }
                .onFailure { error ->
                    scope.launch(Dispatchers.Main) {
                        onFailure(error.message ?: "Rust playback event failed")
                    }
                }
        }
    }

    fun release() {
        progressJob?.cancel()
        active = null
        mediaSession.release()
        player.release()
    }
}
