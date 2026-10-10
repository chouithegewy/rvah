package com.rvah.tv

/** How long the player's controls stay up after the last key press. */
const val CONTROLS_HIDE_AFTER_MS = 3_500L

/**
 * Whether the controls hide by themselves: only while the video plays.
 * Paused, buffering, still resolving or failed, they stay up, since the
 * status line and buttons are what the viewer needs then.
 */
fun controlsAutoHide(started: Boolean, isPlaying: Boolean, failed: Boolean): Boolean =
    started && isPlaying && !failed
