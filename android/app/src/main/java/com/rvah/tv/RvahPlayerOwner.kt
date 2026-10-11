package com.rvah.tv

import android.app.ActivityManager
import android.content.Context
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.media3.common.C
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.TrackGroup
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.Tracks
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.exoplayer.DefaultLoadControl
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
import uniffi.rvah_android.ScrobbleChange
import uniffi.rvah_android.ScrobbleTarget

/** What the player screen renders; updated from ExoPlayer callbacks (main thread). */
data class PlayerUi(
    val isPlaying: Boolean = false,
    val status: String = "",
    val failure: String? = null,
    /** Media buffered ahead of the playhead; only tracked in debug builds. */
    val bufferedAheadMs: Long = 0,
    /** The file's audio tracks this device can play. */
    val audio: List<TrackOption> = emptyList(),
    /** The file's subtitle tracks this device can show. */
    val subtitles: List<TrackOption> = emptyList(),
)

/** One track of the playing file, as the Audio or Subtitles menu offers it. */
data class TrackOption(val name: String, val selected: Boolean, val group: TrackGroup, val index: Int)

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

    /** This device's read-ahead budget; see [BufferPlan]. */
    val bufferPlan: BufferPlan = run {
        val activities = context.getSystemService(ActivityManager::class.java)
        val memory = ActivityManager.MemoryInfo().also { activities.getMemoryInfo(it) }
        bufferPlan(Runtime.getRuntime().maxMemory(), memory.totalMem, activities.isLowRamDevice)
            .also { if (BuildConfig.DEBUG) Log.d("rvah", "buffer plan: $it") }
    }
    val player: ExoPlayer = ExoPlayer.Builder(context)
        .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory))
        .setLoadControl(
            DefaultLoadControl.Builder()
                .setBufferDurationsMs(
                    bufferPlan.minBufferMs,
                    bufferPlan.maxBufferMs,
                    bufferPlan.startMs,
                    bufferPlan.restartMs,
                )
                .setTargetBufferBytes(bufferPlan.targetBytes)
                .build(),
        )
        .build()
        .also {
            it.trackSelectionParameters = it.trackSelectionParameters.withLanguageDefaults()
            it.addListener(this)
        }
    private val mediaSession = MediaSession.Builder(context, player).build()
    private var active: PlaybackSpec? = null
    private var progressJob: Job? = null
    /** What scrobbling add-ons hear about, for a title they can name; else null. */
    private var scrobbleTarget: ScrobbleTarget? = null
    private var scrobbles = ScrobbleState()

    fun play(spec: PlaybackSpec, scrobble: ScrobbleTarget? = null) {
        active = spec
        scrobbleTarget = scrobble
        scrobbles = ScrobbleState()
        ui = PlayerUi(status = "Buffering…")
        requestHeaders.set(spec.headers)
        // Each video starts from the language defaults, whatever was picked for the last.
        player.trackSelectionParameters = player.trackSelectionParameters.forNewVideo()
        report(NativePlaybackEvent.Preparing)
        player.setMediaItem(spec.toMediaItem(), spec.resumePositionMs)
        player.prepare()
        player.play()
        progressJob?.cancel()
        progressJob = scope.launch {
            while (isActive) {
                delay(1_000)
                if (BuildConfig.DEBUG) ui = ui.copy(bufferedAheadMs = player.totalBufferedDuration)
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

    /** Play [option]'s track: an audio track, or subtitles (turning them on). */
    fun select(option: TrackOption) {
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon()
            .setTrackTypeDisabled(option.group.type, false)
            .setOverrideForType(TrackSelectionOverride(option.group, option.index))
            .build()
    }

    fun subtitlesOff() {
        player.trackSelectionParameters = player.trackSelectionParameters.buildUpon()
            .clearOverridesOfType(C.TRACK_TYPE_TEXT)
            .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
            .build()
    }

    fun stop() {
        progressJob?.cancel()
        scrobble(scrobbles.stopped())
        scrobbleTarget = null
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
                scrobble(scrobbles.stopped())
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
        // A stall while buffering is not a pause.
        if (isPlaying) scrobble(scrobbles.playing()) else if (ready) scrobble(scrobbles.paused())
    }

    /** Tell scrobbling add-ons, off the main thread; failures only reach the log. */
    private fun scrobble(change: ScrobbleChange?) {
        val target = scrobbleTarget ?: return
        val core = core ?: return
        change ?: return
        val progress = scrobbleProgress(player.currentPosition, durationMs()?.toLong())
        scope.launch(Dispatchers.IO) {
            runCatching { core.scrobble(change, target, progress) }
                .onSuccess { failed -> if (BuildConfig.DEBUG && failed.isNotEmpty()) Log.d("rvah", "scrobble $change: $failed") }
        }
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

    /** The menus' choices; debug builds also log each file's tracks, selected ones starred. */
    override fun onTracksChanged(tracks: Tracks) {
        ui = ui.copy(audio = options(tracks, C.TRACK_TYPE_AUDIO), subtitles = options(tracks, C.TRACK_TYPE_TEXT))
        if (!BuildConfig.DEBUG) return
        for ((type, name) in listOf(C.TRACK_TYPE_AUDIO to "audio", C.TRACK_TYPE_TEXT to "subtitles")) {
            val line = tracks.groups.filter { it.type == type }.flatMap { group ->
                (0 until group.length).map { i ->
                    val format = group.getTrackFormat(i)
                    val default = if (format.selectionFlags and C.SELECTION_FLAG_DEFAULT != 0) " default" else ""
                    val forced = if (format.selectionFlags and C.SELECTION_FLAG_FORCED != 0) " forced" else ""
                    val mark = if (group.isTrackSelected(i)) "*" else ""
                    "$mark${format.language ?: "und"}${format.label?.let { " \"$it\"" } ?: ""}$default$forced"
                }
            }
            Log.d("rvah", "$name: ${line.joinToString(", ").ifEmpty { "none" }}")
        }
    }

    private fun options(tracks: Tracks, type: Int): List<TrackOption> {
        val playable = tracks.groups.filter { it.type == type }.flatMap { group ->
            (0 until group.length).filter { group.isTrackSupported(it) }.map { i ->
                val format = group.getTrackFormat(i)
                val name = trackName(
                    format.language,
                    format.label,
                    if (type == C.TRACK_TYPE_AUDIO) format.channelCount else 0,
                    forced = format.selectionFlags and C.SELECTION_FLAG_FORCED != 0,
                )
                TrackOption(name, group.isTrackSelected(i), group.mediaTrackGroup, i)
            }
        }
        return playable.zip(distinctNames(playable.map { it.name })) { option, name -> option.copy(name = name) }
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
