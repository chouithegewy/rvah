package com.rvah.tv

import java.util.Locale

/** "2160p", "1080p", "720p" or "480p" from a release name, or null. */
fun resolution(name: String): String? {
    val words = name.lowercase(Locale.ROOT).split(Regex("[^a-z0-9]+")).toSet()
    return when {
        words.any { it in setOf("2160p", "4k", "uhd") } -> "2160p"
        words.any { it in setOf("1080p", "1080i") } -> "1080p"
        "720p" in words -> "720p"
        words.any { it in setOf("480p", "576p") } -> "480p"
        else -> null
    }
}

/** "2160p · 23.4 GB · 512 seeders · Example", leaving out what is unknown. */
fun releaseLine(name: String, sizeBytes: ULong?, seeders: UInt?, origin: String?): String {
    val size = sizeBytes?.toDouble()?.let { bytes ->
        if (bytes >= 1e9) String.format(Locale.ROOT, "%.1f GB", bytes / 1e9) else String.format(Locale.ROOT, "%.0f MB", bytes / 1e6)
    }
    val seeds = seeders?.let { if (it == 1u) "1 seeder" else "$it seeders" }
    return listOfNotNull(resolution(name), size, seeds, origin?.takeIf { it.isNotBlank() }).joinToString(" · ")
}

/** "Acme Corp · S01E02" for an episode, else the title. */
fun sourcesTitle(title: String, seriesTitle: String?, season: UInt?, episode: UInt?): String =
    if (seriesTitle != null && season != null && episode != null) {
        String.format(Locale.ROOT, "%s · S%02dE%02d", seriesTitle, season.toInt(), episode.toInt())
    } else {
        title
    }
