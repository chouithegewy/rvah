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
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.media3.ui.compose.PlayerSurface
import androidx.tv.material3.Button
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Surface
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry

private enum class Screen { Home, Browse, Search, Player, Settings, Install, Diagnostics }

@Composable
fun RvahTvApp(core: AndroidCore, owner: RvahPlayerOwner) {
    MaterialTheme {
        var screen by remember { mutableStateOf(Screen.Home) }
        var selectedAddon by remember { mutableStateOf<String?>(null) }
        var selectedItem by remember { mutableStateOf<String?>(null) }
        var error by remember { mutableStateOf<String?>(null) }
        val addons = remember { mutableStateListOf<AddonRecord>() }
        val catalog = remember { mutableStateListOf<CatalogEntry>() }

        LaunchedEffect(Unit) {
            runCatching { withContext(Dispatchers.IO) { core.addons() } }
                .onSuccess { addons.addAll(it) }
                .onFailure { error = it.message }
        }

        Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFF0B0F14)) {
            when (screen) {
                Screen.Home -> HomeScreen(
                    addons = addons,
                    error = error,
                    onBrowse = { addon ->
                        selectedAddon = addon
                        screen = Screen.Browse
                    },
                    onSearch = { screen = Screen.Search },
                    onTools = { screen = it },
                )
                Screen.Browse -> BrowseScreen(
                    core = core,
                    addonId = selectedAddon.orEmpty(),
                    entries = catalog,
                    onPlay = { itemId ->
                        selectedItem = itemId
                        screen = Screen.Player
                    },
                    onError = { error = it },
                )
                Screen.Search -> SearchScreen(core, selectedAddon, onPlay = {
                    selectedAddon = it.first
                    selectedItem = it.second
                    screen = Screen.Player
                })
                Screen.Player -> PlayerScreen(
                    core,
                    owner,
                    selectedAddon.orEmpty(),
                    selectedItem.orEmpty(),
                    onBack = {
                        owner.stop()
                        screen = Screen.Home
                    },
                )
                Screen.Settings -> ToolScreen("Settings", "Add-on settings are stored by the secure Rust core.") {
                    screen = Screen.Home
                }
                Screen.Install -> ToolScreen("Install add-on", "Release installs require a package signed by a configured trusted key.") {
                    screen = Screen.Home
                }
                Screen.Diagnostics -> ToolScreen("Diagnostics", error ?: "No playback errors in this session.") {
                    screen = Screen.Home
                }
            }
        }
    }
}

@Composable
private fun HomeScreen(
    addons: List<AddonRecord>,
    error: String?,
    onBrowse: (String) -> Unit,
    onSearch: () -> Unit,
    onTools: (Screen) -> Unit,
) {
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(28.dp)) {
        Text("RVAH", fontSize = 38.sp, color = Color.White)
        Text("Your add-ons", color = Color.LightGray)
        LazyRow(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            items(addons, key = { it.id }) { addon ->
                Button(onClick = { onBrowse(addon.id) }) { Text(addon.name) }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            Button(onClick = onSearch) { Text("Search") }
            Button(onClick = { onTools(Screen.Settings) }) { Text("Settings") }
            Button(onClick = { onTools(Screen.Install) }) { Text("Install add-on") }
            Button(onClick = { onTools(Screen.Diagnostics) }) { Text("Diagnostics") }
        }
        error?.let { Text(it, color = Color(0xFFFF8A80)) }
    }
}

@Composable
private fun BrowseScreen(
    core: AndroidCore,
    addonId: String,
    entries: MutableList<CatalogEntry>,
    onPlay: (String) -> Unit,
    onError: (String) -> Unit,
) {
    var path by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(addonId, path) {
        runCatching { withContext(Dispatchers.IO) { core.browse(addonId, path) } }
            .onSuccess { entries.clear(); entries.addAll(it) }
            .onFailure { onError(it.message ?: "Browse failed") }
    }
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        Text("Browse", fontSize = 32.sp, color = Color.White)
        LazyRow(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            items(entries) { entry ->
                when (entry) {
                    is CatalogEntry.Directory -> Button(onClick = { path = entry.path }) { Text(entry.title) }
                    is CatalogEntry.Media -> Button(onClick = { onPlay(entry.id) }) { Text(entry.title) }
                }
            }
        }
    }
}

@Composable
private fun SearchScreen(
    core: AndroidCore,
    addonId: String?,
    onPlay: (Pair<String, String>) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<List<CatalogEntry>>(emptyList()) }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(22.dp)) {
        Text("Search", fontSize = 32.sp, color = Color.White)
        OutlinedTextField(value = query, onValueChange = { query = it }, label = { Text("Title") })
        Button(onClick = {
            val id = addonId ?: return@Button
            scope.launch {
                results = withContext(Dispatchers.IO) { core.search(id, query) }
            }
        }) { Text("Search") }
        LazyRow(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
            items(results) { entry ->
                if (entry is CatalogEntry.Media && addonId != null) {
                    Button(onClick = { onPlay(addonId to entry.id) }) { Text(entry.title) }
                }
            }
        }
    }
}

@Composable
private fun PlayerScreen(
    core: AndroidCore,
    owner: RvahPlayerOwner,
    addonId: String,
    itemId: String,
    onBack: () -> Unit,
) {
    var status by remember { mutableStateOf("Resolving video…") }
    var failure by remember { mutableStateOf<String?>(null) }
    DisposableEffect(owner) {
        owner.onFailure = { failure = it }
        onDispose { owner.onFailure = {} }
    }
    BackHandler(onBack = onBack)
    LaunchedEffect(addonId, itemId) {
        runCatching {
            withContext(Dispatchers.IO) { core.preparePlayback(addonId, itemId) }.toSpec()
        }.onSuccess {
            status = "Buffering…"
            owner.play(it)
        }.onFailure {
            failure = it.message ?: "Could not prepare playback"
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onPreviewKeyEvent {
                if (it.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                when (it.key) {
                    Key.DirectionLeft -> { owner.seekBy(-10_000); true }
                    Key.DirectionRight -> { owner.seekBy(10_000); true }
                    Key.MediaPlayPause, Key.Enter, Key.DirectionCenter -> {
                        if (owner.player.isPlaying) owner.player.pause() else owner.player.play()
                        true
                    }
                    Key.Back -> { onBack(); true }
                    else -> false
                }
            },
    ) {
        PlayerSurface(player = owner.player, modifier = Modifier.fillMaxSize())
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(40.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(failure ?: status, color = if (failure == null) Color.White else Color(0xFFFF8A80))
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                Button(onClick = { owner.seekBy(-10_000) }) { Text("−10s") }
                Button(onClick = {
                    if (owner.player.isPlaying) owner.player.pause() else owner.player.play()
                }) { Text(if (owner.player.isPlaying) "Pause" else "Play") }
                Button(onClick = { owner.seekBy(10_000) }) { Text("+10s") }
                Button(onClick = onBack) { Text("Back") }
            }
        }
    }
}

@Composable
private fun ToolScreen(title: String, body: String, onBack: () -> Unit) {
    BackHandler(onBack = onBack)
    Column(Modifier.fillMaxSize().padding(48.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        Text(title, fontSize = 32.sp, color = Color.White)
        Text(body, color = Color.LightGray)
        Button(onClick = onBack) { Text("Back") }
    }
}
