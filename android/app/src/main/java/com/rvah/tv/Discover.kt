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
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Text
import coil3.ImageLoader
import coil3.compose.AsyncImage
import coil3.compose.setSingletonImageLoaderFactory
import coil3.disk.DiskCache
import coil3.memory.MemoryCache
import coil3.request.crossfade
import okio.Path.Companion.toOkioPath
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
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
    /** Null when no add-ons can find and play releases. */
    onFindSources: ((ItemDetailsRecord) -> Unit)?,
    onEpisodes: (ItemDetailsRecord) -> Unit,
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
                    // The first action takes focus: Episodes for a series,
                    // else Find sources, else the first Find in.
                    val series = shown?.kind == "series"
                    val sourcesFirst = shown != null && !series && onFindSources != null
                    if (shown != null && series) {
                        Button(onClick = { onEpisodes(shown) }, modifier = Modifier.focusRequester(first)) {
                            Text("Episodes", maxLines = 1, softWrap = false)
                        }
                    }
                    if (sourcesFirst && onFindSources != null && shown != null) {
                        Button(onClick = { onFindSources(shown) }, modifier = Modifier.focusRequester(first)) {
                            Text("Find sources", maxLines = 1, softWrap = false)
                        }
                    }
                    players.forEachIndexed { index, player ->
                        Button(
                            onClick = { onFind(Finder(player, findQuery(shown?.seriesTitle ?: title, shown?.year))) },
                            modifier = if (index == 0 && !series && !sourcesFirst) Modifier.focusRequester(first) else Modifier,
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
                            modifier = if (index == 0 && players.isEmpty() && !series && !sourcesFirst) Modifier.focusRequester(first) else Modifier,
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
