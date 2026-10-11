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
)

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
    return HomeLayout(named, groups, downloads)
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
