package com.rvah.tv

import java.util.Locale

/** What to type on the phone: "https://real-debrid.com/device/" → "real-debrid.com/device". */
fun displayUrl(url: String): String = url.removePrefix("https://").removePrefix("http://").trimEnd('/')

/** Time left on a sign-in code, e.g. "29:05"; "0:00" once it has passed. */
fun formatRemaining(seconds: Long): String {
    val left = seconds.coerceAtLeast(0)
    return "%d:%02d".format(Locale.ROOT, left / 60, left % 60)
}
