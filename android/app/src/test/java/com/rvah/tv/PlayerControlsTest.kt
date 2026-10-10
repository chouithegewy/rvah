package com.rvah.tv

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayerControlsTest {
    @Test fun controlsHideOnlyWhilePlaying() {
        assertTrue(controlsAutoHide(started = true, isPlaying = true, failed = false))
        assertFalse(controlsAutoHide(started = true, isPlaying = false, failed = false))
        assertFalse(controlsAutoHide(started = false, isPlaying = false, failed = false))
        assertFalse(controlsAutoHide(started = true, isPlaying = true, failed = true))
    }
}
