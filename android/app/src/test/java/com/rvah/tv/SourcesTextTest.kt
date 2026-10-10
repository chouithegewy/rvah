package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test

class SourcesTextTest {
    @Test fun releaseLineShowsResolutionSizeSeedersAndOrigin() {
        assertEquals(
            "2160p · 23.4 GB · 512 seeders · Example",
            releaseLine("Film.2026.2160p.UHD.BluRay.REMUX", 23_400_000_000u, 512u, "Example"),
        )
        assertEquals("1080p · 1 seeder", releaseLine("Show.S01E02.1080p.WEB", null, 1u, null))
        assertEquals("850 MB", releaseLine("Show S01E02", 850_000_000u, null, ""))
    }

    @Test fun resolutionIsReadFromTheName() {
        assertEquals("2160p", resolution("Film.2026.4K.HDR"))
        assertEquals("2160p", resolution("film 2160p"))
        assertEquals("720p", resolution("Show.S01E02.720p.HDTV"))
        assertEquals(null, resolution("Show.S01E02.HDTV"))
    }

    @Test fun anEpisodeIsNamedWithItsNumbers() {
        assertEquals("Acme Corp · S01E02", sourcesTitle("The Next One", "Acme Corp", 1u, 2u))
        assertEquals("Coyote vs. Acme", sourcesTitle("Coyote vs. Acme", null, null, null))
    }
}
