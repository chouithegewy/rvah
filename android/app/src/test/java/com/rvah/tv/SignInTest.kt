package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test

class SignInTest {
    @Test fun urlIsShownTheWayItIsTyped() {
        assertEquals("real-debrid.com/device", displayUrl("https://real-debrid.com/device"))
        assertEquals("real-debrid.com/device", displayUrl("https://real-debrid.com/device/"))
        assertEquals("example.com/link?x=1", displayUrl("https://example.com/link?x=1"))
    }

    @Test fun remainingTimeIsMinutesAndSeconds() {
        assertEquals("29:05", formatRemaining(1745))
        assertEquals("0:59", formatRemaining(59))
        assertEquals("0:00", formatRemaining(-3))
    }
}
