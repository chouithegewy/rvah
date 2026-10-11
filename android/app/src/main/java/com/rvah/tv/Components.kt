package com.rvah.tv

import android.util.Log
import uniffi.rvah_android.AddonRecord
import uniffi.rvah_android.AndroidCore

/** Each add-on RVAH Discover nests, and the secrets it takes over from it. */
private val ABSORBED = mapOf(
    TRAKT_ADDON to listOf("client_id", "access_token", "refresh_token"),
    TMDB_ADDON to listOf("tmdb_key", "mdblist_key"),
)

/** The nested add-ons still installed beside an enabled RVAH Discover, with their secrets. */
fun componentsToAbsorb(addons: List<AddonRecord>): List<Pair<String, List<String>>> =
    if (addons.none { it.id == DISCOVER_ADDON && it.status == "enabled" }) {
        emptyList()
    } else {
        ABSORBED.filterKeys { id -> addons.any { it.id == id } }.toList()
    }

/**
 * RVAH Discover takes over the sign-ins and keys of the add-ons it nests
 * (it does what they do), then they are uninstalled, so nothing shows or
 * scrobbles twice. True when any went.
 */
fun absorbComponents(core: AndroidCore, addons: List<AddonRecord>): Boolean {
    var changed = false
    for ((component, secrets) in componentsToAbsorb(addons)) {
        runCatching {
            val adopted = core.adoptSecrets(component, DISCOVER_ADDON, secrets)
            core.uninstallAddon(component)
            Log.i("rvah", "RVAH Discover took over $component (${adopted.joinToString().ifEmpty { "nothing to adopt" }}) and removed it")
            changed = true
        }.onFailure { Log.w("rvah", "could not fold $component into RVAH Discover: ${it.userMessage()}") }
    }
    return changed
}
