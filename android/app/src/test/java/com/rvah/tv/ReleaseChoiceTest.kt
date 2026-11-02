package com.rvah.tv

import java.lang.reflect.Proxy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import uniffi.rvah_android.AndroidCoreException
import uniffi.rvah_android.AndroidCoreInterface
import uniffi.rvah_android.NativePreparedPlayback
import uniffi.rvah_android.NativeProtocol
import uniffi.rvah_android.ReleaseRecord
import uniffi.rvah_android.SourceQuery

class ReleaseChoiceTest {
    private fun release(n: Int) = ReleaseRecord("Film.$n", n.toString().padStart(40, '0'), null, null, null)

    private val found = (1..8).map(::release)

    private fun state(refused: Map<String, String> = emptyMap()) =
        SourcesState(SourcesTarget("tmdb", "movie:1", "Film", SourceQuery(null, "Film", null, null, null)))
            .apply { show(found, refused) }

    private fun pick(state: SourcesState, n: Int) = ReleasePick("rd", release(n), state.target)

    private val playback = NativePreparedPlayback(
        1u, "https://cdn.example/film.mp4", NativeProtocol.PROGRESSIVE, emptyList(), 0u, emptyList(), "Film", null, emptyList(),
    )

    /** A core whose prepareRelease answers per release; anything else fails the test. */
    private fun core(answer: (ReleaseRecord) -> NativePreparedPlayback): Pair<AndroidCoreInterface, MutableList<String>> {
        val asked = mutableListOf<String>()
        val nothingElse = Proxy.newProxyInstance(javaClass.classLoader, arrayOf(AndroidCoreInterface::class.java)) { _, method, _ ->
            error("unexpected call ${method.name}")
        } as AndroidCoreInterface
        val core = object : AndroidCoreInterface by nothingElse {
            override fun prepareRelease(
                debridId: String,
                release: ReleaseRecord,
                season: UInt?,
                episode: UInt?,
                titleAddonId: String,
                titleItemId: String,
                title: String?,
            ): NativePreparedPlayback {
                asked += release.name
                return answer(release)
            }
        }
        return core to asked
    }

    private fun unavailable(why: String) = AndroidCoreException.Unavailable(why, false)

    @Test fun refusedReleasesAreListedLastInTheirOrder() {
        val state = state(mapOf(release(2).infoHash to "blocked", release(5).infoHash to "blocked"))
        assertEquals(listOf(1, 3, 4, 6, 7, 8, 2, 5).map { "Film.$it" }, state.releases!!.map { it.name })
    }

    @Test fun aChoiceTriesTheReleasesAfterItThatHaveNotFailedUpToFive() {
        val state = state(mapOf(release(4).infoHash to "blocked"))
        state.tried[release(3).infoHash] = "not cached"
        assertEquals(
            listOf(2, 5, 6, 7, 8).map { "Film.$it" },
            state.candidates(release(2)).map { it.name },
        )
        // A release chosen on purpose is tried, refused before or not.
        assertEquals("Film.4", state.candidates(release(4)).first().name)
    }

    @Test fun theFirstReleaseThatPlaysWinsAndEachFailureIsMarked() {
        val state = state()
        val (core, asked) = core { release ->
            if (release.name == "Film.3") playback else throw unavailable("${release.name} not cached")
        }
        val progress = mutableListOf<String>()
        val prepared = prepareFirstPlayable(core, pick(state, 1), state, "Film") { progress += it }

        assertSame(playback, prepared)
        assertEquals(listOf("Film.1", "Film.2", "Film.3"), asked)
        assertEquals(mapOf(release(1).infoHash to "Film.1 not cached", release(2).infoHash to "Film.2 not cached"), state.tried.toMap())
        assertEquals("Film.3", state.playing?.name)
        assertEquals(2, state.focusIndex)
        assertEquals(2, progress.size)
        assertTrue(progress.last(), progress.last().contains("3 of 5"))
    }

    @Test fun whenNoneOfTheTriesPlaysTheLastFailureIsThrown() {
        val state = state()
        val (core, asked) = core { release -> throw unavailable("${release.name} not cached") }
        try {
            prepareFirstPlayable(core, pick(state, 4), state, "Film") {}
            fail("expected Unavailable")
        } catch (e: AndroidCoreException.Unavailable) {
            assertEquals("Film.8 not cached", e.reason)
        }
        assertEquals(listOf(4, 5, 6, 7, 8).map { "Film.$it" }, asked)
        assertEquals(5, state.tried.size)
        assertEquals(7, state.focusIndex)
    }

    @Test fun otherFailuresStopAtOnce() {
        val state = state()
        val (core, asked) = core { throw AndroidCoreException.Playback("network down") }
        try {
            prepareFirstPlayable(core, pick(state, 1), state, "Film") {}
            fail("expected Playback")
        } catch (_: AndroidCoreException.Playback) {
        }
        assertEquals(listOf("Film.1"), asked)
        assertTrue(state.tried.isEmpty())
    }
}
