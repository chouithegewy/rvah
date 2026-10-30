package com.rvah.tv

import android.content.Context
import android.util.Log
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.rvah_android.AdBlocker

/**
 * The web player's ad blocker, built from EasyList and EasyPrivacy. The lists
 * are downloaded into the app's files (never bundled), used for a week, then
 * fetched again; a failed download keeps the copy already there. Without any
 * list there is no blocker, and the player still locks its page down.
 */
internal object FilterLists {
    const val EASYLIST = "https://easylist.to/easylist/easylist.txt"
    const val EASYPRIVACY = "https://easylist.to/easylist/easyprivacy.txt"

    private val lists = listOf("easylist.txt" to EASYLIST, "easyprivacy.txt" to EASYPRIVACY)
    private val lock = Mutex()
    /** Builds outlive any one caller, so a caller's timeout only stops its wait. */
    private val work = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var building: Deferred<AdBlocker?>? = null

    /**
     * The blocker, building it (and fetching due lists) the first time. A
     * build that found no lists is tried again on the next call. Waiting is
     * cancellable: a caller's timeout ends its wait, not the build.
     */
    suspend fun blocker(context: Context): AdBlocker? = lock.withLock {
        val current = building
        if (current != null && !(current.isCompleted && current.await() == null)) {
            current
        } else {
            work.async { load(context.applicationContext) }.also { building = it }
        }
    }.await()

    /** Fetch and build in the background, so the first web video need not wait. */
    fun warmUp(context: Context, scope: CoroutineScope) {
        scope.launch { blocker(context) }
    }

    private fun load(context: Context): AdBlocker? {
        val dir = File(context.filesDir, "filters").apply { mkdirs() }
        val texts = lists.mapNotNull { (name, url) ->
            val file = File(dir, name)
            if (listsStale(file.lastModified(), System.currentTimeMillis())) fetch(url, file)
            file.takeIf { it.isFile && it.length() > 0 }?.readText()
        }
        if (texts.isEmpty()) return null
        return AdBlocker(texts).also { Log.i("rvah", "ad blocker: ${it.ruleCount()} rules") }
    }

    /** Download [url] over [file], via a temporary file so a cut-off download never replaces a good list. */
    private fun fetch(url: String, file: File) {
        runCatching {
            val connection = URL(url).openConnection() as HttpURLConnection
            connection.connectTimeout = 15_000
            connection.readTimeout = 15_000
            try {
                check(connection.responseCode == 200) { "HTTP ${connection.responseCode}" }
                val partial = File(file.parentFile, file.name + ".part")
                connection.inputStream.use { input -> partial.outputStream().use { input.copyTo(it) } }
                check(partial.renameTo(file)) { "could not replace ${file.name}" }
            } finally {
                connection.disconnect()
            }
        }.onFailure { Log.w("rvah", "filter list $url: ${it.message}") }
    }
}
