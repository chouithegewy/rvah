package com.rvah.tv

/** "2026 · 1 h 31 min · Comedy, Animation", leaving out what is unknown. */
fun detailsLine(year: UShort?, runtimeSecs: UInt?, genres: List<String>): String {
    val runtime = runtimeSecs?.let { secs ->
        val minutes = (secs / 60u).toInt()
        when {
            minutes < 60 -> "$minutes min"
            minutes % 60 == 0 -> "${minutes / 60} h"
            else -> "${minutes / 60} h ${minutes % 60} min"
        }
    }
    val genreList = genres.takeIf { it.isNotEmpty() }?.joinToString(", ")
    return listOfNotNull(year?.toString(), runtime, genreList).joinToString(" · ")
}

/** How a rating's source is named on screen. */
fun ratingLabel(source: String): String = when (source) {
    "rotten_tomatoes" -> "Rotten Tomatoes"
    "rt_audience" -> "RT Audience"
    "imdb" -> "IMDb"
    "metacritic" -> "Metacritic"
    "letterboxd" -> "Letterboxd"
    "tmdb" -> "TMDB"
    else -> source
}

/** What "Find in …" searches for: the title's words, and its year when known. */
fun findQuery(title: String, year: UShort?): String =
    listOfNotNull(title, year?.toString()).joinToString(" ")
