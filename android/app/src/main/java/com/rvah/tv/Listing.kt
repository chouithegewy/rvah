package com.rvah.tv

/**
 * What a catalog or search screen has loaded so far: the entries, the cursor
 * for the page after them (FR-024), and the row D-pad focus belongs on.
 * Immutable; every transition returns a new value.
 */
data class Listing<T>(
    val entries: List<T>,
    val nextCursor: String?,
    val loadingMore: Boolean = false,
    val moreError: String? = null,
    /** Row to focus when shown; `entries.size` is the trailing "Load more" row. */
    val focusIndex: Int = 0,
) {
    val hasMore: Boolean get() = nextCursor != null

    /** Cursor to request next, or null when on the last page or a request is in flight. */
    fun cursorToLoad(): String? = nextCursor.takeUnless { loadingMore }

    fun startLoadingMore(): Listing<T> = copy(loadingMore = true, moreError = null)

    /**
     * Append the page fetched with [requested]. Focus moves to the page's first
     * entry so the D-pad carries on downward. A result for anything but the
     * request in flight (the listing was reloaded meanwhile) is ignored.
     */
    fun appended(requested: String, page: List<T>, cursor: String?): Listing<T> {
        if (!loadingMore || nextCursor != requested) return this
        return copy(
            entries = entries + page,
            nextCursor = cursor,
            loadingMore = false,
            moreError = null,
            focusIndex = when {
                page.isNotEmpty() || cursor != null -> entries.size
                else -> (entries.size - 1).coerceAtLeast(0)
            },
        )
    }

    /** Keep the cursor so the "Load more" row can retry; focus stays on it. */
    fun failedMore(requested: String, message: String): Listing<T> {
        if (!loadingMore || nextCursor != requested) return this
        return copy(loadingMore = false, moreError = message, focusIndex = entries.size)
    }

    /** Remember the opened row so returning to this listing focuses it again. */
    fun opened(index: Int): Listing<T> = copy(focusIndex = index)
}
