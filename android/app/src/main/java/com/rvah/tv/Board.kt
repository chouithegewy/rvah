package com.rvah.tv

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.weight
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
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
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.Text
import coil3.SingletonImageLoader
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.CatalogEntry
import uniffi.rvah_android.CatalogPage
import uniffi.rvah_android.ItemDetailsRecord

/** What a board has loaded, kept while its titles' pages are open so Back finds it as it was. */
internal class BoardState {
    val listings = mutableStateMapOf<String, Listing<CatalogEntry>>()
    val failures = mutableStateMapOf<String, Throwable>()
    val details = mutableStateMapOf<String, ItemDetailsRecord>()
    val detailFailures = mutableStateMapOf<String, Boolean>()
    val autoplayedTrailers = mutableStateMapOf<String, Boolean>()
    /** The shelf last opened from; its remembered poster takes focus on return. */
    var homeColumn by mutableStateOf<String?>(null)
}

private val PosterBlank = Color(0xFF1E2633)

private sealed interface BoardItem {
    val spec: ColumnSpec
    val key: String

    data class Entry(
        override val spec: ColumnSpec,
        val entry: CatalogEntry,
        val index: Int,
    ) : BoardItem {
        override val key = spec.key + "\u0000" + index + "\u0000" + entry.idForBoard()
    }

    data class More(override val spec: ColumnSpec, val listing: Listing<CatalogEntry>) : BoardItem {
        override val key = spec.key + "\u0000more"
    }
}

private fun CatalogEntry.idForBoard(): String = when (this) {
    is CatalogEntry.Media -> id
    is CatalogEntry.Directory -> path
}

private fun AndroidCore.page(spec: ColumnSpec, cursor: String?): CatalogPage =
    spec.query?.let { search(spec.addonId, it, cursor) } ?: browse(spec.addonId, spec.path, cursor)

private fun detailKey(spec: ColumnSpec, entry: CatalogEntry.Media) = spec.addonId + "\u0000" + entry.id

/**
 * A single fast poster grid. Catalogs load only after this screen opens; the
 * old narrow, independently scrolling columns are deliberately avoided.
 * Focus updates the description pane without an extra Select press.
 */
@Composable
internal fun Board(
    core: AndroidCore,
    columns: List<ColumnSpec>,
    state: BoardState,
    scope: CoroutineScope,
    onOpen: (ColumnSpec, CatalogEntry) -> Unit,
    modifier: Modifier = Modifier,
    detailsAddons: Set<String> = emptySet(),
    onTrailer: (String, String) -> Unit = { _, _ -> },
) {
    columns.forEach { spec ->
        key(spec.key) {
            LaunchedEffect(spec.key) {
                if (spec.key in state.listings) return@LaunchedEffect
                onIo { core.page(spec, null) }
                    .onSuccess {
                        state.listings[spec.key] = Listing(it.entries, it.nextCursor)
                        state.failures.remove(spec.key)
                    }
                    .onFailure { state.failures[spec.key] = it }
            }
        }
    }

    val boardItems = buildList {
        columns.forEach { spec ->
            state.listings[spec.key]?.let { listing ->
                listing.entries.forEachIndexed { index, entry -> add(BoardItem.Entry(spec, entry, index)) }
                if (listing.hasMore) add(BoardItem.More(spec, listing))
            }
        }
    }
    val entries = boardItems.filterIsInstance<BoardItem.Entry>()
    val remembered = entries.firstOrNull { item ->
        item.spec.key == state.homeColumn && state.listings[item.spec.key]?.focusIndex == item.index
    } ?: entries.firstOrNull()
    var focused by remember(columns.map { it.key }) { mutableStateOf<BoardItem.Entry?>(null) }
    val focusRequester = remember(columns.map { it.key }) { FocusRequester() }
    var requestedFocus by remember(columns.map { it.key }) { mutableStateOf(false) }

    LaunchedEffect(remembered?.key) {
        if (remembered == null || requestedFocus) return@LaunchedEffect
        focused = remembered
        repeat(3) {
            withFrameNanos { }
            if (runCatching { focusRequester.requestFocus() }.isSuccess) {
                requestedFocus = true
                return@LaunchedEffect
            }
        }
    }

    val focusedEntry = focused
    val focusedDetails = focusedEntry?.let { item ->
        (item.entry as? CatalogEntry.Media)?.let { state.details[detailKey(item.spec, it)] }
    }
    LaunchedEffect(focusedEntry?.key) {
        val item = focusedEntry ?: return@LaunchedEffect
        val media = item.entry as? CatalogEntry.Media ?: return@LaunchedEffect
        if (item.spec.addonId !in detailsAddons) return@LaunchedEffect
        delay(220)
        val key = detailKey(item.spec, media)
        if (key in state.details || key in state.detailFailures) return@LaunchedEffect
        onIo { core.details(item.spec.addonId, media.id) }
            .onSuccess { state.details[key] = it }
            .onFailure { state.detailFailures[key] = true }
    }
    LaunchedEffect(focusedEntry?.key, focusedDetails?.trailers?.firstOrNull()?.url) {
        val item = focusedEntry ?: return@LaunchedEffect
        val trailer = focusedDetails?.trailers?.firstOrNull { youtubeVideoId(it.url) != null } ?: return@LaunchedEffect
        if (state.autoplayedTrailers[item.key] == true) return@LaunchedEffect
        delay(4_000)
        state.autoplayedTrailers[item.key] = true
        onTrailer(trailer.url, trailer.name)
    }

    // Warm the next screen's artwork and a small first batch of ratings while
    // the viewer is deciding what to open. Both caches are bounded.
    val context = LocalContext.current
    LaunchedEffect(entries.map { it.key }) {
        val loader = SingletonImageLoader.get(context)
        entries.asSequence().take(18).flatMap { item ->
            when (val entry = item.entry) {
                is CatalogEntry.Media -> sequenceOf(entry.poster, entry.fanart)
                is CatalogEntry.Directory -> sequenceOf(entry.poster)
            }.filterNotNull()
        }.distinct().forEach { url ->
            loader.enqueue(ImageRequest.Builder(context).data(url).build())
        }
        entries.asSequence()
            .filter { it.spec.addonId in detailsAddons && it.entry is CatalogEntry.Media }
            .take(8)
            .forEach { item ->
                val media = item.entry as CatalogEntry.Media
                val key = detailKey(item.spec, media)
                if (key !in state.details && key !in state.detailFailures) {
                    onIo { core.details(item.spec.addonId, media.id) }
                        .onSuccess { state.details[key] = it }
                        .onFailure { state.detailFailures[key] = true }
                    delay(80)
                }
            }
    }

    Row(
        modifier.fillMaxSize().padding(horizontal = 32.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(28.dp),
    ) {
        Box(Modifier.weight(0.7f).fillMaxHeight()) {
            when {
                entries.isNotEmpty() -> LazyVerticalGrid(
                    columns = GridCells.Adaptive(145.dp),
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                    contentPadding = PaddingValues(8.dp),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    items(boardItems, key = { it.key }) { item ->
                        when (item) {
                            is BoardItem.Entry -> {
                                val details = (item.entry as? CatalogEntry.Media)?.let { state.details[detailKey(item.spec, it)] }
                                PosterCard(
                                    item = item,
                                    details = details,
                                    modifier = if (item.key == remembered?.key) Modifier.focusRequester(focusRequester) else Modifier,
                                    onFocused = {
                                        focused = item
                                        state.homeColumn = item.spec.key
                                        state.listings[item.spec.key]?.let { state.listings[item.spec.key] = it.copy(focusIndex = item.index) }
                                    },
                                    onClick = {
                                        state.homeColumn = item.spec.key
                                        state.listings[item.spec.key]?.let { state.listings[item.spec.key] = it.opened(item.index) }
                                        onOpen(item.spec, item.entry)
                                    },
                                )
                            }
                            is BoardItem.More -> MoreCard(
                                spec = item.spec,
                                listing = item.listing,
                                onClick = {
                                    val update: ((Listing<CatalogEntry>) -> Listing<CatalogEntry>) -> Unit = { change ->
                                        state.listings[item.spec.key]?.let { state.listings[item.spec.key] = change(it) }
                                    }
                                    scope.loadMore(update) { cursor -> core.page(item.spec, cursor).let { it.entries to it.nextCursor } }
                                },
                            )
                        }
                    }
                }
                state.failures.isNotEmpty() -> Text(
                    state.failures.values.joinToString("\n") { it.userMessage() },
                    color = ErrorText,
                    fontSize = 14.sp,
                )
                state.listings.size < columns.size -> Text("Loading…", color = Secondary, fontSize = 14.sp)
                else -> Text("Nothing here.", color = Secondary, fontSize = 14.sp)
            }
        }
        PreviewPane(
            item = focusedEntry,
            details = focusedDetails,
            modifier = Modifier.weight(0.3f).fillMaxHeight(),
        )
    }
}

@Composable
private fun PosterCard(
    item: BoardItem.Entry,
    details: ItemDetailsRecord?,
    modifier: Modifier,
    onFocused: () -> Unit,
    onClick: () -> Unit,
) {
    val entry = item.entry
    val title: String
    val poster: String?
    val line: String
    when (entry) {
        is CatalogEntry.Media -> {
            title = entry.title
            poster = entry.poster
            line = entry.note ?: cardLine(entry.year, entry.rating)
        }
        is CatalogEntry.Directory -> {
            title = entry.title
            poster = entry.poster
            line = entry.subtitle.orEmpty()
        }
    }
    Card(
        onClick = onClick,
        modifier = modifier.onFocusChanged { if (it.isFocused) onFocused() },
        scale = CardDefaults.scale(focusedScale = 1.06f),
    ) {
        Column {
            Box(Modifier.fillMaxWidth().aspectRatio(2f / 3f).background(PosterBlank)) {
                poster?.let {
                    AsyncImage(model = it, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                }
            }
            Column(Modifier.padding(8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(title, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val rotten = details?.ratings?.firstOrNull { it.source == "rotten_tomatoes" }
                if (rotten != null) {
                    Row(horizontalArrangement = Arrangement.spacedBy(5.dp), verticalAlignment = Alignment.CenterVertically) {
                        Image(
                            painter = painterResource(R.drawable.ic_rating_tomato),
                            contentDescription = ratingLabel(rotten.source),
                            modifier = Modifier.size(18.dp),
                        )
                        Text(rotten.value, fontSize = 12.sp)
                    }
                } else if (line.isNotEmpty()) {
                    Text(line, fontSize = 12.sp, maxLines = 1, color = LocalContentColor.current.copy(alpha = 0.75f))
                }
                if (item.spec.tag != null) {
                    Text(item.spec.title, fontSize = 10.sp, maxLines = 1, color = LocalContentColor.current.copy(alpha = 0.6f))
                }
            }
        }
    }
}

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun PreviewPane(item: BoardItem.Entry?, details: ItemDetailsRecord?, modifier: Modifier) {
    val entry = item?.entry
    val title = when (entry) {
        is CatalogEntry.Media -> details?.title ?: entry.title
        is CatalogEntry.Directory -> entry.title
        null -> ""
    }
    val art = when (entry) {
        is CatalogEntry.Media -> details?.fanart ?: entry.fanart ?: details?.poster ?: entry.poster
        is CatalogEntry.Directory -> entry.poster
        null -> null
    }
    val description = when (entry) {
        is CatalogEntry.Media -> details?.synopsis ?: entry.synopsis
        is CatalogEntry.Directory -> entry.subtitle
        null -> null
    }
    Box(modifier.clip(RoundedCornerShape(10.dp)).background(Color(0xFF111821))) {
        art?.let {
            AsyncImage(model = it, contentDescription = null, contentScale = ContentScale.Crop, alpha = 0.28f, modifier = Modifier.fillMaxSize())
        }
        Box(Modifier.fillMaxSize().background(Brush.verticalGradient(0f to Color.Transparent, 0.55f to Color(0xDD111821))))
        Column(
            Modifier.align(Alignment.BottomStart).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (title.isEmpty()) {
                Text("Move to a title for details", color = Secondary, fontSize = 16.sp)
            } else {
                Text(title, color = Primary, fontSize = 23.sp, lineHeight = 28.sp, maxLines = 3, overflow = TextOverflow.Ellipsis)
                details?.ratings?.let { ratings ->
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        ratings.forEach { rating ->
                            Row(
                                Modifier.background(Color(0x55000000), RoundedCornerShape(5.dp)).padding(horizontal = 7.dp, vertical = 3.dp),
                                horizontalArrangement = Arrangement.spacedBy(5.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                ratingIcon(rating.source)?.let {
                                    Image(painterResource(it), ratingLabel(rating.source), Modifier.size(19.dp))
                                } ?: Text(ratingLabel(rating.source), fontSize = 11.sp)
                                Text(rating.value, fontSize = 13.sp)
                            }
                        }
                    }
                }
                description?.takeIf { it.isNotBlank() }?.let {
                    Text(it, color = Primary, fontSize = 15.sp, lineHeight = 20.sp, maxLines = 9, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

@Composable
private fun MoreCard(spec: ColumnSpec, listing: Listing<CatalogEntry>, onClick: () -> Unit) {
    Card(onClick = onClick, scale = CardDefaults.scale(focusedScale = 1.06f)) {
        Column(Modifier.fillMaxWidth().aspectRatio(2f / 3f).padding(16.dp), verticalArrangement = Arrangement.Center) {
            Text(
                when {
                    listing.loadingMore -> "Loading…"
                    listing.moreError != null -> "Try again"
                    else -> "More ${spec.title}"
                },
                fontSize = 15.sp,
            )
            listing.moreError?.let { Text(it, fontSize = 12.sp, color = ErrorText, modifier = Modifier.padding(top = 8.dp)) }
        }
    }
}
