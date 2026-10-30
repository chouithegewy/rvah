package com.rvah.tv

import androidx.media3.common.C
import androidx.media3.common.MimeTypes
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Test

class PlaybackMediaTest {
    @Test fun protocolMappingIsExplicit() {
        assertEquals(MimeTypes.APPLICATION_M3U8, playbackMimeType(StreamProtocol.Hls, "https://x/video"))
        assertEquals(MimeTypes.APPLICATION_MPD, playbackMimeType(StreamProtocol.Dash, "https://x/video"))
        assertEquals(MimeTypes.VIDEO_WEBM, playbackMimeType(StreamProtocol.Progressive, "https://x/video.webm?token=x"))
        assertEquals(MimeTypes.VIDEO_MP4, playbackMimeType(StreamProtocol.Progressive, "https://x/video.mp4"))
    }

    @Test fun subtitleFormatFallsBackToExtension() {
        assertEquals(MimeTypes.TEXT_VTT, inferSubtitleMimeType("https://x/en.vtt?token=x"))
        assertEquals(MimeTypes.APPLICATION_TTML, inferSubtitleMimeType("https://x/en.ttml"))
        assertEquals(MimeTypes.TEXT_SSA, inferSubtitleMimeType("https://x/en.ass"))
        assertEquals(MimeTypes.APPLICATION_SUBRIP, inferSubtitleMimeType("https://x/en.srt"))
    }

    @Test fun requestedOnlineSubtitleIsMarkedDefault() {
        val english = "https://subs.example/en.vtt"
        val spanish = "https://subs.example/es.srt"
        assertEquals(0, subtitleSelectionFlags(english, spanish))
        assertTrue(subtitleSelectionFlags(spanish, spanish) and C.SELECTION_FLAG_DEFAULT != 0)
    }

    @Test fun onlineSubtitleCanBeMatchedAfterMedia3RewritesItsId() {
        val wanted = SubtitleSpec("https://subs.example/en.srt", "en", "English", MimeTypes.APPLICATION_SUBRIP)
        assertTrue(subtitleTrackMatches(wanted.url, "und", null, wanted))
        assertTrue(subtitleTrackMatches("generated-id", "EN", "English", wanted))
        assertFalse(subtitleTrackMatches("generated-id", "es", "Spanish", wanted))
        assertFalse(subtitleTrackMatches("generated-id", "en", null, wanted.copy(label = null)))
    }
}
