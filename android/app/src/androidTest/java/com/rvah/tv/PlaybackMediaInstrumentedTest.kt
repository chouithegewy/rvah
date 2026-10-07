package com.rvah.tv

import androidx.media3.common.MimeTypes
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/** MediaItem construction needs android.net.Uri, which is only a stub on the JVM. */
@RunWith(AndroidJUnit4::class)
class PlaybackMediaInstrumentedTest {
    @Test fun mediaItemCarriesResumeSeparatelyAndIncludesSubtitles() {
        val spec = PlaybackSpec(
            sessionId = 7u,
            url = "https://x/master.m3u8",
            protocol = StreamProtocol.Hls,
            headers = mapOf("Referer" to "https://x"),
            resumePositionMs = 12_500,
            subtitles = listOf(SubtitleSpec("https://x/en.vtt", "en", "English", MimeTypes.TEXT_VTT)),
            title = "Fixture",
        )
        val item = spec.toMediaItem()
        assertEquals(MimeTypes.APPLICATION_M3U8, item.localConfiguration?.mimeType)
        assertEquals(1, item.localConfiguration?.subtitleConfigurations?.size)
        assertEquals("Fixture", item.mediaMetadata.title.toString())
    }
}
