package com.rvah.tv

import androidx.media3.common.TrackSelectionParameters

/** The viewer's language for audio and subtitles. */
const val PREFERRED_LANGUAGE = "en"

/**
 * English audio when the file has it; otherwise the track the file marks as
 * default, which is normally the original language. English subtitles are
 * on by default, full ones ahead of forced (signs-only) ones.
 */
fun TrackSelectionParameters.withLanguageDefaults(): TrackSelectionParameters =
    buildUpon()
        .setPreferredAudioLanguage(PREFERRED_LANGUAGE)
        .setPreferredTextLanguage(PREFERRED_LANGUAGE)
        .build()
