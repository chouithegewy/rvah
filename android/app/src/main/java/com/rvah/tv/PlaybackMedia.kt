package com.rvah.tv

import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.MimeTypes
import uniffi.rvah_android.NativePreparedPlayback
import uniffi.rvah_android.NativeProtocol
import uniffi.rvah_android.NativeSubtitle
import uniffi.rvah_android.NativeSubtitleFormat

data class PlaybackSpec(
    val sessionId: ULong,
    val url: String,
    val protocol: StreamProtocol,
    val headers: Map<String, String>,
    val resumePositionMs: Long,
    val subtitles: List<SubtitleSpec>,
    val title: String,
)

enum class StreamProtocol { Progressive, Hls, Dash }

data class SubtitleSpec(val url: String, val language: String, val label: String?, val mimeType: String)

fun NativePreparedPlayback.toSpec() = PlaybackSpec(
    sessionId = sessionId,
    url = url,
    protocol = when (protocol) {
        NativeProtocol.PROGRESSIVE -> StreamProtocol.Progressive
        NativeProtocol.HLS -> StreamProtocol.Hls
        NativeProtocol.DASH -> StreamProtocol.Dash
    },
    headers = headers.associate { it.name to it.value },
    resumePositionMs = resumePositionMs.toLong(),
    subtitles = subtitles.map { it.toSubtitleSpec() },
    title = title,
)

fun NativeSubtitle.toSubtitleSpec() = SubtitleSpec(
    url,
    language,
    label,
    when (format) {
        NativeSubtitleFormat.WEB_VTT -> MimeTypes.TEXT_VTT
        NativeSubtitleFormat.SUB_RIP -> MimeTypes.APPLICATION_SUBRIP
        NativeSubtitleFormat.TTML -> MimeTypes.APPLICATION_TTML
        NativeSubtitleFormat.SSA, NativeSubtitleFormat.ASS -> MimeTypes.TEXT_SSA
        NativeSubtitleFormat.UNKNOWN -> inferSubtitleMimeType(url)
    },
)

fun playbackMimeType(protocol: StreamProtocol, url: String): String = when (protocol) {
    StreamProtocol.Hls -> MimeTypes.APPLICATION_M3U8
    StreamProtocol.Dash -> MimeTypes.APPLICATION_MPD
    StreamProtocol.Progressive -> when (url.substringBefore('?').substringAfterLast('.', "").lowercase()) {
        "webm" -> MimeTypes.VIDEO_WEBM
        "mkv" -> MimeTypes.VIDEO_MATROSKA
        else -> MimeTypes.VIDEO_MP4
    }
}

fun inferSubtitleMimeType(url: String): String = when (
    url.substringBefore('?').substringAfterLast('.', "").lowercase()
) {
    "vtt" -> MimeTypes.TEXT_VTT
    "ttml", "dfxp", "xml" -> MimeTypes.APPLICATION_TTML
    "ssa", "ass" -> MimeTypes.TEXT_SSA
    else -> MimeTypes.APPLICATION_SUBRIP
}

fun PlaybackSpec.toMediaItem(preferredSubtitleUrl: String? = null): MediaItem {
    val subtitles = subtitles.map {
        val builder = MediaItem.SubtitleConfiguration.Builder(android.net.Uri.parse(it.url))
            // The track's format carries the URL, so a file added later can be found and picked.
            .setId(it.url)
            .setMimeType(it.mimeType)
            .setLanguage(it.language)
            .setLabel(it.label)
        builder.setSelectionFlags(subtitleSelectionFlags(it.url, preferredSubtitleUrl))
        builder.build()
    }
    return MediaItem.Builder()
        .setUri(url)
        .setMimeType(playbackMimeType(protocol, url))
        .setSubtitleConfigurations(subtitles)
        .setMediaMetadata(MediaMetadata.Builder().setTitle(title).build())
        .build()
}

fun subtitleSelectionFlags(url: String, preferredSubtitleUrl: String?): Int =
    if (url == preferredSubtitleUrl) C.SELECTION_FLAG_DEFAULT else 0

/** Media3 may rewrite an external track id, so retain a language/label fallback. */
fun subtitleTrackMatches(
    id: String?,
    language: String?,
    label: String?,
    wanted: SubtitleSpec,
): Boolean = id == wanted.url || (
    wanted.label != null &&
    language.equals(wanted.language, ignoreCase = true) &&
        label.equals(wanted.label, ignoreCase = true)
)
