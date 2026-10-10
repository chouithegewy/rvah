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
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Text
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.ReleaseRecord
import uniffi.rvah_android.SourceQuery

/** What a Sources screen finds releases of: a film or an episode. */
internal data class SourcesTarget(
    /** The add-on and item the releases are of; progress is kept under them. */
    val titleAddonId: String,
    val titleItemId: String,
    /** "Acme Corp · S01E02" */
    val heading: String,
    val query: SourceQuery,
)

/** A release chosen to play, through a debrid add-on. */
internal data class ReleasePick(val debridId: String, val release: ReleaseRecord, val target: SourcesTarget)

/** One search, kept while its player is in use, so Back finds the list as it was. */
internal class SourcesState(val target: SourcesTarget) {
    var releases by mutableStateOf<List<ReleaseRecord>?>(null)
    var failure by mutableStateOf<Throwable?>(null)
    /** Why a release did not play, by info hash ("not cached on Real-Debrid"). */
    val tried = mutableStateMapOf<String, String>()
    /** The row to focus: the one last chosen. */
    var focusIndex by mutableStateOf(0)
}

/** Releases of a film or episode from a sources add-on; choosing one plays it. */
@Composable
internal fun SourcesScreen(
    core: AndroidCore,
    state: SourcesState,
    source: AddonRecord,
    onPlay: (ReleaseRecord) -> Unit,
    onExit: () -> Unit,
) {
    BackHandler(onBack = onExit)
    LaunchedEffect(state) {
        if (state.releases != null || state.failure != null) return@LaunchedEffect
        onIo { core.sources(source.id, state.target.query) }
            .onSuccess { state.releases = it }
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
                                state.tried[release.infoHash]?.let { Text(it, fontSize = 14.sp, color = ErrorText) }
                            }
                        }
                    }
                }
            }
        }
    }
}
