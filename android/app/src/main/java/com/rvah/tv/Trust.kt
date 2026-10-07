package com.rvah.tv

import java.io.File

/**
 * The build's trusted Ed25519 package-signing keys (`-Prvah.trustedKeys=`):
 * 64 hex characters each, separated by commas or whitespace. Anything else
 * is dropped so a typo cannot widen trust.
 */
fun parseTrustedKeys(raw: String): List<String> =
    raw.split(',', ' ', '\n', '\t')
        .map { it.trim().lowercase() }
        .filter { key -> key.length == 64 && key.all { it in '0'..'9' || it in 'a'..'f' } }
        .distinct()

/** Short, readable fingerprint of a hex key for the consent screen. */
fun keyFingerprint(hex: String): String = hex.take(16).chunked(4).joinToString(" ")

/** `.rvah` packages in [dir] that have a detached `.sig` next to them, newest first. */
fun findPackages(dir: File?): List<File> =
    dir?.listFiles { file -> file.isFile && file.name.endsWith(".rvah") && File(file.path + ".sig").isFile }
        ?.sortedByDescending { it.lastModified() }
        .orEmpty()

fun formatBytes(bytes: Long): String = when {
    bytes >= 1024 * 1024 -> "${bytes / (1024 * 1024)} MiB"
    bytes >= 1024 -> "${bytes / 1024} KiB"
    else -> "$bytes bytes"
}
