package com.rvah.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as FieldLabel
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.ui.compose.PlayerSurface
import androidx.tv.material3.Button
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import androidx.tv.material3.darkColorScheme
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.rvah_android.AccountState
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.AndroidCoreException
import uniffi.rvah_android.NativePreparedPlayback
import uniffi.rvah_android.ScrobbleTarget
import uniffi.rvah_android.SourceQuery
import uniffi.rvah_android.CatalogEntry
import uniffi.rvah_android.DiagnosticRecord
import uniffi.rvah_android.InstallPreview
import uniffi.rvah_android.SecretStatus
import uniffi.rvah_android.SettingRecord
import uniffi.rvah_android.SignInPrompt
import uniffi.rvah_android.SignInState

internal val Background = Color(0xFF0B0F14)
internal val Primary = Color.White
internal val Secondary = Color.LightGray
internal val ErrorText = Color(0xFFFF8A80)

/** Navigation state. Browse keeps its directory stack so Back from the player returns to it. */
private sealed interface Screen {
    data object Home : Screen
    /** [returnTo]: where Back from the first level goes (Home when null). */
    data class Browse(
        val addonId: String,
        val addonName: String,
        val stack: List<String?> = listOf(null),
        val returnTo: Screen? = null,
    ) : Screen
    data object Search : Screen
    /** A catalog item, or with [release] a release of it through a debrid add-on. */
    data class Player(
        val addonId: String,
        val itemId: String,
        val title: String,
        val returnTo: Screen,
        val release: ReleasePick? = null,
    ) : Screen
    data class Details(val addonId: String, val itemId: String, val title: String, val returnTo: Screen) : Screen
    data class Sources(val returnTo: Screen) : Screen
    data object Settings : Screen
    data class AddonSettings(val addon: AddonRecord) : Screen
    data class LinkAccount(val addon: AddonRecord, val returnTo: Screen = AddonSettings(addon)) : Screen
    data object Install : Screen
    data object Diagnostics : Screen
}

/** The screen Back eventually leads to: the player, details and Link account return to where they were opened. */
private val Screen.base: Screen get() = when (this) {
    is Screen.Player -> returnTo.base
    is Screen.Details -> returnTo.base
    is Screen.Sources -> returnTo.base
    is Screen.LinkAccount -> returnTo.base
    else -> this
}

internal data class BrowseKey(val addonId: String, val path: String?)

/** Submitted search: Load more repeats this query, not whatever is typed now. */
private data class SearchResults(
    val generation: Int,
    val addonId: String,
    val query: String,
    val listing: Listing<CatalogEntry.Media>,
)

/** A failed submission, kept with its add-on and query so signing in can retry it. */
private class SearchFailure(val addon: AddonRecord, val query: String, val error: Throwable)

/** Search outlives a trip to the player or Link account; leaving it for another screen starts over. */
private class SearchModel {
    var addonId by mutableStateOf<String?>(null)
    var query by mutableStateOf("")
    var results by mutableStateOf<SearchResults?>(null)
    var searching by mutableStateOf(false)
    var failure by mutableStateOf<SearchFailure?>(null)
    /** Submitted again when Search is next shown: the query that needed a sign-in. */
    var retryQuery: String? = null
    /** Bumped per submission so a slow earlier search cannot overwrite a newer one. */
    var generation = 0

    /** Apply [change] only while submission [generation]'s results are the ones shown. */
    fun updateResults(generation: Int, change: (Listing<CatalogEntry.Media>) -> Listing<CatalogEntry.Media>) {
        results?.takeIf { it.generation == generation }?.let { results = it.copy(listing = change(it.listing)) }
    }
}

@Composable
fun RvahTvApp(
    openCore: () -> AndroidCore,
    owner: RvahPlayerOwner,
    packagesDir: File?,
    trustedKeys: List<String>,
) {
    ArtworkLoader()
    RvahTheme {
        Box(Modifier.fillMaxSize().background(Background)) {
            var core by remember { mutableStateOf<AndroidCore?>(null) }
            var startupError by remember { mutableStateOf<String?>(null) }
            LaunchedEffect(Unit) {
                onIo { openCore() }
                    .onSuccess {
                        owner.core = it
                        core = it
                    }
                    .onFailure { startupError = it.userMessage() }
            }
            val ready = core
            val failed = startupError
            when {
                ready != null -> AppNavigation(ready, owner, packagesDir, trustedKeys)
                failed != null -> Page("RVAH could not start") { Body(failed, ErrorText) }
                else -> Page("RVAH") { Body("Starting…") }
            }
        }
    }
}

/**
 * Buttons and labels follow tv.material3's theme, the text fields
 * compose.material3's. Both are dark, so focus brightens a button.
 */
@Composable
private fun RvahTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = darkColorScheme()) {
        androidx.compose.material3.MaterialTheme(
            colorScheme = androidx.compose.material3.darkColorScheme(),
            content = content,
        )
    }
}

@Composable
private fun AppNavigation(
    core: AndroidCore,
    owner: RvahPlayerOwner,
    packagesDir: File?,
    trustedKeys: List<String>,
) {
    var screen by remember { mutableStateOf<Screen>(Screen.Home) }
    var addons by remember { mutableStateOf<List<AddonRecord>>(emptyList()) }
    var addonsError by remember { mutableStateOf<String?>(null) }
    var refresh by remember { mutableStateOf(0) }
    // Loaded pages live here rather than in their screens, so Back from the
    // player or a subdirectory shows them again with the opened row focused.
    val browsed = remember { mutableStateMapOf<BrowseKey, Listing<CatalogEntry>>() }
    var search by remember { mutableStateOf(SearchModel()) }
    // Load-more requests outlive their screen so none is left stuck "Loading…".
    val scope = rememberCoroutineScope()
    LaunchedEffect(screen) {
        // Keep only what Back can still reach from here.
        val base = screen.base
        val levels = if (base is Screen.Browse) base.stack.map { BrowseKey(base.addonId, it) } else emptyList()
        browsed.keys.filterNot { it in levels }.forEach { browsed.remove(it) }
        if (base != Screen.Search) search = SearchModel()
    }
    LaunchedEffect(refresh) {
        onIo {
            seedDevSecrets(core)
            core.addons()
        }
            .onSuccess {
                addons = it
                addonsError = null
            }
            .onFailure { addonsError = it.userMessage() }
    }
    // Discover rows per add-on, kept until Home so Back finds the same poster.
    val discover = remember { mutableMapOf<String, DiscoverState>() }
    // Where Back leaves Search for, when a details page opened it.
    var searchBack by remember { mutableStateOf<Screen?>(null) }
    // The last source search, kept while its releases play.
    var sources by remember { mutableStateOf<SourcesState?>(null) }
    val goHome = {
        refresh++
        discover.clear()
        searchBack = null
        sources = null
        screen = Screen.Home
    }
    val enabledWith: (String) -> AddonRecord? = { capability ->
        addons.firstOrNull { it.status == "enabled" && capability in it.capabilities }
    }
    val hasDetails: (String) -> Boolean = { id -> addons.any { it.id == id && "details" in it.capabilities } }
    // A title opens its details page when its add-on has them; else it plays.
    val open: (String, String, String, Screen) -> Unit = { addonId, itemId, title, from ->
        screen = if (hasDetails(addonId)) {
            Screen.Details(addonId, itemId, title, from)
        } else {
            Screen.Player(addonId, itemId, title, from)
        }
    }
    // An item of an add-on without details pages (Trakt, say) opens on the
    // TMDB add-on's, when it names a TMDB id; else it opens as below.
    val openEntry: (String, CatalogEntry.Media, Screen) -> Unit = { addonId, entry, from ->
        val tmdb = entry.tmdbId
        val described = !hasDetails(addonId) && tmdb != null && hasDetails(TMDB_ADDON) &&
            (entry.kind == "movie" || entry.kind == "series")
        if (described) {
            val id = if (entry.kind == "movie") "movie:$tmdb" else "tv:$tmdb"
            screen = Screen.Details(TMDB_ADDON, id, entry.title, from)
        } else {
            open(addonId, entry.id, entry.title, from)
        }
    }
    val searchable = addons.filter { it.status == "enabled" && "search" in it.capabilities }
    // Where an add-on's sign-in error leads: device sign-in when it has one.
    val signInFor: (AddonRecord, Screen) -> Pair<String, () -> Unit> = { addon, back ->
        if ("auth" in addon.capabilities) {
            "Link account" to { screen = Screen.LinkAccount(addon, returnTo = back) }
        } else {
            "Add-on settings" to { screen = Screen.AddonSettings(addon) }
        }
    }

    when (val current = screen) {
        Screen.Home -> HomeScreen(
            addons = addons,
            error = addonsError,
            canSearch = searchable.isNotEmpty(),
            onOpen = { addon ->
                screen = if (addon.status == "enabled") {
                    Screen.Browse(addon.id, addon.name)
                } else {
                    Screen.AddonSettings(addon)
                }
            },
            onNavigate = { screen = it },
        )
        is Screen.Browse -> BrowseScreen(
            core = core,
            browse = current,
            listings = browsed,
            scope = scope,
            signIn = addons.firstOrNull { it.id == current.addonId }?.let { signInFor(it, current) },
            discover = discover.getOrPut(current.addonId) { DiscoverState(current.addonId) },
            onNavigate = { screen = it },
            onPlay = { entry -> openEntry(current.addonId, entry, current) },
            onExit = { current.returnTo?.let { screen = it } ?: goHome() },
        )
        is Screen.Details -> DetailsScreen(
            core = core,
            addonId = current.addonId,
            itemId = current.itemId,
            fallbackTitle = current.title,
            // Add-ons that can find and play the title.
            players = addons.filter {
                it.id != current.addonId && it.status == "enabled" &&
                    "search" in it.capabilities && "resolve" in it.capabilities
            },
            onFind = { finder ->
                search = SearchModel().apply {
                    addonId = finder.addon.id
                    query = finder.query
                    retryQuery = finder.query
                }
                searchBack = current
                screen = Screen.Search
            },
            // A film or episode: find releases, when an add-on can find them
            // and another can play them.
            onFindSources = if (enabledWith("sources") != null && enabledWith("debrid") != null) {
                { details ->
                    val query = SourceQuery(
                        imdbId = details.imdbId,
                        title = details.seriesTitle ?: details.title,
                        year = details.year.takeIf { details.kind == "movie" },
                        season = details.season,
                        episode = details.episode,
                    )
                    val heading = sourcesTitle(details.title, details.seriesTitle, details.season, details.episode)
                    val scrobble = details.tmdbId?.let {
                        ScrobbleTarget(tmdbId = it, imdbId = details.imdbId, season = details.season, episode = details.episode)
                    }
                    sources = SourcesState(SourcesTarget(current.addonId, details.id, heading, query, scrobble))
                    screen = Screen.Sources(returnTo = current)
                }
            } else {
                null
            },
            // A series: its seasons and episodes, as rows.
            onEpisodes = { details ->
                screen = Screen.Browse(current.addonId, details.title, stack = listOf(details.id), returnTo = current)
            },
            onExit = { screen = current.returnTo },
        )
        is Screen.Sources -> {
            val state = sources
            val source = enabledWith("sources")
            val debrid = enabledWith("debrid")
            if (state == null || source == null || debrid == null) {
                LaunchedEffect(Unit) { screen = current.returnTo }
            } else {
                SourcesScreen(
                    core = core,
                    state = state,
                    source = source,
                    onPlay = { release ->
                        screen = Screen.Player(
                            addonId = state.target.titleAddonId,
                            itemId = state.target.titleItemId,
                            title = state.target.heading,
                            returnTo = current,
                            release = ReleasePick(debrid.id, release, state.target),
                        )
                    },
                    onExit = { screen = current.returnTo },
                )
            }
        }
        Screen.Search -> SearchScreen(
            core = core,
            addons = searchable,
            model = search,
            scope = scope,
            signIn = { addon -> signInFor(addon, Screen.Search) },
            onPlay = { addonId, itemId, title -> open(addonId, itemId, title, Screen.Search) },
            onExit = {
                val back = searchBack
                searchBack = null
                if (back != null) screen = back else goHome()
            },
        )
        is Screen.Player -> PlayerScreen(
            owner = owner,
            target = current,
            prepare = {
                val pick = current.release
                if (pick == null) {
                    core.preparePlayback(current.addonId, current.itemId, current.title)
                } else {
                    core.prepareRelease(
                        pick.debridId,
                        pick.release,
                        pick.target.query.season,
                        pick.target.query.episode,
                        pick.target.titleAddonId,
                        pick.target.titleItemId,
                        current.title,
                    )
                }
            },
            // A release that cannot play (not cached, say) goes back to the
            // list, marked, so another can be picked.
            onPrepareFailed = current.release?.let { pick ->
                { message: String ->
                    sources?.tried?.set(pick.release.infoHash, message)
                    screen = current.returnTo
                }
            },
            onExit = {
                owner.stop()
                screen = current.returnTo
            },
        )
        Screen.Settings -> Page("Settings") {
            BackHandler(onBack = goHome)
            if (addons.isEmpty()) Body("No add-ons installed.")
            val first = rememberInitialFocus(addons)
            LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                items(addons, key = { it.id }) { addon ->
                    val modifier = if (addon == addons.first()) Modifier.focusRequester(first) else Modifier
                    Button(onClick = { screen = Screen.AddonSettings(addon) }, modifier = modifier) {
                        Text("${addon.name} — ${addon.status}")
                    }
                }
            }
        }
        is Screen.AddonSettings -> AddonSettingsScreen(
            core = core,
            addon = current.addon,
            onChanged = { refresh++ },
            onLinkAccount = { screen = Screen.LinkAccount(current.addon) },
            onExit = { screen = Screen.Settings },
        )
        is Screen.LinkAccount -> LinkAccountScreen(
            core = core,
            addon = current.addon,
            scope = scope,
            onExit = {
                refresh++
                screen = current.returnTo
            },
        )
        Screen.Install -> InstallScreen(
            core = core,
            packagesDir = packagesDir,
            trustedKeys = trustedKeys,
            onInstalled = { refresh++ },
            onExit = goHome,
        )
        Screen.Diagnostics -> DiagnosticsScreen(core, onExit = goHome)
    }
}

@Composable
private fun HomeScreen(
    addons: List<AddonRecord>,
    error: String?,
    canSearch: Boolean,
    onOpen: (AddonRecord) -> Unit,
    onNavigate: (Screen) -> Unit,
) {
    Page("RVAH") {
        Body("Your add-ons", Secondary)
        val first = rememberInitialFocus(addons.size)
        if (addons.isEmpty()) {
            Body("No add-ons installed yet. Use Install add-on to add a signed package.")
        } else {
            Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                addons.forEachIndexed { index, addon ->
                    val label = if (addon.status == "enabled") addon.name else "${addon.name} (${addon.status})"
                    val modifier = if (index == 0) Modifier.focusRequester(first) else Modifier
                    Button(onClick = { onOpen(addon) }, modifier = modifier) { Text(label) }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            if (canSearch) Button(onClick = { onNavigate(Screen.Search) }) { Text("Search") }
            Button(onClick = { onNavigate(Screen.Settings) }) { Text("Settings") }
            val installModifier = if (addons.isEmpty()) Modifier.focusRequester(first) else Modifier
            Button(onClick = { onNavigate(Screen.Install) }, modifier = installModifier) { Text("Install add-on") }
            Button(onClick = { onNavigate(Screen.Diagnostics) }) { Text("Diagnostics") }
        }
        error?.let { Body(it, ErrorText) }
    }
}

@Composable
private fun BrowseScreen(
    core: AndroidCore,
    browse: Screen.Browse,
    listings: SnapshotStateMap<BrowseKey, Listing<CatalogEntry>>,
    scope: CoroutineScope,
    /** Label and action offered when the add-on needs an account. */
    signIn: Pair<String, () -> Unit>?,
    /** Rows state: a first level of only folders shows as poster rows. */
    discover: DiscoverState?,
    onNavigate: (Screen) -> Unit,
    onPlay: (CatalogEntry.Media) -> Unit,
    onExit: () -> Unit,
) {
    val level = BrowseKey(browse.addonId, browse.stack.last())
    var failure by remember(level) { mutableStateOf<Throwable?>(null) }
    LaunchedEffect(level) {
        if (level in listings) return@LaunchedEffect
        onIo { core.browse(level.addonId, level.path, null) }
            .onSuccess { listings[level] = Listing(it.entries, it.nextCursor) }
            .onFailure { failure = it }
    }
    BackHandler {
        if (browse.stack.size > 1) onNavigate(browse.copy(stack = browse.stack.dropLast(1))) else onExit()
    }
    // Only while the level is still loaded: a late page for a level Back
    // already dropped must not bring it back.
    val update: ((Listing<CatalogEntry>) -> Listing<CatalogEntry>) -> Unit = { change ->
        listings[level]?.let { listings[level] = change(it) }
    }
    Page(browse.addonName) {
        val loaded = listings[level]
        val failed = failure
        when {
            failed != null -> {
                Body(failed.userMessage(), ErrorText)
                if (failed is AndroidCoreException.SignInRequired && signIn != null) {
                    val action = rememberInitialFocus(Unit)
                    Button(onClick = signIn.second, modifier = Modifier.focusRequester(action)) { Text(signIn.first) }
                }
            }
            loaded == null -> Body("Loading…")
            loaded.entries.isEmpty() && !loaded.hasMore -> Body("Nothing here.")
            discover != null && browse.stack.size == 1 && loaded.entries.all { it is CatalogEntry.Directory } ->
                DiscoverRows(
                    core = core,
                    rows = loaded.entries.filterIsInstance<CatalogEntry.Directory>(),
                    state = discover,
                    scope = scope,
                    onOpen = { entry ->
                        when (entry) {
                            is CatalogEntry.Directory -> onNavigate(browse.copy(stack = browse.stack + entry.path))
                            is CatalogEntry.Media -> onPlay(entry)
                        }
                    },
                )
            // Each level gets its own scroll and focus state.
            else -> key(level) {
                ListingView(
                    listing = loaded,
                    onOpen = { index, entry ->
                        update { it.opened(index) }
                        when (entry) {
                            is CatalogEntry.Directory -> onNavigate(browse.copy(stack = browse.stack + entry.path))
                            is CatalogEntry.Media -> onPlay(entry)
                        }
                    },
                    onLoadMore = {
                        scope.loadMore(update) { cursor ->
                            core.browse(level.addonId, level.path, cursor).let { it.entries to it.nextCursor }
                        }
                    },
                )
            }
        }
    }
}

/**
 * The rows loaded so far, then a "Load more" row while the add-on has more
 * pages (FR-024). Focus goes to [Listing.focusIndex] when shown and whenever
 * a page lands, so the D-pad continues into the new rows.
 */
@Composable
private fun <T : CatalogEntry> ListingView(
    listing: Listing<T>,
    onOpen: (index: Int, entry: T) -> Unit,
    onLoadMore: () -> Unit,
) {
    val focus = rememberInitialFocus(listing.focusIndex to listing.entries.size)
    fun focusAt(index: Int) = if (index == listing.focusIndex) Modifier.focusRequester(focus) else Modifier
    LazyColumn(
        state = rememberLazyListState(initialFirstVisibleItemIndex = listing.focusIndex),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        items(listing.entries.size) { index ->
            val entry = listing.entries[index]
            Button(onClick = { onOpen(index, entry) }, modifier = focusAt(index)) { Text(entryLabel(entry)) }
        }
        if (listing.hasMore) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    listing.moreError?.let { Body(it, ErrorText) }
                    Button(onClick = onLoadMore, modifier = focusAt(listing.entries.size)) {
                        Text(
                            when {
                                listing.loadingMore -> "Loading…"
                                listing.moreError != null -> "Try again"
                                else -> "Load more"
                            },
                        )
                    }
                }
            }
        }
    }
}

private fun entryLabel(entry: CatalogEntry): String = when (entry) {
    is CatalogEntry.Directory -> "▸ ${entry.title}"
    is CatalogEntry.Media -> listOfNotNull(entry.title, entry.year?.toString()).joinToString("  ·  ")
}

/**
 * Fetch the page after the listing [update] reaches, in this app-wide scope so
 * leaving the screen mid-request cannot strand it "Loading…". Each transition
 * applies to whatever the listing is by then; [update] skips a listing that
 * is gone, and [Listing.appended] skips one reloaded meanwhile.
 */
internal fun <T> CoroutineScope.loadMore(
    update: ((Listing<T>) -> Listing<T>) -> Unit,
    fetch: (cursor: String) -> Pair<List<T>, String?>,
) {
    var cursor: String? = null
    update { current -> current.cursorToLoad()?.let { cursor = it; current.startLoadingMore() } ?: current }
    val requested = cursor ?: return
    launch {
        onIo { fetch(requested) }
            .onSuccess { (page, next) -> update { it.appended(requested, page, next) } }
            .onFailure { error -> update { it.failedMore(requested, error.userMessage()) } }
    }
}

@Composable
private fun SearchScreen(
    core: AndroidCore,
    addons: List<AddonRecord>,
    model: SearchModel,
    scope: CoroutineScope,
    /** Label and action offered when [AddonRecord] needs an account. */
    signIn: (AddonRecord) -> Pair<String, () -> Unit>,
    onPlay: (addonId: String, itemId: String, title: String) -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    Page("Search") {
        val addon = addons.firstOrNull { it.id == model.addonId } ?: addons.firstOrNull()
        if (addon == null) {
            Body("No installed add-on supports search.")
            return@Page
        }
        if (addons.size > 1) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                addons.forEach { candidate ->
                    Button(onClick = { model.addonId = candidate.id }) {
                        Text(if (candidate == addon) "• ${candidate.name}" else candidate.name)
                    }
                }
            }
        }
        val results = model.results
        // Returning from the player focuses the opened result instead.
        val field = rememberInitialFocus(Unit, enabled = results == null)
        OutlinedTextField(
            value = model.query,
            onValueChange = { model.query = it },
            label = { FieldLabel("Search ${addon.name}") },
            singleLine = true,
            modifier = Modifier.focusRequester(field),
        )
        fun submit(query: String) {
            val generation = ++model.generation
            model.searching = true
            model.failure = null
            scope.launch {
                val outcome = onIo { core.search(addon.id, query, null) }
                if (generation != model.generation) return@launch
                model.searching = false
                outcome
                    .onSuccess { page ->
                        val media = page.entries.filterIsInstance<CatalogEntry.Media>()
                        model.results = SearchResults(generation, addon.id, query, Listing(media, page.nextCursor))
                    }
                    .onFailure { model.failure = SearchFailure(addon, query, it) }
            }
        }
        // Back from Link account: run the search that needed it.
        LaunchedEffect(Unit) {
            model.retryQuery?.let {
                model.retryQuery = null
                submit(it)
            }
        }
        Button(onClick = {
            val query = model.query.trim()
            if (query.isNotEmpty()) submit(query)
        }) { Text("Search") }
        val failure = model.failure
        when {
            model.searching -> Body("Searching…")
            failure != null -> {
                Body(failure.error.userMessage(), ErrorText)
                if (failure.error is AndroidCoreException.SignInRequired) {
                    val (label, go) = signIn(failure.addon)
                    val action = rememberInitialFocus(failure)
                    Button(onClick = {
                        model.addonId = failure.addon.id
                        model.retryQuery = failure.query
                        go()
                    }, modifier = Modifier.focusRequester(action)) { Text(label) }
                }
            }
            results != null && results.listing.entries.isEmpty() && !results.listing.hasMore -> Body("No results.")
        }
        // Each submission starts at the top with its first result focused.
        if (results != null) key(results.generation) {
            ListingView(
                listing = results.listing,
                onOpen = { index, entry ->
                    model.updateResults(results.generation) { it.opened(index) }
                    onPlay(results.addonId, entry.id, entry.title)
                },
                onLoadMore = {
                    // Bound to this submission: a newer search drops its late pages.
                    scope.loadMore({ change -> model.updateResults(results.generation, change) }) { cursor ->
                        core.search(results.addonId, results.query, cursor)
                            .let { page -> page.entries.filterIsInstance<CatalogEntry.Media>() to page.nextCursor }
                    }
                },
            )
        }
    }
}

@Composable
private fun PlayerScreen(
    owner: RvahPlayerOwner,
    target: Screen.Player,
    /** Resolve what to play (on an IO thread). */
    prepare: () -> NativePreparedPlayback,
    /** When set, a failure to prepare goes here instead of the player's status. */
    onPrepareFailed: ((String) -> Unit)?,
    onExit: () -> Unit,
) {
    var started by remember(target) { mutableStateOf(false) }
    LaunchedEffect(target) {
        onIo { prepare() }.onSuccess {
            started = true
            owner.play(it.toSpec(), scrobble = target.release?.target?.scrobble)
        }.onFailure { error ->
            val message = error.userMessage()
            if (onPrepareFailed != null) onPrepareFailed(message) else owner.fail(message)
        }
    }
    val ui = owner.ui
    // The controls hide after a quiet spell while playing, or on Back; any
    // key brings them back. `touched` restarts the countdown.
    var controls by remember(target) { mutableStateOf(true) }
    var touched by remember(target) { mutableStateOf(0) }
    val autoHide = controlsAutoHide(started, ui.isPlaying, ui.failure != null)
    LaunchedEffect(controls, touched, autoHide) {
        if (controls && autoHide) {
            delay(CONTROLS_HIDE_AFTER_MS)
            controls = false
        }
    }
    BackHandler { if (controls) controls = false else onExit() }
    val primary = rememberInitialFocus(controls, enabled = controls)
    // With the controls gone, the screen itself holds focus for key events.
    val screen = rememberInitialFocus(controls, enabled = !controls)
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .focusRequester(screen)
            .focusable()
            .onPreviewKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                touched++
                if (controls || event.key == Key.Back) return@onPreviewKeyEvent false
                controls = true
                // The first D-pad or Select press only reveals the controls;
                // media keys also act (below).
                event.key !in MediaKeys
            }
            // Bubbling (not preview) handler: D-pad and Select reach the
            // focused button first; only media keys are handled here.
            .onKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onKeyEvent false
                when (event.key) {
                    Key.MediaPlayPause -> { owner.togglePlayPause(); true }
                    Key.MediaPlay -> { owner.player.play(); true }
                    Key.MediaPause -> { owner.player.pause(); true }
                    Key.MediaFastForward -> { owner.seekBy(30_000); true }
                    Key.MediaRewind -> { owner.seekBy(-10_000); true }
                    else -> false
                }
            },
    ) {
        PlayerSurface(player = owner.player, modifier = Modifier.fillMaxSize())
        if (controls) Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(40.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(target.title, color = Primary, fontSize = 22.sp)
            val failure = ui.failure
            val status = when {
                failure != null -> failure
                !started -> "Resolving video…"
                else -> ui.status
            }
            if (status.isNotEmpty()) Text(status, color = if (failure != null) ErrorText else Secondary)
            if (BuildConfig.DEBUG && started) Text(bufferLine(ui.bufferedAheadMs, owner.bufferPlan), color = Secondary)
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                Button(onClick = { owner.seekBy(-10_000) }) { Text("−10s") }
                Button(onClick = { owner.togglePlayPause() }, modifier = Modifier.focusRequester(primary)) {
                    Text(if (ui.isPlaying) "Pause" else "Play")
                }
                Button(onClick = { owner.seekBy(30_000) }) { Text("+30s") }
                Button(onClick = onExit) { Text("Back") }
            }
        }
    }
}

private val MediaKeys = setOf(Key.MediaPlayPause, Key.MediaPlay, Key.MediaPause, Key.MediaFastForward, Key.MediaRewind)

@Composable
private fun AddonSettingsScreen(
    core: AndroidCore,
    addon: AddonRecord,
    onChanged: () -> Unit,
    onLinkAccount: () -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    val canLink = "auth" in addon.capabilities
    var status by remember { mutableStateOf(addon.status) }
    var account by remember { mutableStateOf<String?>(null) }
    var settings by remember { mutableStateOf<List<SettingRecord>>(emptyList()) }
    var secrets by remember { mutableStateOf<List<SecretStatus>>(emptyList()) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(reload) {
        onIo { core.settings(addon.id) to core.secretStatus(addon.id) }.onSuccess { (loadedSettings, loadedSecrets) ->
            settings = loadedSettings
            secrets = loadedSecrets
        }.onFailure { error = it.userMessage() }
        if ("account_status" in addon.capabilities && status == "enabled") {
            account = onIo { core.accountStatus(addon.id) }
                .fold(::accountLine) { "Account status unavailable: ${it.userMessage()}" }
        }
    }
    fun act(done: String, afterSuccess: () -> Unit = {}, call: () -> Unit) {
        message = null
        error = null
        scope.launch {
            onIo { call() }
                .onSuccess {
                    message = done
                    afterSuccess()
                    reload++
                    onChanged()
                }
                .onFailure { error = it.userMessage() }
        }
    }

    Page(addon.name) {
        val first = rememberInitialFocus(Unit)
        if (status != "enabled") {
            Body("This add-on is $status. Fix its credentials or settings below, then enable it.", ErrorText)
            Button(
                onClick = { act("Enabled.", afterSuccess = { status = "enabled" }) { core.enableAddon(addon.id) } },
                modifier = Modifier.focusRequester(first),
            ) { Text("Enable add-on") }
        }
        account?.let { Body(it, Secondary) }
        if (canLink && status == "enabled") {
            Button(onClick = onLinkAccount, modifier = Modifier.focusRequester(first)) { Text("Link account") }
        }
        message?.let { Body(it) }
        error?.let { Body(it, ErrorText) }
        if (secrets.isEmpty() && settings.isEmpty()) Body("This add-on has no settings.")
        if (canLink && secrets.isNotEmpty()) Body("Or enter credentials by hand:", Secondary)
        LazyColumn(verticalArrangement = Arrangement.spacedBy(14.dp)) {
            items(secrets, key = { "secret:" + it.name }) { secret ->
                var value by remember { mutableStateOf("") }
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Body("${secret.name}: ${if (secret.isSet) "set" else "not set"}", Secondary)
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(
                            value = value,
                            onValueChange = { value = it },
                            label = { FieldLabel(if (secret.isSet) "Replace ${secret.name}" else "Enter ${secret.name}") },
                            singleLine = true,
                            visualTransformation = PasswordVisualTransformation(),
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        )
                        Button(onClick = {
                            val entered = value
                            value = ""
                            act("Saved ${secret.name}.") { core.setSecret(addon.id, secret.name, entered) }
                        }) { Text("Save") }
                    }
                }
            }
            items(settings, key = { "setting:" + it.key }) { setting ->
                when {
                    setting.options.isNotEmpty() -> Button(onClick = {
                        val next = setting.options[(setting.options.indexOf(setting.value) + 1) % setting.options.size]
                        act("Saved ${setting.key}.") { core.saveSetting(addon.id, setting.key, next) }
                    }) { Text("${setting.key}: ${setting.value}") }
                    setting.valueType == "bool" -> Button(onClick = {
                        val next = if (setting.value == "true") "false" else "true"
                        act("Saved ${setting.key}.") { core.saveSetting(addon.id, setting.key, next) }
                    }) { Text("${setting.key}: ${if (setting.value == "true") "on" else "off"}") }
                    else -> {
                        var value by remember(setting.value) { mutableStateOf(setting.value) }
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                            OutlinedTextField(
                                value = value,
                                onValueChange = { value = it },
                                label = { FieldLabel(setting.key) },
                                singleLine = true,
                            )
                            Button(onClick = {
                                act("Saved ${setting.key}.") { core.saveSetting(addon.id, setting.key, value) }
                            }) { Text("Save") }
                        }
                    }
                }
            }
        }
    }
}

private fun accountLine(state: AccountState): String = when (state) {
    is AccountState.SignedIn -> state.displayName?.let { "Signed in as $it." } ?: "Signed in."
    AccountState.SignedOut -> "Not signed in."
    AccountState.Expired -> "Sign-in expired. Link the account again."
}

private sealed interface LinkStep {
    data object Starting : LinkStep
    data class Waiting(val prompt: SignInPrompt) : LinkStep
    data class Linked(val account: String) : LinkStep
    data object Expired : LinkStep
    data object Denied : LinkStep
    data class Failed(val message: String) : LinkStep
}

/**
 * Device-code sign-in (UC-04): show the code and where to enter it, then
 * poll until the provider approves or the code expires. The credentials go
 * straight into the vault in Rust; this screen never sees them. Back or
 * Cancel abandons the code; starting again replaces it in any case.
 */
@Composable
private fun LinkAccountScreen(
    core: AndroidCore,
    addon: AddonRecord,
    scope: CoroutineScope,
    onExit: () -> Unit,
) {
    val leave = {
        // On IO: the core lock may be held by an add-on call for a while.
        scope.launch { onIo { core.cancelSignIn(addon.id) } }
        onExit()
    }
    BackHandler(onBack = leave)
    var attempt by remember { mutableStateOf(0) }
    var step by remember { mutableStateOf<LinkStep>(LinkStep.Starting) }
    LaunchedEffect(attempt) {
        step = LinkStep.Starting
        val prompt = onIo { core.beginSignIn(addon.id) }.getOrElse {
            step = LinkStep.Failed(it.userMessage())
            return@LaunchedEffect
        }
        step = LinkStep.Waiting(prompt)
        val interval = prompt.intervalSecs.toLong().coerceAtLeast(1) * 1000
        while (true) {
            delay(interval)
            val state = onIo { core.pollSignIn(addon.id) }.getOrElse {
                step = LinkStep.Failed(it.userMessage())
                return@LaunchedEffect
            }
            step = when (state) {
                SignInState.WAITING -> continue
                SignInState.LINKED -> LinkStep.Linked(
                    if ("account_status" in addon.capabilities) {
                        onIo { core.accountStatus(addon.id) }.fold(::accountLine) { "Account linked." }
                    } else {
                        "Account linked."
                    },
                )
                SignInState.EXPIRED -> LinkStep.Expired
                SignInState.DENIED -> LinkStep.Denied
            }
            return@LaunchedEffect
        }
    }
    Page("Link ${addon.name}") {
        val action = rememberInitialFocus(step::class)
        when (val current = step) {
            LinkStep.Starting -> Body("Getting a sign-in code…")
            is LinkStep.Waiting -> {
                val prompt = current.prompt
                var now by remember(prompt) { mutableStateOf(System.currentTimeMillis() / 1000) }
                LaunchedEffect(prompt) {
                    while (true) {
                        delay(1_000)
                        now = System.currentTimeMillis() / 1000
                    }
                }
                Body("On your phone or computer, open", Secondary)
                Text(displayUrl(prompt.verificationUrl), fontSize = 40.sp, color = Primary)
                Body("and enter this code:", Secondary)
                Text(prompt.userCode, fontSize = 64.sp, letterSpacing = 6.sp, color = Primary)
                val left = prompt.expiresAtUnix.toLong() - now
                Body("Waiting for approval… the code expires in ${formatRemaining(left)}.", Secondary)
                Button(onClick = leave, modifier = Modifier.focusRequester(action)) { Text("Cancel") }
            }
            is LinkStep.Linked -> {
                Body(current.account)
                Button(onClick = onExit, modifier = Modifier.focusRequester(action)) { Text("Done") }
            }
            LinkStep.Expired -> Retry("The code expired before it was entered.", action) { attempt++ }
            LinkStep.Denied -> Retry("The sign-in was declined.", action) { attempt++ }
            is LinkStep.Failed -> Retry(current.message, action) { attempt++ }
        }
    }
}

@Composable
private fun Retry(problem: String, focus: FocusRequester, onRetry: () -> Unit) {
    Body(problem, ErrorText)
    Button(onClick = onRetry, modifier = Modifier.focusRequester(focus)) { Text("Get a new code") }
}

@Composable
private fun InstallScreen(
    core: AndroidCore,
    packagesDir: File?,
    trustedKeys: List<String>,
    onInstalled: () -> Unit,
    onExit: () -> Unit,
) {
    var rescan by remember { mutableStateOf(0) }
    var packages by remember { mutableStateOf<List<File>>(emptyList()) }
    var preview by remember { mutableStateOf<Pair<File, InstallPreview>?>(null) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(rescan) {
        packages = withContext(Dispatchers.IO) { findPackages(packagesDir) }
    }
    BackHandler { if (preview != null) preview = null else onExit() }

    Page("Install add-on") {
        message?.let { Body(it) }
        error?.let { Body(it, ErrorText) }
        val pending = preview
        if (pending != null) {
            ConsentPanel(
                preview = pending.second,
                busy = busy,
                onApprove = {
                    busy = true
                    error = null
                    scope.launch {
                        onIo {
                            core.installSigned(pending.first.absolutePath, trustedKeys, pending.second.permissionHash)
                        }.onSuccess {
                            message = "Installed ${it.name} ${it.version}."
                            preview = null
                            onInstalled()
                        }.onFailure { error = it.userMessage() }
                        busy = false
                    }
                },
                onCancel = { preview = null },
            )
        } else {
            if (trustedKeys.isEmpty()) {
                Body(
                    "This build trusts no signing keys, so nothing can be installed. " +
                        "Rebuild with -Prvah.trustedKeys=<hex public key>.",
                    ErrorText,
                )
            }
            Body("Copy a signed package and its .sig file to:", Secondary)
            Body(packagesDir?.absolutePath ?: "(app storage unavailable)")
            Body("for example: adb push real-debrid.rvah real-debrid.rvah.sig <that folder>", Secondary)
            val first = rememberInitialFocus(packages.size)
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Button(onClick = { rescan++ }, modifier = if (packages.isEmpty()) Modifier.focusRequester(first) else Modifier) {
                    Text("Rescan")
                }
            }
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(packages.size) { index ->
                    val file = packages[index]
                    Button(
                        onClick = {
                            error = null
                            message = null
                            scope.launch {
                                onIo { core.previewInstall(file.absolutePath, trustedKeys) }
                                    .onSuccess { preview = file to it }
                                    .onFailure { error = it.userMessage() }
                            }
                        },
                        modifier = if (index == 0) Modifier.focusRequester(first) else Modifier,
                    ) { Text(file.name) }
                }
            }
        }
    }
}

@Composable
private fun ConsentPanel(
    preview: InstallPreview,
    busy: Boolean,
    onApprove: () -> Unit,
    onCancel: () -> Unit,
) {
    val verb = if (preview.isUpdate) "Update" else "Install"
    Body("$verb ${preview.name} ${preview.version}?", Primary)
    Body("Signed by trusted key ${keyFingerprint(preview.signerKeyHex)}", Secondary)
    Body("It will be allowed to:", Secondary)
    Body("• provide: ${preview.capabilities.joinToString().ifEmpty { "nothing" }}")
    Body(
        "• reach the network: " +
            (if (preview.network.isEmpty()) "no hosts" else preview.network.joinToString()),
    )
    Body("• store up to ${formatBytes(preview.storageBytes.toLong())}")
    Body("• read secrets: ${preview.secrets.joinToString().ifEmpty { "none" }}")
    val approve = rememberInitialFocus(preview.permissionHash)
    Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
        Button(onClick = { if (!busy) onApprove() }, modifier = Modifier.focusRequester(approve)) {
            Text(if (busy) "Installing…" else if (preview.isUpdate) "Approve and update" else "Approve and install")
        }
        Button(onClick = onCancel) { Text("Cancel") }
    }
}

@Composable
private fun DiagnosticsScreen(core: AndroidCore, onExit: () -> Unit) {
    BackHandler(onBack = onExit)
    var reload by remember { mutableStateOf(0) }
    var events by remember { mutableStateOf<List<DiagnosticRecord>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(reload) {
        onIo { core.diagnostics(200u) }
            .onSuccess { events = it }
            .onFailure { error = it.userMessage() }
    }
    Page("Diagnostics") {
        val first = rememberInitialFocus(Unit)
        Button(onClick = { reload++ }, modifier = Modifier.focusRequester(first)) { Text("Refresh") }
        error?.let { Body(it, ErrorText) }
        val loaded = events
        when {
            loaded == null -> Body("Loading…")
            loaded.isEmpty() -> Body("No diagnostic events.")
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                items(loaded) { event ->
                    val source = event.addonId?.let { " · $it" }.orEmpty()
                    Body(
                        "${event.level}$source · ${event.code}: ${event.message}",
                        if (event.level == "error") ErrorText else Secondary,
                    )
                }
            }
        }
    }
}

/**
 * Run a blocking Rust call on the IO dispatcher. Unlike `runCatching`, this
 * rethrows cancellation, so leaving a screen never reports "cancelled" as a
 * failure into shared state such as the player.
 */
internal suspend fun <T> onIo(block: () -> T): Result<T> = try {
    Result.success(withContext(Dispatchers.IO) { block() })
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (error: Throwable) {
    Result.failure(error)
}

@Composable
internal fun Page(title: String, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        Text(title, fontSize = 34.sp, color = Primary)
        content()
    }
}

@Composable
internal fun Body(text: String, color: Color = Primary) {
    Text(text, color = color, fontSize = 18.sp)
}

/**
 * A requester that takes D-pad focus once its target is composed, and again
 * whenever [key] changes, so every screen starts with a focused control.
 */
@Composable
internal fun rememberInitialFocus(key: Any?, enabled: Boolean = true): FocusRequester {
    val requester = remember { FocusRequester() }
    LaunchedEffect(key, enabled) {
        if (!enabled) return@LaunchedEffect
        // Wait a frame first: a control composed with this effect starts
        // watching its focus in an effect of its own, and a focus change it
        // misses leaves it focused but drawn unfocused. A lazy row is
        // composed during layout, possibly a frame later; a screen with
        // nothing focusable never attaches the requester. Both are harmless.
        repeat(3) {
            withFrameNanos { }
            if (runCatching { requester.requestFocus() }.isSuccess) return@LaunchedEffect
        }
    }
    return requester
}
