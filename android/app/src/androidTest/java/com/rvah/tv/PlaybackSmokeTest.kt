package com.rvah.tv

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PlaybackSmokeTest {
    @Test fun instrumentationHarnessIsInstalled() {
        // The fixture-media task replaces this sentinel with progressive,
        // HLS, and DASH playback assertions on an API-36 TV emulator.
        assertTrue(true)
    }
}
