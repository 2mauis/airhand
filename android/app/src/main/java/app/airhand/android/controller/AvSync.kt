package app.airhand.android.controller

import kotlin.math.abs
import kotlin.math.roundToInt

/** 音频 / 视频共用的采集时钟映射；本地时间统一用 System.nanoTime（与 AudioTimestamp 相同）。 */
class AvSync {
    private data class VideoSample(val atNs: Long, val offsetNs: Long)

    @Volatile var enabled = false
        private set
    @Volatile var extraMs = 0
        private set
    private var audioNs: Long? = null
    private var audioBaseNs: Long? = null
    private var audioAtNs = Long.MIN_VALUE
    private val video = ArrayDeque<VideoSample>()
    private var lastUpdateNs = Long.MIN_VALUE
    private var generation = 0L

    /** 每次启停声音换一代，已经退出的播放器不能污染新会话的时钟。 */
    @Synchronized
    fun reset(): Long {
        generation++
        audioNs = null
        audioBaseNs = null
        audioAtNs = Long.MIN_VALUE
        video.clear()
        extraMs = 0
        lastUpdateNs = Long.MIN_VALUE
        return generation
    }

    @Synchronized
    fun setEnabled(value: Boolean) {
        if (enabled == value) return
        enabled = value
        extraMs = 0
        video.clear()
        lastUpdateNs = Long.MIN_VALUE
    }

    @Synchronized
    fun onAudioClock(token: Long, hostUs: Long, localNs: Long, appliedMs: Double, nowNs: Long) {
        if (token != generation) return
        val off = localNs - hostUs * 1000
        val old = audioNs
        audioNs = if (old == null || abs(off - old) > 40 * MS) off else old + (off - old) / 10
        audioBaseNs = audioNs!! - (appliedMs * MS).toLong()
        audioAtNs = nowNs
    }

    @Synchronized
    fun onVideo(hostUs: Long, arrivalNs: Long, decodeNs: Long): Long {
        video.addLast(VideoSample(arrivalNs, arrivalNs + decodeNs + DISPLAY_NS - hostUs * 1000))
        while (video.isNotEmpty() && (video.first().atNs < arrivalNs - 3_000 * MS || video.size > 600)) video.removeFirst()
        update(arrivalNs)
        return decodeTimeNs(hostUs, arrivalNs, decodeNs)
    }

    @Synchronized
    fun decodeTimeNs(hostUs: Long, nowNs: Long, decodeNs: Long): Long {
        val audio = currentAudio(nowNs) ?: return nowNs
        return (hostUs * 1000 + audio - decodeNs - DISPLAY_NS).coerceIn(nowNs, nowNs + MAX_VIDEO_DELAY_NS)
    }

    /** 解码前承担大部分等待；Surface 只承担剩余的少量延迟，避免耗尽解码器输出缓冲。 */
    @Synchronized
    fun renderTimeNs(hostUs: Long, nowNs: Long): Long {
        val audio = currentAudio(nowNs) ?: return nowNs
        return (hostUs * 1000 + audio).coerceIn(nowNs, nowNs + 50 * MS)
    }

    private fun currentAudio(nowNs: Long): Long? =
        if (enabled && audioNs != null && nowNs - audioAtNs in 0..500 * MS) audioNs else null

    private fun update(nowNs: Long) {
        val base = audioBaseNs
        if (currentAudio(nowNs) == null || base == null) {
            extraMs = 0
            return
        }
        if (video.size < 10 || (lastUpdateNs != Long.MIN_VALUE && nowNs - lastUpdateNs < 2_000 * MS)) return
        val offsets = video.map { it.offsetNs }.sorted()
        val want = ((offsets[(offsets.size * 0.9).toInt()] - base).toDouble() / MS).coerceIn(0.0, 300.0)
        val next = when {
            want > extraMs + 15 -> (want + 5).roundToInt().coerceAtMost(300)
            want < extraMs - 30 -> if (want > 0) (want + 5).roundToInt().coerceAtMost(300) else 0
            else -> extraMs
        }
        if (next != extraMs) {
            extraMs = next
            lastUpdateNs = nowNs
        }
    }

    companion object {
        private const val MS = 1_000_000L
        private const val DISPLAY_NS = 8 * MS
        private const val MAX_VIDEO_DELAY_NS = 500 * MS
    }
}
