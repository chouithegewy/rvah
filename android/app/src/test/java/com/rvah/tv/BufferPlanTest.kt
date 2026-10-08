package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BufferPlanTest {
    private val mib = 1024L * 1024
    private val gib = 1024 * mib

    @Test fun largeHeapTvGetsTheFullBudget() {
        // Typical 2 GB TV with largeHeap: 512 MB heap.
        val plan = bufferPlan(maxHeapBytes = 512 * mib, totalRamBytes = 2 * gib, lowRamDevice = false)
        assertEquals(256 * mib, plan.targetBytes.toLong())
        assertEquals(120_000, plan.maxBufferMs)
    }

    @Test fun budgetIsHalfTheHeapAtMost() {
        val plan = bufferPlan(maxHeapBytes = 192 * mib, totalRamBytes = 2 * gib, lowRamDevice = false)
        assertEquals(96 * mib, plan.targetBytes.toLong())
    }

    @Test fun budgetIsAnEighthOfRamAtMost() {
        val plan = bufferPlan(maxHeapBytes = 512 * mib, totalRamBytes = 3 * gib / 2, lowRamDevice = false)
        assertEquals(192 * mib, plan.targetBytes.toLong())
    }

    @Test fun lowRamDevicesStaySmall() {
        val plan = bufferPlan(maxHeapBytes = 256 * mib, totalRamBytes = 1 * gib, lowRamDevice = true)
        assertEquals(64 * mib, plan.targetBytes.toLong())
    }

    @Test fun durationsSatisfyDefaultLoadControl() {
        // DefaultLoadControl.Builder rejects anything else.
        for (heap in listOf(64 * mib, 192 * mib, 512 * mib, 2 * gib)) {
            for (lowRam in listOf(true, false)) {
                val plan = bufferPlan(heap, 4 * gib, lowRam)
                assertTrue(plan.startMs <= plan.minBufferMs)
                assertTrue(plan.restartMs <= plan.minBufferMs)
                assertTrue(plan.minBufferMs <= plan.maxBufferMs)
                assertTrue(plan.targetBytes in (16 * mib).toInt()..(256 * mib).toInt())
            }
        }
    }

    @Test fun debugLineShowsSecondsAheadAndBudget() {
        val plan = bufferPlan(512 * mib, 2 * gib, lowRamDevice = false)
        assertEquals("Buffered 42 s ahead · budget 256 MiB", bufferLine(42_400, plan))
    }
}
