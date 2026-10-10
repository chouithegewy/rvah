package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test

class DetailsTextTest {
    @Test fun factsLineJoinsWhatIsKnown() {
        assertEquals(
            "2026 · 1 h 31 min · Comedy, Animation",
            detailsLine(2026u, 91u * 60u, listOf("Comedy", "Animation")),
        )
        assertEquals("2026 · 45 min", detailsLine(2026u, 45u * 60u, emptyList()))
        assertEquals("2 h", detailsLine(null, 120u * 60u, emptyList()))
        assertEquals("", detailsLine(null, null, emptyList()))
    }

    @Test fun ratingSourcesHaveNames() {
        assertEquals("Rotten Tomatoes", ratingLabel("rotten_tomatoes"))
        assertEquals("RT Audience", ratingLabel("rt_audience"))
        assertEquals("IMDb", ratingLabel("imdb"))
        assertEquals("TMDB", ratingLabel("tmdb"))
        assertEquals("someone_new", ratingLabel("someone_new"))
    }

    @Test fun findingATitleSearchesItsWordsAndYear() {
        assertEquals("Coyote vs. Acme 2026", findQuery("Coyote vs. Acme", 2026u))
        assertEquals("Coyote vs. Acme", findQuery("Coyote vs. Acme", null))
    }
}
