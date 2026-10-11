package com.rvah.tv

import java.util.Locale

/**
 * A track's name in the player's Audio and Subtitles menus: its language
 * (as Media3 gives it, normalized to ISO 639-1 where there is a code),
 * then what the file calls it ("SDH", "Commentary") unless that only
 * repeats the language, its channels ("5.1"), and "forced" for a
 * signs-only subtitle track. [channels] is 0 or less when unknown or not
 * audio.
 */
fun trackName(language: String?, label: String?, channels: Int, forced: Boolean): String {
    val languageName = language
        ?.takeIf { it.isNotBlank() && it != "und" }
        ?.let { Locale.forLanguageTag(it).getDisplayLanguage(Locale.ENGLISH).ifBlank { it } }
    val label = label?.trim()?.takeIf { it.isNotEmpty() && !it.equals(languageName, ignoreCase = true) }
    val layout = when {
        channels <= 0 -> null
        channels == 1 -> "mono"
        channels == 2 -> "stereo"
        channels == 6 -> "5.1"
        channels == 8 -> "7.1"
        else -> "$channels ch"
    }
    return listOfNotNull(languageName ?: label ?: "Unknown language", label.takeIf { languageName != null }, layout, "forced".takeIf { forced })
        .joinToString(" · ")
}

/** [names] with a repeated name numbered ("English", "English (2)"), so each choice reads apart. */
fun distinctNames(names: List<String>): List<String> {
    val seen = mutableMapOf<String, Int>()
    return names.map { name ->
        val count = (seen[name] ?: 0) + 1
        seen[name] = count
        if (count == 1) name else "$name ($count)"
    }
}

/** A track's name as a button shows it: the language alone ("English"). */
fun shortTrackName(name: String): String = name.substringBefore(" · ")
