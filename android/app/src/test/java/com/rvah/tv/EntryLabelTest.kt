package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.rvah_android.CatalogEntry

class EntryLabelTest {
    private fun folder(title: String, subtitle: String?) =
        CatalogEntry.Directory("p", title, subtitle, "p", null, false)

    @Test fun folderRowsShowTheirSubtitle() {
        assertEquals("▸ Bulls vs Celtics  ·  LIVE · 2nd · 10–20", entryLabel(folder("Bulls vs Celtics", "LIVE · 2nd · 10–20")))
        assertEquals("▸ NBA", entryLabel(folder("NBA", null)))
        assertEquals("▸ NBA", entryLabel(folder("NBA", " ")))
    }
}
