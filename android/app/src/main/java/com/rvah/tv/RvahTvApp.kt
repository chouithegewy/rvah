package com.rvah.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.material3.Text
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
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.ui.compose.PlayerSurface
import androidx.tv.material3.Button
import androidx.tv.material3.MaterialTheme
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry
import uniffi.rvah_android.DiagnosticRecord
import uniffi.rvah_android.InstallPreview
import uniffi.rvah_android.SecretStatus
import uniffi.rvah_android.SettingRecord

private val Background = Color(0xFF0B0F14)
private val Primary = Color.White
private val Secondary = Color.LightGray
private val ErrorText = Color(0xFFFF8A80)

/** Navigation state. Browse keeps its directory stack so Back from the player returns to it. */
private sealed interface Screen {
    data object Home : Screen
    data class Browse(val addonId: String, val addonName: String, val stack: List<String?> = listOf(null)) : Screen
    data object Search : Screen
    data class Player(val addonId: String, val itemId: String, val title: String, val returnTo: Screen) : Screen
    data object Settings : Screen
    data class AddonSettings(val addon: AddonRecord) : Screen
    data object Install : Screen
    data object Diagnostics : Screen
}

/** The screen Back eventually leads to: the player returns to where it was opened. */
private val Screen.base: Screen get() = if (this is Screen.Player) returnTo.base else this

private data class BrowseKey(val addonId: String, val path: String?)

/** Submitted search: Load more repeats this query, not whatever is typed now. */
private data class SearchResults(
    val generation: Int,
    val addonId: String,
    val query: String,
    val listing: Listing<CatalogEntry.Media>,
)

/** Search outlives a trip to the player; leaving it for another screen starts over. */
private class SearchModel {
    var addonId by mutableStateOf<String?>(null)
    var query by mutableStateOf("")
    var results by mutableStateOf<SearchResults?>(null)
    var searching by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
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
    MaterialTheme {
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
        onIo { core.addons() }
            .onSuccess {
                addons = it
                addonsError = null
            }
            .onFailure { addonsError = it.userMessage() }
    }
    val goHome = {
        refresh++
        screen = Screen.Home
    }
    val searchable = addons.filter { it.status == "enabled" && "search" in it.capabilities }

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
            onNavigate = { screen = it },
            onPlay = { itemId, title -> screen = Screen.Player(current.addonId, itemId, title, current) },
            onExit = goHome,
        )
        Screen.Search -> SearchScreen(
            core = core,
            addons = searchable,
            model = search,
            scope = scope,
            onPlay = { addonId, itemId, title -> screen = Screen.Player(addonId, itemId, title, Screen.Search) },
            onExit = goHome,
        )
        is Screen.Player -> PlayerScreen(
            core = core,
            owner = owner,
            target = current,
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
            onExit = { screen = Screen.Settings },
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
    onNavigate: (Screen) -> Unit,
    onPlay: (itemId: String, title: String) -> Unit,
    onExit: () -> Unit,
) {
    val level = BrowseKey(browse.addonId, browse.stack.last())
    var error by remember(level) { mutableStateOf<String?>(null) }
    LaunchedEffect(level) {
        if (level in listings) return@LaunchedEffect
        onIo { core.browse(level.addonId, level.path, null) }
            .onSuccess { listings[level] = Listing(it.entries, it.nextCursor) }
            .onFailure { error = it.userMessage() }
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
        when {
            error != null -> Body(error.orEmpty(), ErrorText)
            loaded == null -> Body("Loading…")
            loaded.entries.isEmpty() && !loaded.hasMore -> Body("Nothing here.")
            // Each level gets its own scroll and focus state.
            else -> key(level) {
                ListingView(
                    listing = loaded,
                    onOpen = { index, entry ->
                        update { it.opened(index) }
                        when (entry) {
                            is CatalogEntry.Directory -> onNavigate(browse.copy(stack = browse.stack + entry.path))
                            is CatalogEntry.Media -> onPlay(entry.id, entry.title)
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
private fun <T> CoroutineScope.loadMore(
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
            label = { Text("Search ${addon.name}") },
            singleLine = true,
            modifier = Modifier.focusRequester(field),
        )
        Button(onClick = {
            val query = model.query.trim()
            if (query.isEmpty()) return@Button
            val generation = ++model.generation
            model.searching = true
            model.error = null
            scope.launch {
                val outcome = onIo { core.search(addon.id, query, null) }
                if (generation != model.generation) return@launch
                model.searching = false
                outcome
                    .onSuccess { page ->
                        val media = page.entries.filterIsInstance<CatalogEntry.Media>()
                        model.results = SearchResults(generation, addon.id, query, Listing(media, page.nextCursor))
                    }
                    .onFailure { model.error = it.userMessage() }
            }
        }) { Text("Search") }
        when {
            model.searching -> Body("Searching…")
            model.error != null -> Body(model.error.orEmpty(), ErrorText)
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
    core: AndroidCore,
    owner: RvahPlayerOwner,
    target: Screen.Player,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    var started by remember(target) { mutableStateOf(false) }
    LaunchedEffect(target) {
        onIo { core.preparePlayback(target.addonId, target.itemId, target.title) }.onSuccess {
            started = true
            owner.play(it.toSpec())
        }.onFailure { owner.fail(it.userMessage()) }
    }
    val ui = owner.ui
    val primary = rememberInitialFocus(Unit)
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
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
        Column(
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

@Composable
private fun AddonSettingsScreen(
    core: AndroidCore,
    addon: AddonRecord,
    onChanged: () -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    var status by remember { mutableStateOf(addon.status) }
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
        message?.let { Body(it) }
        error?.let { Body(it, ErrorText) }
        if (secrets.isEmpty() && settings.isEmpty()) Body("This add-on has no settings.")
        LazyColumn(verticalArrangement = Arrangement.spacedBy(14.dp)) {
            items(secrets, key = { "secret:" + it.name }) { secret ->
                var value by remember { mutableStateOf("") }
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Body("${secret.name}: ${if (secret.isSet) "set" else "not set"}", Secondary)
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(
                            value = value,
                            onValueChange = { value = it },
                            label = { Text(if (secret.isSet) "Replace ${secret.name}" else "Enter ${secret.name}") },
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
                                label = { Text(setting.key) },
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
private suspend fun <T> onIo(block: () -> T): Result<T> = try {
    Result.success(withContext(Dispatchers.IO) { block() })
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (error: Throwable) {
    Result.failure(error)
}

@Composable
private fun Page(title: String, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        Text(title, fontSize = 34.sp, color = Primary)
        content()
    }
}

@Composable
private fun Body(text: String, color: Color = Primary) {
    Text(text, color = color, fontSize = 18.sp)
}

/**
 * A requester that takes D-pad focus once its target is composed, and again
 * whenever [key] changes, so every screen starts with a focused control.
 */
@Composable
private fun rememberInitialFocus(key: Any?, enabled: Boolean = true): FocusRequester {
    val requester = remember { FocusRequester() }
    LaunchedEffect(key, enabled) {
        if (!enabled) return@LaunchedEffect
        // A lazy row is composed during layout, possibly a frame later; a
        // screen with nothing focusable never attaches it. Both are harmless.
        repeat(3) {
            if (runCatching { requester.requestFocus() }.isSuccess) return@LaunchedEffect
            withFrameNanos { }
        }
    }
    return requester
}
