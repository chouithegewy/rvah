package com.rvah.tv

/**
 * How much of a stream ExoPlayer reads ahead, in memory. Nothing is cached
 * to disk: Real-Debrid files are large, usually watched once, and quick to
 * fetch again, while TV flash is small and wears out.
 *
 * The byte budget is what matters for high-bitrate files: ExoPlayer stops
 * loading at whichever comes first, [maxBufferMs] or [targetBytes]. The
 * buffer lives on the Java heap, so the budget is a share of the heap
 * (raised by `largeHeap` in the manifest) and of physical RAM.
 *
 * There is no back buffer: ExoPlayer counts it against the same byte budget,
 * so for a 4K remux ten seconds behind the playhead would take about half of
 * the read-ahead. Rewinding re-fetches instead, which costs about a second.
 */
data class BufferPlan(
    val targetBytes: Int,
    val minBufferMs: Int,
    val maxBufferMs: Int,
    /** Buffered media needed to start playing. */
    val startMs: Int,
    /** Buffered media needed to resume after a stall. */
    val restartMs: Int,
)

private const val MIB = 1024L * 1024

fun bufferPlan(maxHeapBytes: Long, totalRamBytes: Long, lowRamDevice: Boolean): BufferPlan {
    val budget = if (lowRamDevice) {
        minOf(maxHeapBytes / 4, totalRamBytes / 16).coerceIn(16 * MIB, 64 * MIB)
    } else {
        minOf(maxHeapBytes / 2, totalRamBytes / 8).coerceIn(16 * MIB, 256 * MIB)
    }
    return BufferPlan(
        targetBytes = budget.toInt(),
        // Equal min and max: top up continuously rather than in bursts.
        minBufferMs = 120_000,
        maxBufferMs = 120_000,
        startMs = 1_000,
        // Longer than ExoPlayer's 2 s, so a connection just below the file's
        // bitrate stalls less often, for a little longer each time.
        restartMs = 5_000,
    )
}

/** Debug builds show this on the player screen. */
fun bufferLine(bufferedAheadMs: Long, plan: BufferPlan): String =
    "Buffered ${bufferedAheadMs / 1000} s ahead · budget ${formatBytes(plan.targetBytes.toLong())}"
