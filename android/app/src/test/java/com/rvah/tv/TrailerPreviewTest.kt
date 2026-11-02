package com.rvah.tv

import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class TrailerPreviewTest {
    private val down = KeyEventType.KeyDown
    private val up = KeyEventType.KeyUp

    @Test fun aTrailerLoadingUnseenLeavesKeysToTheBoard() {
        val preview = TrailerPreview()
        preview.load("https://youtu.be/abc_123-Z", showAt = 10_000) {}
        assertFalse(preview.onKey(down, Key.DirectionRight))
        assertFalse(preview.onKey(up, Key.DirectionRight))
    }

    @Test fun aKeyDismissesAShownTrailerAndDoesNothingElse() {
        val preview = TrailerPreview()
        var shown = 0
        val request = preview.load("https://youtu.be/abc_123-Z", showAt = 10_000) { shown++ }
        preview.markShown(request)
        assertEquals(1, shown)
        assertTrue(preview.shown)

        assertTrue(preview.onKey(down, Key.DirectionRight))
        assertNull(preview.request)
        assertFalse(preview.shown)
        // Its release is swallowed too; the next press is the board's.
        assertTrue(preview.onKey(up, Key.DirectionRight))
        assertFalse(preview.onKey(down, Key.DirectionRight))
        assertFalse(preview.onKey(up, Key.DirectionRight))
    }

    @Test fun anOldRequestNeitherStopsNorShowsANewerOne() {
        val preview = TrailerPreview()
        val first = preview.load("https://youtu.be/aaaaaaaa", showAt = 10_000) {}
        var shown = false
        val second = preview.load("https://youtu.be/bbbbbbbb", showAt = 20_000) { shown = true }
        preview.cancel(first)
        preview.markShown(first)
        assertSame(second, preview.request)
        assertFalse(preview.shown)
        assertFalse(shown)
        preview.cancel(second)
        assertNull(preview.request)
    }
}
