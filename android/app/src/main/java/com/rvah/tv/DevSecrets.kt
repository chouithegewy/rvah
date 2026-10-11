package com.rvah.tv

import android.util.Log
import uniffi.rvah_android.AndroidCore

/** A key from the repository's `.env` that a debug build stores for an add-on. */
data class SeedSecret(val addonId: String, val name: String, val value: String)

const val TMDB_ADDON = "org.themoviedb.discover"
const val PROWLARR_ADDON = "com.rvah.sources.prowlarr"
const val TRAKT_ADDON = "com.rvah.trakt"

/**
 * Which `.env` variable feeds which add-on secret. For the same secret the
 * earlier variable wins: TMDB's read access token before its API key.
 */
private val SEEDS = listOf("com.rvah.discover", TMDB_ADDON).flatMap { addon ->
    listOf(
        Triple("TMDB_READ_ACCESS_TOKEN", addon, "tmdb_key"),
        Triple("TMDB_API_KEY", addon, "tmdb_key"),
        Triple("MDBLIST_API_KEY", addon, "mdblist_key"),
    )
} + listOf("com.rvah.discover", TRAKT_ADDON).map { addon ->
    Triple("TRAKT_CLIENT_ID", addon, "client_id")
} + listOf(
    Triple("PROWLARR_API_KEY", PROWLARR_ADDON, "api_key"),
    // A private token (real-debrid.com/apitoken) instead of Link account.
    Triple("REAL_DEBRID_API_TOKEN", "com.real-debrid.downloads", "api_token"),
)

/** `NAME=value;NAME=value`, as Gradle writes `BuildConfig.DEV_SECRETS`. */
fun parseDevEnv(encoded: String): Map<String, String> =
    encoded.split(';')
        .mapNotNull { pair -> pair.split('=', limit = 2).takeIf { it.size == 2 && it[1].isNotEmpty() } }
        .associate { (name, value) -> name to value }

/** The secrets `env` provides, one per add-on secret. */
fun devSecrets(env: Map<String, String>): List<SeedSecret> =
    SEEDS.mapNotNull { (variable, addonId, name) -> env[variable]?.let { SeedSecret(addonId, name, it) } }
        .distinctBy { it.addonId to it.name }

/**
 * Store each `.env` key whose add-on is installed and whose secret is still
 * unset, so a key typed on the TV is never replaced. Debug builds only; a
 * failure is logged, never thrown.
 */
fun seedDevSecrets(core: AndroidCore, encoded: String = BuildConfig.DEV_SECRETS) {
    if (!BuildConfig.DEBUG) return
    val seeds = devSecrets(parseDevEnv(encoded))
    if (seeds.isEmpty()) return
    runCatching {
        val installed = core.addons().map { it.id }.toSet()
        for ((addonId, group) in seeds.filter { it.addonId in installed }.groupBy { it.addonId }) {
            val unset = core.secretStatus(addonId).filterNot { it.isSet }.map { it.name }.toSet()
            for (seed in group.filter { it.name in unset }) {
                core.setSecret(addonId, seed.name, seed.value)
                Log.d("rvah", "stored ${seed.name} for $addonId from .env")
            }
        }
    }.onFailure { Log.w("rvah", "seeding .env keys failed: ${it.userMessage()}") }
}
