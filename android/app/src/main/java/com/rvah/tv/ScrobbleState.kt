package com.rvah.tv

import uniffi.rvah_android.ScrobbleChange

/**
 * Which scrobble a change in playback means, per session: start when it
 * plays (again after a pause), pause when the viewer pauses, stop once at
 * the end or on leaving. Repeats are dropped.
 */
class ScrobbleState {
    private var playing = false
    private var stopped = false

    fun playing(): ScrobbleChange? {
        if (stopped || playing) return null
        playing = true
        return ScrobbleChange.START
    }

    fun paused(): ScrobbleChange? {
        if (stopped || !playing) return null
        playing = false
        return ScrobbleChange.PAUSE
    }

    fun stopped(): ScrobbleChange? {
        if (stopped) return null
        stopped = true
        return ScrobbleChange.STOP
    }
}

/** How far in, in percent; 0 when the duration is unknown. */
fun scrobbleProgress(positionMs: Long, durationMs: Long?): Float =
    if (durationMs == null || durationMs <= 0) 0f else (positionMs * 100f / durationMs).coerceIn(0f, 100f)
