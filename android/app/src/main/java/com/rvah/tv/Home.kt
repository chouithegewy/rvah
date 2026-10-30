package com.rvah.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.weight
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as FieldLabel
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.Text
import kotlinx.coroutines.CoroutineScope
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry

/** The compact home and the one destination currently open from it. */
internal sealed interface HomeTab {
    val label: String

    data object Discover : HomeTab {
        override val label = "Home"
    }

    data class Section(val section: HomeSection) : HomeTab {
        override val label get() = section.title
    }

    data object Search : HomeTab {
        override val label = "Search results"
    }

    data object Settings : HomeTab {
        override val label = "Settings"
    }
}

/** The home screen's state, kept while a title's pages are open so Back finds it as it was. */
internal class HomeState {
    /** Each catalog add-on's top level, by add-on id. */
    val roots = mutableStateMapOf<String, List<CatalogEntry>>()
    val rootFailures = mutableStateMapOf<String, String>()
    /** A genre/group's poster shelves, by add-on and path. */
    val groupColumns = mutableStateMapOf<String, List<ColumnSpec>>()
    val groupFailures = mutableStateMapOf<String, String>()
    private val boards = mutableMapOf<String, BoardState>()
    var tab by mutableStateOf<HomeTab>(HomeTab.Discover)
    var query by mutableStateOf("")
    var searched by mutableStateOf<String?>(null)

    fun board(key: String): BoardState = boards.getOrPut(key) { BoardState() }

    /** After an add-on's settings or sign-in changed: list it afresh. */
    fun forget(addonId: String) {
        roots.remove(addonId)
        rootFailures.remove(addonId)
        boards.clear()
        groupColumns.clear()
        groupFailures.clear()
    }
}

@Composable
internal fun HomeScreen(
    core: AndroidCore,
    home: HomeState,
    addons: List<AddonRecord>,
    error: String?,
    scope: CoroutineScope,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    onInstall: () -> Unit,
    onDiagnostics: () -> Unit,
    onPortableProfile: () -> Unit,
    onAddonSettings: (AddonRecord) -> Unit,
    onTrailer: (String, String) -> Unit,
) {
    val catalogs = addons.filter { it.status == "enabled" && "catalog" in it.capabilities && shownOnItsOwn(it, addons) }
    val detailsAddons = addons.filter { it.status == "enabled" && "details" in it.capabilities }.mapTo(mutableSetOf()) { it.id }
    // Only roots load on Home. Poster shelves wait until their tile opens.
    LaunchedEffect(catalogs.map { it.id }, home.roots.size) {
        for (addon in catalogs) {
            if (addon.id in home.roots || addon.id in home.rootFailures) continue
            onIo { core.browse(addon.id, null, null) }
                .onSuccess { page ->
                    // A top level of titles is a library shelf; retain the page already fetched.
                    if (page.entries.none { it is CatalogEntry.Directory }) {
                        val spec = ColumnSpec(addon.id, addon.name, null, null)
                        home.board("downloads").listings[spec.key] = Listing(page.entries, page.nextCursor)
                    }
                    home.roots[addon.id] = page.entries
                }
                .onFailure { home.rootFailures[addon.id] = "${addon.name}: ${it.userMessage()}" }
        }
    }
    val layout = homeLayout(addons, home.roots)
    val sections = homeSections(layout)

    if (home.tab != HomeTab.Discover) BackHandler { home.tab = HomeTab.Discover }
    Column(Modifier.fillMaxSize()) {
        HomeHeader(
            tab = home.tab,
            query = home.query,
            onQueryChange = { home.query = it },
            onSearch = {
                home.query.trim().takeIf { it.isNotEmpty() }?.let {
                    home.searched = it
                    home.tab = HomeTab.Search
                }
            },
            onBack = { home.tab = HomeTab.Discover },
            onSettings = { home.tab = HomeTab.Settings },
        )
        error?.let { Text(it, fontSize = 13.sp, color = ErrorText, modifier = Modifier.padding(horizontal = 32.dp)) }
        val content = Modifier.weight(1f).fillMaxWidth()
        key(home.tab) {
            when (val tab = home.tab) {
                HomeTab.Discover -> Column(content) {
                    home.rootFailures.values.forEach {
                        Text(it, fontSize = 13.sp, color = ErrorText, modifier = Modifier.padding(horizontal = 32.dp))
                    }
                    when {
                        sections.isNotEmpty() -> HomeTiles(sections) { home.tab = HomeTab.Section(it) }
                        catalogs.isEmpty() -> Note("No discovery add-ons yet. Install one in Settings.")
                        else -> Note("Loading collections…")
                    }
                }
                is HomeTab.Section -> SectionBoard(
                    core = core,
                    home = home,
                    section = tab.section,
                    scope = scope,
                    detailsAddons = detailsAddons,
                    onOpen = onOpen,
                    onTrailer = onTrailer,
                    modifier = content,
                )
                HomeTab.Search -> SearchResults(
                    core = core,
                    home = home,
                    addons = addons,
                    scope = scope,
                    detailsAddons = detailsAddons,
                    onOpen = onOpen,
                    onTrailer = onTrailer,
                    modifier = content,
                )
                HomeTab.Settings -> SettingsTab(
                    addons,
                    onInstall,
                    onDiagnostics,
                    onPortableProfile,
                    onAddonSettings,
                    content,
                )
            }
        }
    }
}

@Composable
private fun HomeHeader(
    tab: HomeTab,
    query: String,
    onQueryChange: (String) -> Unit,
    onSearch: () -> Unit,
    onBack: () -> Unit,
    onSettings: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().padding(start = 32.dp, end = 32.dp, top = 14.dp, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Image(
            painterResource(R.drawable.rvah_wordmark),
            contentDescription = "RVAH",
            modifier = Modifier.height(24.dp).padding(end = 8.dp),
        )
        if (tab == HomeTab.Discover) {
            OutlinedTextField(
                value = query,
                onValueChange = onQueryChange,
                label = { FieldLabel("Search films, series and downloads") },
                singleLine = true,
                modifier = Modifier.weight(1f),
            )
            Button(onClick = onSearch) { Text("Search") }
            Button(onClick = onSettings) { Text("Settings") }
        } else {
            Button(onClick = onBack) { Text("Back") }
            Text(tab.label, color = Primary, fontSize = 23.sp, modifier = Modifier.weight(1f))
            if (tab != HomeTab.Settings) Button(onClick = onSettings) { Text("Settings") }
        }
    }
}

@Composable
private fun HomeTiles(sections: List<HomeSection>, onOpen: (HomeSection) -> Unit) {
    val first = rememberInitialFocus(sections.map { it.id })
    LazyVerticalGrid(
        columns = GridCells.Fixed(2),
        contentPadding = PaddingValues(horizontal = 48.dp, vertical = 18.dp),
        horizontalArrangement = Arrangement.spacedBy(22.dp),
        verticalArrangement = Arrangement.spacedBy(22.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        itemsIndexed(sections, key = { _, section -> section.id }) { index, section ->
            val shade = listOf(Color(0xFF28364A), Color(0xFF3A2F4C), Color(0xFF253F3A), Color(0xFF44332D))[index % 4]
            Card(
                onClick = { onOpen(section) },
                modifier = (if (index == 0) Modifier.focusRequester(first) else Modifier)
                    .height(150.dp)
                    .onPreviewKeyEvent { event ->
                        if (index % 2 == 1 && event.type == KeyEventType.KeyDown && event.key == Key.DirectionRight) {
                            onOpen(section)
                            true
                        } else {
                            false
                        }
                    },
                scale = CardDefaults.scale(focusedScale = 1.035f),
            ) {
                Column(
                    Modifier.fillMaxSize().background(shade, RoundedCornerShape(10.dp)).padding(22.dp),
                    verticalArrangement = Arrangement.Bottom,
                ) {
                    Text(section.title, color = Primary, fontSize = 24.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(section.subtitle, color = Secondary, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

private fun groupKey(group: GroupTab) = group.addonId + "\u0000" + group.path

/** Load genre folders only after Genres opens, then show their posters in the common grid. */
@Composable
private fun SectionBoard(
    core: AndroidCore,
    home: HomeState,
    section: HomeSection,
    scope: CoroutineScope,
    detailsAddons: Set<String>,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    onTrailer: (String, String) -> Unit,
    modifier: Modifier,
) {
    section.groups.forEach { group ->
        val key = groupKey(group)
        LaunchedEffect(key) {
            if (key in home.groupColumns || key in home.groupFailures) return@LaunchedEffect
            onIo { core.browse(group.addonId, group.path, null) }
                .onSuccess { page ->
                    home.groupColumns[key] = page.entries.filterIsInstance<CatalogEntry.Directory>()
                        .map { ColumnSpec(group.addonId, it.title, group.title, it.path) }
                }
                .onFailure { home.groupFailures[key] = it.userMessage() }
        }
    }
    val groupColumns = section.groups.flatMap { home.groupColumns[groupKey(it)].orEmpty() }
    val columns = section.columns + groupColumns
    Column(modifier) {
        when {
            columns.isNotEmpty() -> Board(
                core,
                columns,
                home.board(if (section.id == "downloads") "downloads" else "section:${section.id}"),
                scope,
                onOpen,
                detailsAddons = detailsAddons,
                onTrailer = onTrailer,
            )
            section.groups.any { groupKey(it) !in home.groupFailures } -> Note("Loading ${section.title.lowercase()}…")
            else -> Note(section.groups.joinToString("\n") { home.groupFailures[groupKey(it)].orEmpty() }, ErrorText)
        }
    }
}

/** One search, merged into the same poster grid instead of provider columns. */
@Composable
private fun SearchResults(
    core: AndroidCore,
    home: HomeState,
    addons: List<AddonRecord>,
    scope: CoroutineScope,
    detailsAddons: Set<String>,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    onTrailer: (String, String) -> Unit,
    modifier: Modifier,
) {
    val searchable = addons.filter { it.status == "enabled" && "search" in it.capabilities && shownOnItsOwn(it, addons) }
    val searched = home.searched
    Column(modifier) {
        when {
            searched == null -> Note("Enter a search from Home.")
            searchable.isEmpty() -> Note("No installed add-on searches.")
            else -> {
                val columns = searchable.map { ColumnSpec(it.id, it.name, it.name, null, query = searched) }
                key(searched) {
                    Board(
                        core,
                        columns,
                        home.board("search:$searched"),
                        scope,
                        onOpen,
                        detailsAddons = detailsAddons,
                        onTrailer = onTrailer,
                    )
                }
            }
        }
    }
}

@Composable
private fun SettingsTab(
    addons: List<AddonRecord>,
    onInstall: () -> Unit,
    onDiagnostics: () -> Unit,
    onPortableProfile: () -> Unit,
    onAddonSettings: (AddonRecord) -> Unit,
    modifier: Modifier,
) {
    Column(modifier.padding(horizontal = 32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        val first = rememberInitialFocus(Unit)
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = onInstall, modifier = Modifier.focusRequester(first)) { Text("Install add-on") }
            Button(onClick = onPortableProfile) { Text("Portable profile") }
            Button(onClick = onDiagnostics) { Text("Diagnostics") }
        }
        Text("Add-ons", fontSize = 15.sp, color = Secondary)
        LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            items(addons, key = { it.id }) { addon ->
                val note = when {
                    addon.status != "enabled" -> " (${addon.status})"
                    !shownOnItsOwn(addon, addons) -> " (included in RVAH Discover)"
                    else -> ""
                }
                Button(onClick = { onAddonSettings(addon) }) { Text(addon.name + note) }
            }
        }
    }
}

/** A line of small print, in line with the poster grid. */
@Composable
private fun Note(text: String, color: Color = Secondary) {
    Text(text, fontSize = 14.sp, color = color, modifier = Modifier.padding(horizontal = 32.dp, vertical = 8.dp))
}
