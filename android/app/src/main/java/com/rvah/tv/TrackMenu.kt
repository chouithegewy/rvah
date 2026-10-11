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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Button
import androidx.tv.material3.Text

/** Which of the player's track menus is open. */
internal enum class TrackMenu(val title: String) {
    Audio("Audio"),
    Subtitles("Subtitles"),
}

/** The track a menu's button names: the selected one's language, else Off (subtitles) or a dash. */
internal fun trackButtonLabel(menu: TrackMenu, options: List<TrackOption>): String {
    val chosen = options.firstOrNull { it.selected }?.let { shortTrackName(it.name) }
    return "${menu.title}: ${chosen ?: if (menu == TrackMenu.Subtitles) "Off" else "—"}"
}

/**
 * The file's tracks of one kind, the selected one ticked and focused; the
 * Subtitles menu starts with Off. Picking one calls [onPick] (null: Off).
 */
@Composable
internal fun TrackMenuList(menu: TrackMenu, options: List<TrackOption>, onPick: (TrackOption?) -> Unit) {
    // Rows: Off (subtitles only), then the tracks.
    val offered = menu == TrackMenu.Subtitles
    val selected = options.indexOfFirst { it.selected }
    val focusRow = when {
        selected >= 0 -> selected + if (offered) 1 else 0
        else -> 0
    }
    val focus = rememberInitialFocus(menu)
    Column(
        Modifier
            .width(460.dp)
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
            if (offered) {
                item {
                    Choice("Off", ticked = selected < 0, Modifier.focusIfRow(focusRow == 0, focus)) { onPick(null) }
                }
            }
            itemsIndexed(options) { i, option ->
                val row = i + if (offered) 1 else 0
                Choice(option.name, ticked = option.selected, Modifier.focusIfRow(row == focusRow, focus)) { onPick(option) }
            }
        }
    }
}

private fun Modifier.focusIfRow(isRow: Boolean, focus: FocusRequester): Modifier =
    if (isRow) focusRequester(focus) else this

@Composable
private fun Choice(name: String, ticked: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Button(onClick = onClick, modifier = modifier.fillMaxWidth()) {
        Text(if (ticked) "✓ $name" else name, fontSize = 15.sp)
    }
}
