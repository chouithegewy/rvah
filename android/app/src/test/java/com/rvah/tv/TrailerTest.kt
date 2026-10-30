package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TrailerTest {
    @Test fun extractsSupportedYouTubeLinks() {
        assertEquals("abc_123-Z", youtubeVideoId("https://www.youtube.com/watch?v=abc_123-Z&feature=share"))
        assertEquals("abc_123-Z", youtubeVideoId("https://youtu.be/abc_123-Z?t=2"))
        assertEquals("abc_123-Z", youtubeVideoId("https://www.youtube.com/embed/abc_123-Z"))
        assertEquals("abc_123-Z", youtubeVideoId("https://www.youtube.com/shorts/abc_123-Z?feature=share"))
    }

    @Test fun rejectsNonYouTubeAndUnsafeIds() {
        assertNull(youtubeVideoId("https://example.com/watch?v=abc_123-Z"))
        assertNull(youtubeVideoId("https://youtube.com/watch?v=<script>"))
    }
}
