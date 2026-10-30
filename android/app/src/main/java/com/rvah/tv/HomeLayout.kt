package com.rvah.tv

import java.util.Locale
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.CatalogEntry

const val DISCOVER_ADDON = "com.rvah.discover"

/** Add-ons that RVAH Discover nests: shown through it, not again on their own. */
private val DISCOVER_COMPONENTS = setOf(TMDB_ADDON, TRAKT_ADDON)

/** One column of a board: a folder's titles, or what a search found. */
data class ColumnSpec(
    val addonId: String,
    val title: String,
    /** Small print under the title, such as the add-on's name. */
    val tag: String?,
    /** The folder listed (null: the add-on's top level), unless [query] is set. */
    val path: String?,
    val query: String? = null,
) {
    val key: String get() = listOf(addonId, path.orEmpty(), query.orEmpty()).joinToString("\u0000")
}

/** A folder of folders, shown as its own board behind a top-bar tab. */
data class GroupTab(val addonId: String, val title: String, val path: String)

/** What the home screen is made of, from the add-ons' top levels. */
data class HomeLayout(
    val discover: List<ColumnSpec>,
    val groups: List<GroupTab>,
    /** Add-ons whose top level is titles, such as a downloads history. */
    val downloads: List<ColumnSpec>,
    /** Add-on names by id, for tiles of their own. */
    val names: Map<String, String> = emptyMap(),
)

/**
 * Whether a browsed level shows as a board of its folders: a first level of
 * only folders does, unless it was opened as a list ([listFirst], an
 * add-on's own Home tile), where each folder opens in turn.
 */
fun firstLevelBoard(depth: Int, entries: List<CatalogEntry>, listFirst: Boolean): Boolean =
    !listFirst && depth == 1 && entries.all { it is CatalogEntry.Directory }

/** An add-on opened as a list of its folders, from its own Home tile. */
data class BrowseTarget(val addonId: String, val addonName: String)

/** A deliberately small home destination; its catalogs are not loaded until opened. */
data class HomeSection(
    val id: String,
    val title: String,
    val subtitle: String,
    val columns: List<ColumnSpec> = emptyList(),
    val groups: List<GroupTab> = emptyList(),
    /** Set for an add-on's own tile: it opens the add-on's folders as a list. */
    val browse: BrowseTarget? = null,
)

/** Turn many provider folders into a stable, uncluttered two-column home. */
fun homeSections(layout: HomeLayout): List<HomeSection> {
    val remaining = layout.discover.toMutableList()
    fun take(matches: (ColumnSpec) -> Boolean): List<ColumnSpec> {
        val found = remaining.filter(matches)
        remaining.removeAll(found.toSet())
        return found
    }
    val trending = take { spec ->
        spec.title.contains("trending", ignoreCase = true) || spec.title.contains("popular", ignoreCase = true)
    }
    val recent = take { spec ->
        spec.title.contains("recent", ignoreCase = true) ||
            spec.title.contains("new", ignoreCase = true) ||
            spec.title.contains("streaming", ignoreCase = true)
    }
    val genres = take { spec ->
        val path = spec.path.orEmpty().lowercase(Locale.ROOT)
        path.startsWith("genre/") || "/genre/" in path
    }
    val trakt = take { spec ->
        spec.addonId == TRAKT_ADDON ||
            spec.title.contains("trakt", ignoreCase = true) ||
            spec.title.contains("up next", ignoreCase = true) ||
            spec.title.contains("watchlist", ignoreCase = true) ||
            spec.title.contains("might like", ignoreCase = true) ||
            spec.title.contains("recommend", ignoreCase = true)
    }
    // An add-on none of whose folders fits a section above gets a tile of
    // its own, rather than being scattered into More.
    val grouped = layout.groups.map { it.addonId }.toSet() + DISCOVER_ADDON
    val tiles = remaining.map { it.addonId }.distinct()
        .filter { id -> id !in grouped && remaining.count { it.addonId == id } == layout.discover.count { it.addonId == id } }
        .map { id ->
            remaining.removeAll { it.addonId == id }
            val name = layout.names[id] ?: id
            HomeSection("addon:$id", name, "Browse", browse = BrowseTarget(id, name))
        }
    return buildList {
        if (trending.isNotEmpty()) add(HomeSection("trending", "Trending & popular", "What people are watching", trending))
        if (recent.isNotEmpty()) add(HomeSection("recent", "Recently added", "New releases and new to streaming", recent))
        if (genres.isNotEmpty() || layout.groups.isNotEmpty()) {
            add(HomeSection("genres", "Genres", "Browse by mood and category", columns = genres, groups = layout.groups))
        }
        if (trakt.isNotEmpty()) add(HomeSection("trakt", "Trakt", "Up next, watchlists and recommendations", trakt))
        if (layout.downloads.isNotEmpty()) add(HomeSection("downloads", "Downloads", "Your debrid library", layout.downloads))
        addAll(tiles)
        if (remaining.isNotEmpty()) add(HomeSection("more", "More to explore", "More collections from your add-ons", remaining))
    }
}

/** Whether [addon] shows on its own: not nested in an installed RVAH Discover. */
fun shownOnItsOwn(addon: AddonRecord, all: List<AddonRecord>): Boolean =
    addon.id !in DISCOVER_COMPONENTS || all.none { it.id == DISCOVER_ADDON && it.status == "enabled" }

/**
 * Lay out the home screen from each enabled catalog add-on's top level
 * ([roots], by add-on id; missing while loading). RVAH Discover first, then
 * add-ons with an account (yours), then the rest.
 */
fun homeLayout(addons: List<AddonRecord>, roots: Map<String, List<CatalogEntry>>): HomeLayout {
    val shown = addons
        .filter { it.status == "enabled" && "catalog" in it.capabilities && shownOnItsOwn(it, addons) }
        .sortedBy {
            when {
                it.id == DISCOVER_ADDON -> 0
                "auth" in it.capabilities -> 1
                else -> 2
            }
        }
    val discover = mutableListOf<ColumnSpec>()
    val groups = mutableListOf<GroupTab>()
    val downloads = mutableListOf<ColumnSpec>()
    for (addon in shown) {
        val root = roots[addon.id] ?: continue
        if (root.isNotEmpty() && root.all { it is CatalogEntry.Directory }) {
            for (folder in root.filterIsInstance<CatalogEntry.Directory>()) {
                if (folder.ofDirectories) {
                    groups += GroupTab(addon.id, folder.title, folder.path)
                } else {
                    discover += ColumnSpec(addon.id, folder.title, addon.name, folder.path)
                }
            }
        } else {
            downloads += ColumnSpec(addon.id, addon.name, null, null)
        }
    }
    // Name the add-on under a column only when columns come from several.
    val named = if (discover.map { it.addonId }.distinct().size > 1) discover else discover.map { it.copy(tag = null) }
    return HomeLayout(named, groups, downloads, shown.associate { it.id to it.name })
}

/** A card's small line: "2026 · ★ 8.4", leaving out what is unknown. */
fun cardLine(year: UShort?, rating: Float?): String =
    listOfNotNull(
        year?.toString(),
        rating?.takeIf { it > 0f }?.let { String.format(Locale.ROOT, "★ %.1f", it) },
    ).joinToString(" · ")

/** A list setting with [option] switched: off if on, else on and last. */
fun toggled(value: String, option: String): String {
    val chosen = value.split(',').map { it.trim() }.filter { it.isNotEmpty() }
    return (if (option in chosen) chosen - option else chosen + option).joinToString(",")
}
