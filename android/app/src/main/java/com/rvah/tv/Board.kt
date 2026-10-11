package com.rvah.tv

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.Text
import coil3.compose.AsyncImage
import kotlinx.coroutines.CoroutineScope
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry
import uniffi.rvah_android.CatalogPage

/** What a board has loaded, kept while its titles' pages are open so Back finds it as it was. */
internal class BoardState {
    val listings = mutableStateMapOf<String, Listing<CatalogEntry>>()
    val failures = mutableStateMapOf<String, Throwable>()
    /** The column last opened from; it takes focus when the board shows again. */
    var homeColumn by mutableStateOf<String?>(null)
}

private val ColumnWidth = 280.dp
private val PosterBlank = Color(0xFF1E2633)

/**
 * Columns side by side, each scrolling down through its titles, with its
 * name at the top. Left and right move between columns; each column loads
 * its first page when it comes into view, and pages on with More.
 */
@Composable
internal fun Board(
    core: AndroidCore,
    columns: List<ColumnSpec>,
    state: BoardState,
    scope: CoroutineScope,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    modifier: Modifier = Modifier,
) {
    val home = columns.indexOfFirst { it.key == state.homeColumn }.coerceAtLeast(0)
    LazyRow(
        state = rememberLazyListState(initialFirstVisibleItemIndex = home),
        horizontalArrangement = Arrangement.spacedBy(20.dp),
        contentPadding = PaddingValues(horizontal = 32.dp),
        modifier = modifier.fillMaxSize(),
    ) {
        itemsIndexed(columns, key = { _, spec -> spec.key }) { index, spec ->
            BoardColumn(
                core = core,
                spec = spec,
                state = state,
                scope = scope,
                takesFocus = index == home,
                onOpen = { entry ->
                    state.homeColumn = spec.key
                    onOpen(spec, entry)
                },
            )
        }
    }
}

private fun AndroidCore.page(spec: ColumnSpec, cursor: String?): CatalogPage =
    spec.query?.let { search(spec.addonId, it, cursor) } ?: browse(spec.addonId, spec.path, cursor)

@Composable
private fun BoardColumn(
    core: AndroidCore,
    spec: ColumnSpec,
    state: BoardState,
    scope: CoroutineScope,
    takesFocus: Boolean,
    onOpen: (CatalogEntry) -> Unit,
) {
    val key = spec.key
    LaunchedEffect(key) {
        if (key in state.listings) return@LaunchedEffect
        onIo { core.page(spec, null) }
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
    // The home column focuses its remembered card once loaded; a column also
    // takes focus back when a More page lands (or fails) in it.
    val focus = remember { FocusRequester() }
    var claim by remember { mutableStateOf(takesFocus) }
    LaunchedEffect(claim, listing?.loadingMore, listing?.entries?.size) {
        if (!claim || listing == null || listing.entries.isEmpty() || listing.loadingMore) return@LaunchedEffect
        repeat(3) {
            // A frame first: the card must watch its focus before it gets it.
            withFrameNanos { }
            if (runCatching { focus.requestFocus() }.isSuccess) {
                claim = false
                return@LaunchedEffect
            }
        }
        claim = false
    }
    Column(Modifier.width(ColumnWidth).fillMaxHeight()) {
        Text(spec.title, fontSize = 17.sp, color = Primary, maxLines = 1, overflow = TextOverflow.Ellipsis)
        spec.tag?.let { Text(it, fontSize = 12.sp, color = Secondary, maxLines = 1) }
        Spacer(Modifier.height(6.dp))
        val failure = state.failures[key]
        when {
            failure != null -> Text(failure.userMessage(), fontSize = 14.sp, color = ErrorText)
            listing == null -> Text("Loading…", fontSize = 14.sp, color = Secondary)
            listing.entries.isEmpty() && !listing.hasMore -> Text("Nothing here.", fontSize = 14.sp, color = Secondary)
            else -> LazyColumn(
                state = rememberLazyListState(initialFirstVisibleItemIndex = listing.focusIndex),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                // Room for a focused card to grow.
                contentPadding = PaddingValues(horizontal = 6.dp, vertical = 8.dp),
            ) {
                itemsIndexed(listing.entries) { index, entry ->
                    EntryCard(
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
                                scope.loadMore(update) { cursor -> core.page(spec, cursor).let { it.entries to it.nextCursor } }
                            },
                        )
                    }
                }
            }
        }
    }
}

/** A title (or folder) as one compact row: a small poster, its name, year and score. */
@Composable
private fun EntryCard(entry: CatalogEntry, modifier: Modifier, onClick: () -> Unit) {
    val (title, poster, line) = when (entry) {
        is CatalogEntry.Media -> Triple(entry.title, entry.poster, entry.note ?: cardLine(entry.year, entry.rating))
        is CatalogEntry.Directory -> Triple(entry.title, entry.poster, entry.subtitle.orEmpty())
    }
    Card(
        onClick = onClick,
        modifier = modifier.fillMaxWidth(),
        scale = CardDefaults.scale(focusedScale = 1.04f),
    ) {
        Row(
            Modifier.padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.width(48.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(4.dp)).background(PosterBlank),
            ) {
                poster?.let {
                    AsyncImage(model = it, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, fontSize = 15.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (line.isNotEmpty()) {
                    Text(line, fontSize = 12.sp, maxLines = 1, color = LocalContentColor.current.copy(alpha = 0.75f))
                }
            }
        }
    }
}

@Composable
private fun MoreCard(listing: Listing<CatalogEntry>, modifier: Modifier, onClick: () -> Unit) {
    Card(onClick = onClick, modifier = modifier.fillMaxWidth(), scale = CardDefaults.scale(focusedScale = 1.04f)) {
        Text(
            when {
                listing.loadingMore -> "Loading…"
                listing.moreError != null -> "Try again"
                else -> "More"
            },
            fontSize = 15.sp,
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
        )
        listing.moreError?.let {
            Text(it, fontSize = 12.sp, color = ErrorText, modifier = Modifier.padding(start = 14.dp, end = 14.dp, bottom = 10.dp))
        }
    }
}
