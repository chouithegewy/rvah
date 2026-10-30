package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.CatalogEntry

class HomeLayoutTest {
    private fun addon(id: String, vararg caps: String, status: String = "enabled", name: String = id.substringAfterLast('.')) =
        AddonRecord(id, name, "1.0.0", status == "enabled", status, caps.toList())

    private fun folder(title: String, path: String, groups: Boolean = false) =
        CatalogEntry.Directory(path, title, null, path, null, groups)

    private fun media(id: String) =
        CatalogEntry.Media(id, id, null, null, null, null, null, "other", null, null)

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
        // One add-on gives every column: no need to name it under each.
        assertEquals(listOf<String?>(null), layout.discover.map { it.tag }.distinct())
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
        // Columns from two add-ons: each says whose it is.
        assertEquals(listOf("trakt", "discover"), layout.discover.map { it.tag })
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

    @Test fun homeCondensesProviderFoldersIntoStableTiles() {
        val discover = listOf(
            ColumnSpec(DISCOVER_ADDON, "Popular to stream", null, "popular"),
            ColumnSpec(DISCOVER_ADDON, "New to streaming", null, "new"),
            ColumnSpec(DISCOVER_ADDON, "Up next", null, "up-next"),
            ColumnSpec(DISCOVER_ADDON, "Anime", null, "genre/anime"),
            ColumnSpec(DISCOVER_ADDON, "Documentaries", null, "docs"),
        )
        val layout = HomeLayout(
            discover,
            listOf(GroupTab(DISCOVER_ADDON, "Genres", "genres")),
            listOf(ColumnSpec("downloads", "Downloads", null, null)),
        )

        val sections = homeSections(layout)
        assertEquals(
            listOf("Trending & popular", "Recently added", "Genres", "Trakt", "Downloads", "More to explore"),
            sections.map { it.title },
        )
        assertEquals(listOf("Popular to stream"), sections.first { it.id == "trending" }.columns.map { it.title })
        assertEquals(listOf("Anime"), sections.first { it.id == "genres" }.columns.map { it.title })
        assertEquals(listOf("Up next"), sections.first { it.id == "trakt" }.columns.map { it.title })
    }

    @Test fun addOnWhoseFoldersAllFallToMoreGetsItsOwnBrowseTile() {
        val addons = listOf(addon(DISCOVER_ADDON, "catalog"), addon("com.example.sports", "catalog", name = "Live Sports"))
        val roots = mapOf(
            DISCOVER_ADDON to listOf(folder("Trending", "trending"), folder("Odd shelf", "odd")),
            "com.example.sports" to listOf(folder("NFL", "/nfl/"), folder("NBA", "/nba/")),
        )
        val sections = homeSections(homeLayout(addons, roots))
        val tile = sections.single { it.browse != null }
        assertEquals(BrowseTarget("com.example.sports", "Live Sports"), tile.browse)
        assertEquals("addon:com.example.sports", tile.id)
        assertEquals("Live Sports", tile.title)
        assertEquals(emptyList<ColumnSpec>(), tile.columns)
        // Its folders are not merged into More; Discover's odd shelf still is.
        assertEquals(listOf("Odd shelf"), sections.single { it.id == "more" }.columns.map { it.title })
        // Tiles for add-ons come after the fixed sections, before More.
        assertEquals(listOf("trending", "addon:com.example.sports", "more"), sections.map { it.id })
    }

    @Test fun addOnWithAnyMatchingFolderStaysInSections() {
        val addons = listOf(addon("com.example.mixed", "catalog", name = "Mixed"))
        val roots = mapOf("com.example.mixed" to listOf(folder("Trending", "trending"), folder("NFL", "/nfl/")))
        val sections = homeSections(homeLayout(addons, roots))
        assertEquals(null, sections.firstOrNull { it.browse != null })
        assertEquals(listOf("NFL"), sections.single { it.id == "more" }.columns.map { it.title })
    }

    @Test fun addOnWithGroupsStaysInSections() {
        val addons = listOf(addon("com.example.grouped", "catalog", name = "Grouped"))
        val roots = mapOf(
            "com.example.grouped" to listOf(folder("Odd", "odd"), folder("By genre", "genres", groups = true)),
        )
        assertEquals(null, homeSections(homeLayout(addons, roots)).firstOrNull { it.browse != null })
    }

    @Test fun aFirstLevelOfFoldersIsABoardUnlessOpenedAsAList() {
        val folders = listOf(folder("NFL", "/nfl/"), folder("NBA", "/nba/"))
        assertEquals(true, firstLevelBoard(depth = 1, entries = folders, listFirst = false))
        // An add-on's own Home tile opens on a list of its folders.
        assertEquals(false, firstLevelBoard(depth = 1, entries = folders, listFirst = true))
        assertEquals(false, firstLevelBoard(depth = 2, entries = folders, listFirst = false))
        assertEquals(false, firstLevelBoard(depth = 1, entries = folders + media("m"), listFirst = false))
    }
}
