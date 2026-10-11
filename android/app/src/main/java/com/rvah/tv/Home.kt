package com.rvah.tv

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Text
import kotlinx.coroutines.CoroutineScope
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry
import androidx.compose.material3.Text as FieldLabel

/** A tab of the home screen's top bar. */
internal sealed interface HomeTab {
    val label: String

    data object Discover : HomeTab {
        override val label = "Discover"
    }

    /** A folder of folders: its folders as columns. */
    data class Group(val group: GroupTab) : HomeTab {
        override val label get() = group.title
    }

    data object Search : HomeTab {
        override val label = "Search"
    }

    data object Downloads : HomeTab {
        override val label = "Downloads"
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
    /** A group tab's columns, by its path. */
    val groupColumns = mutableStateMapOf<String, List<ColumnSpec>>()
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
    onAddonSettings: (AddonRecord) -> Unit,
) {
    val catalogs = addons.filter { it.status == "enabled" && "catalog" in it.capabilities && shownOnItsOwn(it, addons) }
    // Each catalog add-on's top level decides its columns.
    LaunchedEffect(catalogs.map { it.id }, home.roots.size) {
        for (addon in catalogs) {
            if (addon.id in home.roots || addon.id in home.rootFailures) continue
            onIo { core.browse(addon.id, null, null) }
                .onSuccess { page ->
                    // A top level of titles is a column itself: keep what was loaded.
                    if (page.entries.none { it is CatalogEntry.Directory }) {
                        home.board("downloads").listings[ColumnSpec(addon.id, addon.name, null, null).key] =
                            Listing(page.entries, page.nextCursor)
                    }
                    home.roots[addon.id] = page.entries
                }
                .onFailure { home.rootFailures[addon.id] = "${addon.name}: ${it.userMessage()}" }
        }
    }
    val layout = homeLayout(addons, home.roots)
    val tabs = buildList {
        add(HomeTab.Discover)
        layout.groups.forEach { add(HomeTab.Group(it)) }
        add(HomeTab.Search)
        if (layout.downloads.isNotEmpty()) add(HomeTab.Downloads)
        add(HomeTab.Settings)
    }
    if (home.tab !in tabs) home.tab = HomeTab.Discover

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(start = 32.dp, end = 32.dp, top = 14.dp, bottom = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("RVAH", fontSize = 15.sp, color = Secondary, modifier = Modifier.padding(end = 12.dp))
            tabs.forEach { tab -> TabButton(tab.label, selected = tab == home.tab) { home.tab = tab } }
        }
        error?.let { Text(it, fontSize = 13.sp, color = ErrorText, modifier = Modifier.padding(horizontal = 32.dp)) }
        val content = Modifier.weight(1f).fillMaxWidth().padding(top = 4.dp)
        // Each tab its own scroll and focus state.
        key(home.tab) {
            when (val tab = home.tab) {
                HomeTab.Discover -> Column(content) {
                    home.rootFailures.values.forEach {
                        Text(it, fontSize = 13.sp, color = ErrorText, modifier = Modifier.padding(horizontal = 32.dp))
                    }
                    when {
                        layout.discover.isNotEmpty() ->
                            Board(core, layout.discover, home.board("discover"), scope, onOpen)
                        catalogs.isEmpty() -> Note("No add-ons to discover with yet: install one in Settings.")
                        else -> Note("Loading…")
                    }
                }
                is HomeTab.Group -> GroupBoard(core, home, tab.group, scope, onOpen, content)
                HomeTab.Search -> SearchTab(core, home, addons, scope, onOpen, content)
                HomeTab.Downloads -> Board(core, layout.downloads, home.board("downloads"), scope, onOpen, content)
                HomeTab.Settings -> SettingsTab(addons, onInstall, onDiagnostics, onAddonSettings, content)
            }
        }
    }
}

@Composable
private fun TabButton(label: String, selected: Boolean, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        contentPadding = PaddingValues(horizontal = 14.dp, vertical = 4.dp),
        colors = ButtonDefaults.colors(
            containerColor = if (selected) Color(0x33FFFFFF) else Color.Transparent,
            contentColor = if (selected) Primary else Secondary,
        ),
    ) { Text(label, fontSize = 15.sp) }
}

/** A folder of folders (say, genres) as a board of its folders. */
@Composable
private fun GroupBoard(
    core: AndroidCore,
    home: HomeState,
    group: GroupTab,
    scope: CoroutineScope,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    modifier: Modifier,
) {
    var failure by remember(group.path) { mutableStateOf<String?>(null) }
    LaunchedEffect(group.path) {
        if (group.path in home.groupColumns) return@LaunchedEffect
        onIo { core.browse(group.addonId, group.path, null) }
            .onSuccess { page ->
                home.groupColumns[group.path] = page.entries.filterIsInstance<CatalogEntry.Directory>()
                    .map { ColumnSpec(group.addonId, it.title, null, it.path) }
            }
            .onFailure { failure = it.userMessage() }
    }
    val columns = home.groupColumns[group.path]
    Column(modifier) {
        when {
            columns != null -> Board(core, columns, home.board("group:" + group.path), scope, onOpen)
            failure != null -> Note(failure.orEmpty(), ErrorText)
            else -> Note("Loading…")
        }
    }
}

/** One query, a column of results for each add-on that searches. */
@Composable
private fun SearchTab(
    core: AndroidCore,
    home: HomeState,
    addons: List<AddonRecord>,
    scope: CoroutineScope,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    modifier: Modifier,
) {
    val searchable = addons.filter { it.status == "enabled" && "search" in it.capabilities && shownOnItsOwn(it, addons) }
    val searched = home.searched
    Column(modifier) {
        Row(
            Modifier.padding(horizontal = 32.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val field = rememberInitialFocus(Unit, enabled = searched == null)
            OutlinedTextField(
                value = home.query,
                onValueChange = { home.query = it },
                label = { FieldLabel("Search films, series and your downloads") },
                singleLine = true,
                modifier = Modifier.focusRequester(field),
            )
            Button(onClick = { home.query.trim().takeIf { it.isNotEmpty() }?.let { home.searched = it } }) {
                Text("Search")
            }
        }
        if (searched != null) {
            val columns = searchable.map { ColumnSpec(it.id, it.name, null, null, query = searched) }
            key(searched) { Board(core, columns, home.board("search:$searched"), scope, onOpen) }
        } else if (searchable.isEmpty()) {
            Note("No installed add-on searches.")
        }
    }
}

@Composable
private fun SettingsTab(
    addons: List<AddonRecord>,
    onInstall: () -> Unit,
    onDiagnostics: () -> Unit,
    onAddonSettings: (AddonRecord) -> Unit,
    modifier: Modifier,
) {
    Column(modifier.padding(horizontal = 32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        val first = rememberInitialFocus(Unit)
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = onInstall, modifier = Modifier.focusRequester(first)) { Text("Install add-on") }
            Button(onClick = onDiagnostics) { Text("Diagnostics") }
        }
        Text("Add-ons", fontSize = 15.sp, color = Secondary)
        LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            items(addons, key = { it.id }) { addon ->
                val note = when {
                    addon.status != "enabled" -> " (${addon.status})"
                    !shownOnItsOwn(addon, addons) -> " (in RVAH Discover, not needed)"
                    else -> ""
                }
                Button(onClick = { onAddonSettings(addon) }) { Text(addon.name + note) }
            }
        }
    }
}

/** A line of small print, in line with the board. */
@Composable
private fun Note(text: String, color: Color = Secondary) {
    Text(text, fontSize = 14.sp, color = color, modifier = Modifier.padding(horizontal = 32.dp, vertical = 6.dp))
}
