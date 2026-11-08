package app.airhand.android.controller

import org.junit.Assert.*
import org.junit.Test

class AudioBufferTest {
    private fun packet(value: Int = 12_000) = ShortArray(960 * 2) { if (it % 2 == 0) value.toShort() else (-value).toShort() }
    private val block = ShortArray(480 * 2)

    @Test
    fun buffersRealAudioBeforePlayingAndPreservesStereo() {
        val buffer = AudioBuffer()
        buffer.push(packet(), 1920, true, 1_000_000)
        assertNull(buffer.pull(block))
        assertTrue(block.all { it == 0.toShort() })
        buffer.push(packet(), 1920, false, 1_020_000)
        assertNull(buffer.pull(block))
        buffer.push(packet(), 1920, false, 1_040_000)
        assertEquals(1_000_000L, buffer.pull(block)!!.hostUs)
        assertEquals(0, block[0].toInt()) // 渐入
        assertEquals(12_000, block[400].toInt())
        assertEquals(-12_000, block[401].toInt())
    }

    @Test
    fun shortNotificationStillPlaysAndDrainsCompletely() {
        val buffer = AudioBuffer()
        buffer.push(packet(), 1920, true, 50_000)
        assertNull(buffer.pull(block))
        assertEquals(50_000L, buffer.pull(block, finishBurst = true)!!.hostUs)
        assertNotNull(buffer.pull(block, finishBurst = true))
        assertEquals(0.0, buffer.bufferedMs, 0.001)
        assertNull(buffer.pull(block, finishBurst = true))
    }

    @Test
    fun networkStarvationRaisesTargetButSilenceDoesNot() {
        fun afterGap(start: Boolean): AudioBuffer {
            val buffer = AudioBuffer()
            repeat(3) { buffer.push(packet(), 1920, it == 0, it * 20_000L) }
            repeat(10) { buffer.pull(block) }
            buffer.push(packet(), 1920, start, 1_000_000)
            return buffer
        }
        assertEquals(80.0, afterGap(false).targetMs, 0.001)
        assertEquals(1, afterGap(false).underruns)
        assertEquals(60.0, afterGap(true).targetMs, 0.001)
        assertEquals(0, afterGap(true).underruns)
    }

    @Test
    fun burstDropsOldAudioAndClockFollowsRetainedSamples() {
        val buffer = AudioBuffer()
        repeat(25) { buffer.push(packet(), 1920, it == 0, 2_000_000L + it * 20_000) }
        val clock = buffer.pull(block)!!
        assertEquals(1, buffer.skips)
        assertEquals(2_440_000L, clock.hostUs)
        assertTrue(buffer.bufferedMs <= 60)
    }

    @Test
    fun ringWrapRetainsLatestClockAndNeverExceedsCapacity() {
        val buffer = AudioBuffer()
        repeat(200) { buffer.push(packet(), 1920, it == 0, 3_000_000L + it * 20_000) }
        assertTrue(buffer.bufferedMs < 2000)
        assertEquals(6_940_000L, buffer.pull(block)!!.hostUs)
        assertEquals(1, buffer.skips)
    }

    @Test
    fun extraDelayRebuffersAndReportsOnlyAppliedDelay() {
        val buffer = AudioBuffer()
        repeat(3) { buffer.push(packet(), 1920, it == 0, it * 20_000L) }
        buffer.pull(block)
        buffer.setExtraDelay(100)
        assertNull(buffer.pull(block)) // 淡出后补齐
        assertTrue(block[0] > block[block.size - 2])
        assertNull(buffer.pull(block))
        repeat(6) { buffer.push(packet(), 1920, false, 60_000L + it * 20_000) }
        assertEquals(100.0, buffer.pull(block)!!.appliedMs, 0.01)
        buffer.setExtraDelay(0)
        assertEquals(0.0, buffer.pull(block)!!.appliedMs, 0.01)
    }

    @Test
    fun steadyNetworkAndClockDriftStayContinuousForOneMinute() {
        for (intervalMs in listOf(20.0, 19.8, 20.1)) {
            val buffer = AudioBuffer()
            var nextPacket = 0.0
            var seq = 0L
            for (ms in 0 until 60_000 step 10) {
                while (nextPacket <= ms) {
                    buffer.push(packet(), 1920, seq == 0L, seq++ * 20_000)
                    nextPacket += intervalMs
                }
                val clock = buffer.pull(block)
                if (ms >= 100) assertNotNull("clock at $ms ms, packet interval $intervalMs", clock)
                assertTrue(buffer.bufferedMs < 200)
            }
            assertEquals("underruns at $intervalMs", 0, buffer.underruns)
            assertEquals("skips at $intervalMs", 0, buffer.skips)
            assertTrue("target at $intervalMs: ${buffer.targetMs}", buffer.targetMs in 40.0..45.0)
        }
    }

    @Test
    fun partialAndZeroWritesRetryAllSamplesInOrder() {
        val pcm = ShortArray(20) { it.toShort() }
        val received = mutableListOf<Short>()
        val sizes = ArrayDeque(listOf(4, 0, 2, 14))
        val frames = writeAudioFully(pcm, { true }) { offset, remaining ->
            val n = sizes.removeFirst()
            assertTrue(n <= remaining)
            received.addAll(pcm.slice(offset until offset + n))
            n
        }
        assertEquals(10, frames)
        assertEquals(pcm.toList(), received)
    }

    @Test
    fun failedWriteIsReportedAndReleaseCancelsPendingWrite() {
        assertThrows(IllegalStateException::class.java) { writeAudioFully(block, { true }) { _, _ -> -6 } }
        var active = true
        assertEquals(2, writeAudioFully(block, { active }) { _, _ -> active = false; 4 })
    }
}
