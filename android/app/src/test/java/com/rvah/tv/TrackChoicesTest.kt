package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test

class TrackChoicesTest {
    @Test fun aTrackIsNamedByLanguageThenLabelChannelsAndForced() {
        assertEquals("English · 5.1", trackName("en", null, 6, forced = false))
        assertEquals("Japanese · stereo", trackName("ja", "Japanese", 2, forced = false))
        assertEquals("English · Commentary · stereo", trackName("en", "Commentary", 2, forced = false))
        assertEquals("English · SDH", trackName("en", "SDH", 0, forced = false))
        assertEquals("English · Signs & Songs · forced", trackName("en", "Signs & Songs", -1, forced = true))
        assertEquals("Spanish · 7.1", trackName("es-419", "", 8, forced = false))
    }

    @Test fun aTrackWithoutALanguageGoesByItsLabel() {
        assertEquals("Director's cut · mono", trackName("und", "Director's cut", 1, forced = false))
        assertEquals("Unknown language", trackName(null, null, 0, forced = false))
        assertEquals("Unknown language · 3 ch", trackName("", " ", 3, forced = false))
    }

    @Test fun repeatedNamesAreNumbered() {
        assertEquals(
            listOf("English", "Japanese", "English (2)", "English (3)"),
            distinctNames(listOf("English", "Japanese", "English", "English")),
        )
    }

    @Test fun buttonsShowTheLanguageAlone() {
        assertEquals("English", shortTrackName("English · Commentary · stereo"))
        assertEquals("Off", shortTrackName("Off"))
    }
}
