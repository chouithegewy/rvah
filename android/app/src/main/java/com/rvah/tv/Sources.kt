package com.rvah.tv

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Text
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.AndroidCoreException
import uniffi.rvah_android.AndroidCoreInterface
import uniffi.rvah_android.NativePreparedPlayback
import uniffi.rvah_android.ReleaseRecord
import uniffi.rvah_android.ScrobbleTarget
import uniffi.rvah_android.SourceQuery

/** What a Sources screen finds releases of: a film or an episode. */
internal data class SourcesTarget(
    /** The add-on and item the releases are of; progress is kept under them. */
    val titleAddonId: String,
    val titleItemId: String,
    /** "Acme Corp · S01E02" */
    val heading: String,
    val query: SourceQuery,
    /** How scrobbling add-ons name it, when it has a TMDB id. */
    val scrobble: ScrobbleTarget? = null,
)

/** A release chosen to play, through a debrid add-on. */
internal data class ReleasePick(val debridId: String, val release: ReleaseRecord, val target: SourcesTarget)

/** At most this many releases are tried for one choice. */
internal const val RELEASES_PER_CHOICE = 5

/** One search, kept while its player is in use, so Back finds the list as it was. */
internal class SourcesState(val target: SourcesTarget) {
    /** As listed: releases the debrid service refused lately come last. */
    var releases by mutableStateOf<List<ReleaseRecord>?>(null)
    var failure by mutableStateOf<Throwable?>(null)
    /** Why a release did not play, by info hash ("not cached on Real-Debrid"). */
    val tried = mutableStateMapOf<String, String>()
    /** Releases the debrid service refused before this search, by info hash, with its reason. */
    var refused by mutableStateOf<Map<String, String>>(emptyMap())
    /** The row to focus: the one last chosen, or the last tried after it. */
    var focusIndex by mutableStateOf(0)
    /** The release playing, which may be a later one than the one chosen. */
    @Volatile var playing: ReleaseRecord? = null

    /** List [found], the ones in [refusedBefore] last. */
    fun show(found: List<ReleaseRecord>, refusedBefore: Map<String, String>) {
        refused = refusedBefore
        releases = found.sortedBy { it.infoHash in refusedBefore }
    }

    /**
     * What choosing [pick] tries, in order: it, then the releases listed
     * after it that have not failed, up to [RELEASES_PER_CHOICE] in all.
     */
    fun candidates(pick: ReleaseRecord): List<ReleaseRecord> {
        val listed = releases.orEmpty()
        val after = listed.drop(listed.indexOfFirst { it.infoHash == pick.infoHash } + 1)
            .filter { it.infoHash !in tried && it.infoHash !in refused && it.infoHash != pick.infoHash }
        return (listOf(pick) + after).take(RELEASES_PER_CHOICE)
    }

    /** Note why [release] did not play, and keep the list's focus on it. */
    fun failed(release: ReleaseRecord, reason: String) = Snapshot.withMutableSnapshot {
        tried[release.infoHash] = reason
        focusAt(release)
    }

    fun focusAt(release: ReleaseRecord) {
        releases?.indexOfFirst { it.infoHash == release.infoHash }?.takeIf { it >= 0 }?.let { focusIndex = it }
    }
}

/**
 * Play [pick], or when it cannot play (not cached, refused), each next
 * release from [SourcesState.candidates] in turn; [progress] hears which
 * try is under way. Every release that fails is marked in [state]; when
 * none plays, the last failure is thrown. Call off the main thread.
 */
internal fun prepareFirstPlayable(
    core: AndroidCoreInterface,
    pick: ReleasePick,
    state: SourcesState?,
    title: String,
    progress: (String) -> Unit,
): NativePreparedPlayback {
    val candidates = state?.candidates(pick.release) ?: listOf(pick.release)
    var last: AndroidCoreException.Unavailable? = null
    candidates.forEachIndexed { index, release ->
        if (index > 0) progress("That release can't play; trying another (${index + 1} of ${candidates.size})…")
        try {
            val prepared = core.prepareRelease(
                pick.debridId,
                release,
                pick.target.query.season,
                pick.target.query.episode,
                pick.target.titleAddonId,
                pick.target.titleItemId,
                title,
            )
            state?.playing = release
            state?.let { Snapshot.withMutableSnapshot { it.focusAt(release) } }
            return prepared
        } catch (e: AndroidCoreException.Unavailable) {
            state?.failed(release, e.reason)
            last = e
        }
    }
    throw last ?: IllegalStateException("no release to try")
}

/** Releases of a film or episode from a sources add-on; choosing one plays it. */
@Composable
internal fun SourcesScreen(
    core: AndroidCore,
    state: SourcesState,
    source: AddonRecord,
    debrid: AddonRecord,
    onPlay: (ReleaseRecord) -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    LaunchedEffect(state) {
        if (state.releases != null || state.failure != null) return@LaunchedEffect
        onIo {
            val found = core.sources(source.id, state.target.query)
            // Only ordering: a list without it is still a list.
            found to runCatching { core.refusedReleases(debrid.id) }.getOrDefault(emptyMap())
        }
            .onSuccess { (found, refused) -> state.show(found, refused) }
            .onFailure { state.failure = it }
    }
    Page(state.target.heading) {
        val releases = state.releases
        val failure = state.failure
        when {
            failure != null -> Body(failure.userMessage(), ErrorText)
            releases == null -> Body("Searching ${source.name}… this can take a while.", Secondary)
            releases.isEmpty() -> Body(
                "${source.name} found no releases. It searches the indexers you added to it.",
                Secondary,
            )
            else -> {
                val focus = rememberInitialFocus(state.focusIndex to releases.size)
                LazyColumn(
                    state = rememberLazyListState(initialFirstVisibleItemIndex = state.focusIndex),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                    contentPadding = PaddingValues(bottom = 48.dp),
                ) {
                    itemsIndexed(releases) { index, release ->
                        Button(
                            onClick = {
                                state.focusIndex = index
                                onPlay(release)
                            },
                            modifier = if (index == state.focusIndex) Modifier.focusRequester(focus) else Modifier,
                        ) {
                            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(release.name, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                val line = releaseLine(release.name, release.sizeBytes, release.seeders, release.origin)
                                if (line.isNotEmpty()) Text(line, fontSize = 14.sp)
                                val failed = state.tried[release.infoHash]
                                    ?: state.refused[release.infoHash]?.let { "Refused before: $it" }
                                failed?.let { Text(it, fontSize = 14.sp, color = ErrorText) }
                            }
                        }
                    }
                }
            }
        }
    }
}
