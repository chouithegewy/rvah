package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.CatalogEntry

class HomeLayoutTest {
    private fun addon(id: String, vararg caps: String, status: String = "enabled") =
        AddonRecord(id, id.substringAfterLast('.'), "1.0.0", status == "enabled", status, caps.toList())

    private fun folder(title: String, path: String, groups: Boolean = false) =
        CatalogEntry.Directory(path, title, null, path, null, groups)

    private fun media(id: String) =
        CatalogEntry.Media(id, id, null, null, null, null, null, "other", null)

    @Test fun discoverComesFirstAndItsComponentsAreNotShownTwice() {
        val addons = listOf(
            addon(TMDB_ADDON, "catalog", "details"),
            addon("com.real-debrid.downloads", "catalog", "auth"),
            addon(DISCOVER_ADDON, "catalog", "auth", "details"),
            addon(TRAKT_ADDON, "catalog", "auth"),
            addon("com.rvah.sources.prowlarr", "sources"),
        )
        val roots = mapOf(
            DISCOVER_ADDON to listOf(folder("New to streaming", "streaming/new"), folder("Anime", "genre/anime")),
            TMDB_ADDON to listOf(folder("Trending", "trending")),
            TRAKT_ADDON to listOf(folder("Trending films", "movies/trending")),
            "com.real-debrid.downloads" to listOf(media("d1")),
        )
        val layout = homeLayout(addons, roots)
        assertEquals(listOf("New to streaming", "Anime"), layout.discover.map { it.title })
        assertEquals(listOf("discover"), layout.discover.map { it.tag }.distinct())
        assertEquals(listOf("com.real-debrid.downloads"), layout.downloads.map { it.addonId })
    }

    @Test fun withoutDiscoverEachAddOnShowsAccountsFirst() {
        val addons = listOf(addon(TMDB_ADDON, "catalog", "details"), addon(TRAKT_ADDON, "catalog", "auth"))
        val roots = mapOf(
            TMDB_ADDON to listOf(folder("Trending", "trending"), folder("Films by genre", "genres/movie", groups = true)),
            TRAKT_ADDON to listOf(folder("You might like", "recommendations")),
        )
        val layout = homeLayout(addons, roots)
        assertEquals(listOf("You might like", "Trending"), layout.discover.map { it.title })
        assertEquals(listOf(GroupTab(TMDB_ADDON, "Films by genre", "genres/movie")), layout.groups)
        // A disabled RVAH Discover hides nothing.
        val withDisabled = addons + addon(DISCOVER_ADDON, "catalog", status = "disabled")
        assertEquals(2, homeLayout(withDisabled, roots).discover.size)
    }

    @Test fun cardLinesAndGenreToggles() {
        assertEquals("2026 · ★ 8.4", cardLine(2026u, 8.4f))
        assertEquals("2026", cardLine(2026u, 0f))
        assertEquals("", cardLine(null, null))
        assertEquals("Anime,Drama", toggled("Sci-Fi,Anime,Drama", "Sci-Fi"))
        assertEquals("Anime,Drama,Sci-Fi", toggled("Anime,Drama", "Sci-Fi"))
        assertEquals("Horror", toggled("", "Horror"))
    }
}
