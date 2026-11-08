package app.airhand.android.controller

import org.junit.Assert.*
import org.junit.Test

class AvSyncTest {
    private val ms = 1_000_000L

    @Test
    fun mapsRemoteClockAndAccountsForDecodeAndDisplayLatency() {
        val sync = AvSync()
        val token = sync.reset()
        sync.setEnabled(true)
        // 主机 10 s 的声音在本机 5 s 输出（两端时钟无需对时）。
        sync.onAudioClock(token, 10_000_000, 5000 * ms, 0.0, 4900 * ms)
        assertEquals(4982 * ms, sync.onVideo(10_000_000, 4900 * ms, 10 * ms))
        assertEquals(5000 * ms, sync.renderTimeNs(10_000_000, 4992 * ms))
    }

    @Test
    fun disabledMissingOrStaleAudioNeverDelaysVideo() {
        val sync = AvSync()
        val token = sync.reset()
        assertEquals(100 * ms, sync.onVideo(0, 100 * ms, 0))
        sync.onAudioClock(token, 0, 200 * ms, 0.0, 100 * ms)
        assertEquals(100 * ms, sync.decodeTimeNs(0, 100 * ms, 0))
        sync.setEnabled(true)
        assertEquals(192 * ms, sync.decodeTimeNs(0, 100 * ms, 0))
        assertEquals(700 * ms, sync.decodeTimeNs(1_000_000, 700 * ms, 0))
        sync.setEnabled(false)
        assertEquals(110 * ms, sync.decodeTimeNs(0, 110 * ms, 0))
    }

    @Test
    fun resetRejectsLateClocksFromPreviousPlayer() {
        val sync = AvSync()
        val old = sync.reset()
        sync.setEnabled(true)
        sync.onAudioClock(old, 0, 200 * ms, 0.0, 100 * ms)
        val next = sync.reset()
        sync.onAudioClock(old, 0, 300 * ms, 0.0, 100 * ms)
        assertEquals(100 * ms, sync.decodeTimeNs(0, 100 * ms, 0))
        sync.onAudioClock(next, 0, 400 * ms, 0.0, 100 * ms)
        assertEquals(392 * ms, sync.decodeTimeNs(0, 100 * ms, 0))
    }

    @Test
    fun videoAndSurfaceWaitsAreBounded() {
        val sync = AvSync()
        val token = sync.reset()
        sync.setEnabled(true)
        sync.onAudioClock(token, 0, 10_000 * ms, 0.0, 100 * ms)
        assertEquals(600 * ms, sync.decodeTimeNs(0, 100 * ms, 0))
        assertEquals(150 * ms, sync.renderTimeNs(0, 100 * ms))
    }

    @Test
    fun consistentlySlowVideoAddsBoundedAudioDelayAndCanRecover() {
        val sync = AvSync()
        val token = sync.reset()
        sync.setEnabled(true)
        fun sample(hostMs: Long, videoLag: Long, applied: Double) {
            val now = (hostMs + videoLag) * ms
            sync.onAudioClock(token, hostMs * 1000, (hostMs + 100) * ms + (applied * ms).toLong(), applied, now)
            sync.onVideo(hostMs * 1000, now, 0)
        }
        repeat(20) { sample(it * 20L, 250, 0.0) }
        assertEquals(163, sync.extraMs)
        // 已生效的声音延迟不会被重复累加。
        repeat(150) { sample(400 + it * 20L, 250, 163.0) }
        assertEquals(163, sync.extraMs)
        repeat(300) { sample(3400 + it * 20L, 50, 163.0) }
        assertEquals(0, sync.extraMs)
        repeat(300) { sample(9400 + it * 20L, 1000, 0.0) }
        assertEquals(300, sync.extraMs)
        sync.setEnabled(false)
        assertEquals(0, sync.extraMs)
    }
}
