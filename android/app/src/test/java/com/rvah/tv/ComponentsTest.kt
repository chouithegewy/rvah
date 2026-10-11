package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.rvah_android.AddonRecord

class ComponentsTest {
    private fun addon(id: String, status: String = "enabled") =
        AddonRecord(id, id, "1.0.0", status == "enabled", status, listOf("catalog"))

    @Test fun componentsGoOnlyBesideAnEnabledDiscover() {
        val parts = listOf(addon(TRAKT_ADDON), addon(TMDB_ADDON), addon("com.real-debrid.downloads"))
        assertEquals(emptyList<Pair<String, List<String>>>(), componentsToAbsorb(parts))
        assertEquals(emptyList<Pair<String, List<String>>>(), componentsToAbsorb(parts + addon(DISCOVER_ADDON, "disabled")))
        val absorbed = componentsToAbsorb(parts + addon(DISCOVER_ADDON))
        assertEquals(listOf(TRAKT_ADDON, TMDB_ADDON), absorbed.map { it.first })
        assertEquals(listOf("client_id", "access_token", "refresh_token"), absorbed[0].second)
    }
}
