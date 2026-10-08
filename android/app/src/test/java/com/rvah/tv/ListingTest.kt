package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ListingTest {
    private val first = Listing(listOf("a", "b"), nextCursor = "p2")

    @Test fun loadMoreAppendsAndFocusesTheFirstNewEntry() {
        val loading = first.startLoadingMore()
        assertTrue(loading.loadingMore)
        assertNull("no second request while one is in flight", loading.cursorToLoad())

        val next = loading.appended("p2", listOf("c", "d"), cursor = "p3")
        assertEquals(listOf("a", "b", "c", "d"), next.entries)
        assertEquals("p3", next.cursorToLoad())
        assertFalse(next.loadingMore)
        assertEquals(2, next.focusIndex)
    }

    @Test fun lastPageDropsTheCursor() {
        val last = first.startLoadingMore().appended("p2", listOf("c"), cursor = null)
        assertFalse(last.hasMore)
        assertNull(last.cursorToLoad())
        assertEquals(2, last.focusIndex)
    }

    @Test fun emptyFinalPageFocusesTheLastEntry() {
        val last = first.startLoadingMore().appended("p2", emptyList(), cursor = null)
        assertEquals(listOf("a", "b"), last.entries)
        assertEquals(1, last.focusIndex)
    }

    @Test fun emptyPageWithACursorKeepsFocusOnLoadMore() {
        val next = first.startLoadingMore().appended("p2", emptyList(), cursor = "p3")
        assertEquals(2, next.focusIndex)
        assertEquals("p3", next.cursorToLoad())
    }

    @Test fun failureKeepsTheCursorForARetry() {
        val failed = first.startLoadingMore().failedMore("p2", "offline")
        assertEquals("offline", failed.moreError)
        assertEquals("p2", failed.cursorToLoad())
        assertEquals(2, failed.focusIndex)
        assertNull(failed.startLoadingMore().moreError)
    }

    @Test fun resultsForAnotherRequestAreIgnored() {
        assertSame("nothing in flight", first, first.appended("p2", listOf("x"), "p9"))
        val loading = first.startLoadingMore()
        assertSame("other cursor", loading, loading.appended("p7", listOf("x"), "p9"))
        assertSame("other cursor", loading, loading.failedMore("p7", "boom"))
    }

    @Test fun openingARowRemembersItForFocus() {
        assertEquals(1, first.opened(1).focusIndex)
    }
}
