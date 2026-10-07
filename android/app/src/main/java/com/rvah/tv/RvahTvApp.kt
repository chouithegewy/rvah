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
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
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
            onNavigate = { screen = it },
            onPlay = { itemId, title -> screen = Screen.Player(current.addonId, itemId, title, current) },
            onExit = goHome,
        )
        Screen.Search -> SearchScreen(
            core = core,
            addons = searchable,
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
    onNavigate: (Screen) -> Unit,
    onPlay: (itemId: String, title: String) -> Unit,
    onExit: () -> Unit,
) {
    val path = browse.stack.last()
    var entries by remember(browse.addonId, path) { mutableStateOf<List<CatalogEntry>?>(null) }
    var error by remember(browse.addonId, path) { mutableStateOf<String?>(null) }
    LaunchedEffect(browse.addonId, path) {
        onIo { core.browse(browse.addonId, path) }
            .onSuccess { entries = it }
            .onFailure { error = it.userMessage() }
    }
    BackHandler {
        if (browse.stack.size > 1) onNavigate(browse.copy(stack = browse.stack.dropLast(1))) else onExit()
    }
    Page(browse.addonName) {
        val loaded = entries
        when {
            error != null -> Body(error.orEmpty(), ErrorText)
            loaded == null -> Body("Loading…")
            loaded.isEmpty() -> Body("Nothing here.")
            else -> EntryList(
                entries = loaded,
                onDirectory = { onNavigate(browse.copy(stack = browse.stack + it.path)) },
                onMedia = { onPlay(it.id, it.title) },
            )
        }
    }
}

@Composable
private fun EntryList(
    entries: List<CatalogEntry>,
    onDirectory: (CatalogEntry.Directory) -> Unit,
    onMedia: (CatalogEntry.Media) -> Unit,
) {
    val first = rememberInitialFocus(entries)
    LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        items(entries.size) { index ->
            val modifier = if (index == 0) Modifier.focusRequester(first) else Modifier
            when (val entry = entries[index]) {
                is CatalogEntry.Directory -> Button(onClick = { onDirectory(entry) }, modifier = modifier) {
                    Text("▸ ${entry.title}")
                }
                is CatalogEntry.Media -> Button(onClick = { onMedia(entry) }, modifier = modifier) {
                    Text(listOfNotNull(entry.title, entry.year?.toString()).joinToString("  ·  "))
                }
            }
        }
    }
}

@Composable
private fun SearchScreen(
    core: AndroidCore,
    addons: List<AddonRecord>,
    onPlay: (addonId: String, itemId: String, title: String) -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    var selected by remember { mutableStateOf(addons.firstOrNull()) }
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<List<CatalogEntry>>(emptyList()) }
    var status by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    Page("Search") {
        val addon = selected
        if (addon == null) {
            Body("No installed add-on supports search.")
            return@Page
        }
        if (addons.size > 1) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                addons.forEach { candidate ->
                    Button(onClick = { selected = candidate }) {
                        Text(if (candidate == addon) "• ${candidate.name}" else candidate.name)
                    }
                }
            }
        }
        val field = rememberInitialFocus(Unit)
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text("Search ${addon.name}") },
            singleLine = true,
            modifier = Modifier.focusRequester(field),
        )
        Button(onClick = {
            if (query.isBlank()) return@Button
            status = "Searching…"
            error = null
            scope.launch {
                onIo { core.search(addon.id, query.trim()) }
                    .onSuccess {
                        results = it
                        status = if (it.isEmpty()) "No results." else null
                    }
                    .onFailure {
                        error = it.userMessage()
                        status = null
                    }
            }
        }) { Text("Search") }
        status?.let { Body(it) }
        error?.let { Body(it, ErrorText) }
        EntryList(
            entries = results.filterIsInstance<CatalogEntry.Media>(),
            onDirectory = {},
            onMedia = { onPlay(addon.id, it.id, it.title) },
        )
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
private fun rememberInitialFocus(key: Any?): FocusRequester {
    val requester = remember { FocusRequester() }
    LaunchedEffect(key) {
        // Not attached when the screen has nothing focusable yet; harmless.
        runCatching { requester.requestFocus() }
    }
    return requester
}
