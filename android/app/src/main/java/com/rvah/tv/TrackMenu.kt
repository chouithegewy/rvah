package com.rvah.tv

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Text

/** Which of the player's track menus is open. */
internal enum class TrackMenu(val title: String) {
    Audio("Audio"),
    Subtitles("Subtitles"),
    Online("Subtitles online"),
}

/** One row of a track menu. */
internal data class MenuRow(val name: String, val ticked: Boolean = false, val onClick: () -> Unit)

/** The track a menu's button names: the selected one's language, else Off (subtitles) or a dash. */
internal fun trackButtonLabel(menu: TrackMenu, options: List<TrackOption>): String {
    val chosen = options.firstOrNull { it.selected }?.let { shortTrackName(it.name) }
    return "${menu.title}: ${chosen ?: if (menu == TrackMenu.Subtitles) "Off" else "—"}"
}

/** How an online subtitle file reads in the menu: language, then what it was made for. */
internal fun onlineName(file: SubtitleSpec): String = trackName(file.language, file.label, 0, forced = false)

/**
 * The rows of [menu]: the file's tracks (Subtitles adds Off first, and
 * Find more online last when a subtitles add-on can look); the online
 * menu shows the search, then what it found. [open] switches menus;
 * [close] closes them.
 */
internal fun trackMenuRows(
    menu: TrackMenu,
    ui: PlayerUi,
    owner: RvahPlayerOwner,
    open: (TrackMenu) -> Unit,
    close: () -> Unit,
): List<MenuRow> = when (menu) {
    TrackMenu.Audio -> ui.audio.map { option -> MenuRow(option.name, option.selected) { owner.select(option); close() } }
    TrackMenu.Subtitles -> buildList {
        add(MenuRow("Off", ui.subtitles.none { it.selected }) { owner.subtitlesOff(); close() })
        ui.subtitles.forEach { option -> add(MenuRow(option.name, option.selected) { owner.select(option); close() }) }
        if (ui.online != OnlineFind.Unavailable) {
            add(MenuRow("Find more online…") { owner.findOnline(); open(TrackMenu.Online) })
        }
    }
    TrackMenu.Online -> when (val state = ui.online) {
        OnlineFind.Unavailable, OnlineFind.NotYet, OnlineFind.Searching -> listOf(MenuRow("Searching…") {})
        is OnlineFind.Failed -> listOf(MenuRow(state.message) { close() })
        is OnlineFind.Found -> state.files.map { file ->
            MenuRow(onlineName(file), ticked = file.url == ui.onlineInUse) { owner.useOnline(file); close() }
        }.ifEmpty { listOf(MenuRow("Nothing found online.") { close() }) }
    }
}

/** A menu of [rows], the ticked one (else the first) focused. */
@Composable
internal fun TrackMenuList(menu: TrackMenu, rows: List<MenuRow>) {
    val focusRow = rows.indexOfFirst { it.ticked }.coerceAtLeast(0)
    // Focus again when the rows change (an online search answering, say).
    val focus = rememberInitialFocus(menu to rows.map { it.name })
    Column(
        Modifier
            .width(560.dp)
            .background(Color(0xE610141A), RoundedCornerShape(10.dp))
            .padding(12.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(menu.title, color = Secondary, fontSize = 15.sp)
        LazyColumn(
            state = rememberLazyListState(initialFirstVisibleItemIndex = focusRow),
            modifier = Modifier.heightIn(max = 300.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
            // Room for a focused choice to grow.
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
        ) {
            itemsIndexed(rows) { i, row ->
                Choice(row.name, row.ticked, Modifier.focusIfRow(i == focusRow, focus), row.onClick)
            }
        }
    }
}

private fun Modifier.focusIfRow(isRow: Boolean, focus: FocusRequester): Modifier =
    if (isRow) focusRequester(focus) else this

@Composable
private fun Choice(name: String, ticked: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Button(onClick = onClick, modifier = modifier.fillMaxWidth()) {
        Text(if (ticked) "✓ $name" else name, fontSize = 15.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
    }
}
