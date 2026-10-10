package com.rvah.tv

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Card
import androidx.tv.material3.Text
import coil3.ImageLoader
import coil3.compose.AsyncImage
import coil3.compose.setSingletonImageLoaderFactory
import coil3.disk.DiskCache
import coil3.memory.MemoryCache
import coil3.request.crossfade
import kotlinx.coroutines.CoroutineScope
import okio.Path.Companion.toOkioPath
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry
import uniffi.rvah_android.ItemDetailsRecord

/**
 * Coil for posters and backdrops: a small memory cache, since ExoPlayer's
 * read-ahead shares the heap, and 64 MiB on disk. Artwork URLs reaching the
 * UI are https on hosts their add-on declared (the core drops the rest).
 */
@Composable
internal fun ArtworkLoader() {
    setSingletonImageLoaderFactory { context ->
        ImageLoader.Builder(context)
            .memoryCache { MemoryCache.Builder().maxSizePercent(context, 0.1).build() }
            .diskCache {
                DiskCache.Builder()
                    .directory(context.cacheDir.resolve("artwork").toOkioPath())
                    .maxSizeBytes(64L * 1024 * 1024)
                    .build()
            }
            .crossfade(true)
            .build()
    }
}

/**
 * The rows of an add-on with details pages, kept while its rows, details,
 * search and player screens are in use, so Back returns to the same poster.
 */
internal class DiscoverState(val addonId: String) {
    val listings = mutableStateMapOf<BrowseKey, Listing<CatalogEntry>>()
    val failures = mutableStateMapOf<BrowseKey, Throwable>()
    /** The row last opened from: it takes focus when the rows show again. */
    var lastRow by mutableStateOf<String?>(null)
}

private val PosterWidth = 150.dp

/** An add-on's root directories as rows of posters, each loading its first page when composed. */
@Composable
internal fun DiscoverRows(
    core: AndroidCore,
    rows: List<CatalogEntry.Directory>,
    state: DiscoverState,
    scope: CoroutineScope,
    onOpen: (CatalogEntry) -> Unit,
) {
    val homeRow = rows.indexOfFirst { it.path == state.lastRow }.coerceAtLeast(0)
    LazyColumn(
        state = rememberLazyListState(initialFirstVisibleItemIndex = homeRow),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        contentPadding = PaddingValues(bottom = 48.dp),
    ) {
        itemsIndexed(rows, key = { _, row -> row.path }) { index, row ->
            PosterRow(
                core = core,
                key = BrowseKey(state.addonId, row.path),
                title = row.title,
                state = state,
                scope = scope,
                takesFocus = index == homeRow,
                onOpen = { entry ->
                    state.lastRow = row.path
                    onOpen(entry)
                },
            )
        }
    }
}

@Composable
private fun PosterRow(
    core: AndroidCore,
    key: BrowseKey,
    title: String,
    state: DiscoverState,
    scope: CoroutineScope,
    takesFocus: Boolean,
    onOpen: (CatalogEntry) -> Unit,
) {
    LaunchedEffect(key) {
        if (key in state.listings) return@LaunchedEffect
        onIo { core.browse(key.addonId, key.path, null) }
            .onSuccess {
                state.listings[key] = Listing(it.entries, it.nextCursor)
                state.failures.remove(key)
            }
            .onFailure { state.failures[key] = it }
    }
    val update: ((Listing<CatalogEntry>) -> Listing<CatalogEntry>) -> Unit = { change ->
        state.listings[key]?.let { state.listings[key] = change(it) }
    }
    val listing = state.listings[key]
    // Focus the row's remembered poster once loaded (the home row), and
    // again when a "More" page lands or fails (the row it was asked in).
    val focus = remember { FocusRequester() }
    var claim by remember { mutableStateOf(takesFocus) }
    LaunchedEffect(claim, listing?.loadingMore, listing?.entries?.size) {
        if (!claim || listing == null || listing.entries.isEmpty() || listing.loadingMore) return@LaunchedEffect
        repeat(3) {
            if (runCatching { focus.requestFocus() }.isSuccess) {
                claim = false
                return@LaunchedEffect
            }
            withFrameNanos { }
        }
        claim = false
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, fontSize = 22.sp, color = Primary)
        val failure = state.failures[key]
        when {
            failure != null -> Body(failure.userMessage(), ErrorText)
            listing == null -> Body("Loading…", Secondary)
            listing.entries.isEmpty() -> Body("Nothing here.", Secondary)
            else -> LazyRow(
                state = rememberLazyListState(initialFirstVisibleItemIndex = listing.focusIndex),
                horizontalArrangement = Arrangement.spacedBy(18.dp),
                // Room for a focused card to grow.
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 14.dp),
            ) {
                itemsIndexed(listing.entries) { index, entry ->
                    PosterCard(
                        entry = entry,
                        modifier = if (index == listing.focusIndex) Modifier.focusRequester(focus) else Modifier,
                        onClick = {
                            update { it.opened(index) }
                            onOpen(entry)
                        },
                    )
                }
                if (listing.hasMore) {
                    item {
                        MoreCard(
                            listing = listing,
                            modifier = if (listing.focusIndex == listing.entries.size) Modifier.focusRequester(focus) else Modifier,
                            onClick = {
                                claim = true
                                scope.loadMore(update) { cursor ->
                                    core.browse(key.addonId, key.path, cursor).let { it.entries to it.nextCursor }
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

private fun CatalogEntry.title(): String = when (this) {
    is CatalogEntry.Directory -> title
    is CatalogEntry.Media -> title
}

private fun CatalogEntry.poster(): String? = when (this) {
    is CatalogEntry.Directory -> poster
    is CatalogEntry.Media -> poster
}

@Composable
private fun PosterCard(entry: CatalogEntry, modifier: Modifier, onClick: () -> Unit) {
    // Room below the card for it to grow into when focused, over its title.
    Column(Modifier.width(PosterWidth), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Card(onClick = onClick, modifier = modifier.width(PosterWidth).aspectRatio(2f / 3f)) {
            val poster = entry.poster()
            if (poster != null) {
                AsyncImage(
                    model = poster,
                    contentDescription = entry.title(),
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Box(Modifier.fillMaxSize().background(Color(0xFF1E2633)).padding(10.dp), contentAlignment = Alignment.Center) {
                    Text(entry.title(), color = Primary, textAlign = TextAlign.Center, maxLines = 4, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        Text(entry.title(), color = Secondary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun MoreCard(listing: Listing<CatalogEntry>, modifier: Modifier, onClick: () -> Unit) {
    Card(onClick = onClick, modifier = modifier.width(PosterWidth).aspectRatio(2f / 3f)) {
        Box(Modifier.fillMaxSize().background(Color(0xFF1E2633)).padding(10.dp), contentAlignment = Alignment.Center) {
            Text(
                when {
                    listing.loadingMore -> "Loading…"
                    listing.moreError != null -> "Try again"
                    else -> "More"
                },
                color = Primary,
                fontSize = 20.sp,
            )
        }
    }
}

/** A search add-on that can play what it finds: "Find in <name>". */
internal data class Finder(val addon: AddonRecord, val query: String)

/**
 * A details page: backdrop, poster, facts, ratings, synopsis, then actions —
 * find the title in each add-on that can play it, and its trailers.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun DetailsScreen(
    core: AndroidCore,
    addonId: String,
    itemId: String,
    fallbackTitle: String,
    players: List<AddonRecord>,
    onFind: (Finder) -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    var details by remember(itemId) { mutableStateOf<ItemDetailsRecord?>(null) }
    var failure by remember(itemId) { mutableStateOf<Throwable?>(null) }
    var notice by remember(itemId) { mutableStateOf<String?>(null) }
    LaunchedEffect(addonId, itemId) {
        onIo { core.details(addonId, itemId) }
            .onSuccess { details = it }
            .onFailure { failure = it }
    }
    val context = LocalContext.current
    Box(Modifier.fillMaxSize().background(Background)) {
        val shown = details
        shown?.fanart?.let {
            AsyncImage(
                model = it,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                alpha = 0.45f,
                modifier = Modifier.fillMaxSize(),
            )
        }
        Box(
            Modifier.fillMaxSize().background(
                Brush.horizontalGradient(0f to Background, 0.55f to Background.copy(alpha = 0.85f), 1f to Background.copy(alpha = 0.2f)),
            ),
        )
        Row(Modifier.fillMaxSize().padding(48.dp), horizontalArrangement = Arrangement.spacedBy(36.dp)) {
            Box(
                Modifier.width(240.dp).aspectRatio(2f / 3f).background(Color(0xFF1E2633), RoundedCornerShape(8.dp)),
            ) {
                shown?.poster?.let {
                    AsyncImage(model = it, contentDescription = shown.title, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                }
            }
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                // The theme's line height suits body text; a wrapped title needs its own.
                Text(
                    shown?.title ?: fallbackTitle,
                    fontSize = 36.sp,
                    lineHeight = 44.sp,
                    color = Primary,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                shown?.tagline?.let { Text(it, color = Secondary, fontStyle = FontStyle.Italic, fontSize = 18.sp) }
                if (shown != null) {
                    val facts = detailsLine(shown.year, shown.runtimeSecs, shown.genres)
                    if (facts.isNotEmpty()) Text(facts, color = Secondary, fontSize = 18.sp)
                    if (shown.ratings.isNotEmpty()) {
                        // Wraps to a second line rather than squeezing a chip.
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            shown.ratings.forEach { rating ->
                                Text(
                                    "${ratingLabel(rating.source)}  ${rating.value}",
                                    color = Primary,
                                    fontSize = 16.sp,
                                    maxLines = 1,
                                    softWrap = false,
                                    modifier = Modifier
                                        .background(Color(0x33FFFFFF), RoundedCornerShape(6.dp))
                                        .padding(horizontal = 10.dp, vertical = 4.dp),
                                )
                            }
                        }
                    }
                } else if (failure == null) {
                    Body("Loading…", Secondary)
                }
                failure?.let { Body(it.userMessage(), ErrorText) }
                // Actions before the synopsis, so they are always on screen.
                val first = rememberInitialFocus(shown != null)
                val title = shown?.title ?: fallbackTitle
                // Wraps like the ratings: a long trailer name must not squeeze a button.
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    players.forEachIndexed { index, player ->
                        Button(
                            onClick = { onFind(Finder(player, findQuery(title, shown?.year))) },
                            modifier = if (index == 0) Modifier.focusRequester(first) else Modifier,
                        ) { Text("Find in ${player.name}", maxLines = 1, softWrap = false) }
                    }
                    shown?.trailers?.forEachIndexed { index, trailer ->
                        Button(
                            onClick = {
                                notice = try {
                                    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(trailer.url)))
                                    null
                                } catch (missing: ActivityNotFoundException) {
                                    "No app on this TV opens YouTube links."
                                }
                            },
                            modifier = if (index == 0 && players.isEmpty()) Modifier.focusRequester(first) else Modifier,
                        ) { Text(if (index == 0) "Play trailer" else trailer.name, maxLines = 1, softWrap = false) }
                    }
                }
                notice?.let { Body(it, ErrorText) }
                if (players.isEmpty() && shown != null) {
                    // Real-Debrid before 0.3.0, say, plays but cannot search.
                    Body("To watch this, install or update an add-on that can search and play, such as Real-Debrid.", Secondary)
                }
                shown?.synopsis?.let {
                    Text(it, color = Primary, fontSize = 18.sp, maxLines = 6, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}
