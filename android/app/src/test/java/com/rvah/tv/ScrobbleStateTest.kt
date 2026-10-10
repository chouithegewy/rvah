package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.rvah_android.ScrobbleChange

class ScrobbleStateTest {
    @Test fun aSessionStartsPausesResumesAndStopsOnce() {
        val state = ScrobbleState()
        assertEquals(ScrobbleChange.START, state.playing())
        assertEquals(null, state.playing())
        assertEquals(ScrobbleChange.PAUSE, state.paused())
        assertEquals(null, state.paused())
        assertEquals(ScrobbleChange.START, state.playing())
        assertEquals(ScrobbleChange.STOP, state.stopped())
        assertEquals(null, state.stopped())
        assertEquals(null, state.playing())
    }

    @Test fun nothingPausesBeforeItStarts() {
        val state = ScrobbleState()
        assertEquals(null, state.paused())
        assertEquals(ScrobbleChange.STOP, state.stopped())
    }

    @Test fun progressIsAPercentageOfTheDuration() {
        assertEquals(50f, scrobbleProgress(30_000, 60_000), 0.001f)
        assertEquals(100f, scrobbleProgress(70_000, 60_000), 0.001f)
        assertEquals(0f, scrobbleProgress(30_000, null), 0.001f)
    }
}
