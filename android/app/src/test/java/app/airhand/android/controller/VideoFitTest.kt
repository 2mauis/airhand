package app.airhand.android.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VideoFitTest {
    /** 常见的 4K 电视解码器：只认横着的 4096×2304 */
    private val landscape4k = { w: Int, h: Int -> w <= 4096 && h <= 2304 }

    /** 只到 1080p 的解码器：按宏块数算（1920×1088 = 8160 个） */
    private val blocks1080p = { w: Int, h: Int -> w <= 4096 && h <= 4096 && ((w + 15) / 16) * ((h + 15) / 16) <= 8160 }

    @Test
    fun keepsTheRequestWhenItFits() {
        assertEquals(1080, VideoFit.maxShort(1080, 2400, 1080) { _, _ -> true })
        assertEquals(0, VideoFit.maxShort(1080, 2400, 0) { _, _ -> true })
        assertEquals(720, VideoFit.maxShort(1080, 2400, 720, blocks1080p))
        // 横屏的 1080p 桌面正好放得下（编出来是 1920×1080，按宏块对齐是 1920×1088）
        assertEquals(1080, VideoFit.maxShort(1920, 1080, 1080) { w, h -> w <= 1920 && h <= 1088 })
        assertEquals(0, VideoFit.maxShort(2400, 1080, 0, landscape4k))
    }

    @Test
    fun shrinksAPortraitPhoneUntilTheDecoderTakesIt() {
        val s = VideoFit.maxShort(1080, 2400, 1080, landscape4k)
        assertTrue("got $s", s in 1000 until 1080)
        VideoFit.sizes(1080, 2400, s).forEach { (w, h) -> assertTrue("${w}x$h", landscape4k(w, h)) }

        // 原样（0 = 不限）和更高的画质也一样
        assertEquals(s, VideoFit.maxShort(1080, 2400, 0, landscape4k))
        assertEquals(s, VideoFit.maxShort(1080, 2400, 1440, landscape4k))

        val b = VideoFit.maxShort(1080, 2400, 1080, blocks1080p)
        assertTrue("got $b", b in 720 until 1080)
        VideoFit.sizes(1080, 2400, b).forEach { (w, h) -> assertTrue("${w}x$h", blocks1080p(w, h)) }
        // 尽量大：再大一档就放不下了
        assertTrue(VideoFit.sizes(1080, 2400, b + 8).any { (w, h) -> !blocks1080p(w, h) })
    }

    @Test
    fun shrinksA4kDesktopForA1080pDecoder() {
        val s = VideoFit.maxShort(3840, 2160, 1440, blocks1080p)
        assertTrue("got $s", s in 1000..1080)
        VideoFit.sizes(3840, 2160, s).forEach { (w, h) -> assertTrue("${w}x$h", blocks1080p(w, h)) }
    }

    @Test
    fun givesUpWhenNothingReasonableFits() {
        assertEquals(1080, VideoFit.maxShort(1080, 2400, 1080) { _, _ -> false })
        assertEquals(0, VideoFit.maxShort(0, 0, 0) { _, _ -> false })
    }

    @Test
    fun sizesMatchBothHosts() {
        // Android 被控端 16 对齐，桌面被控端取偶数
        assertEquals(listOf(1088 to 2400, 1080 to 2400), VideoFit.sizes(1080, 2400, 0))
        assertEquals(listOf(720 to 1600, 720 to 1600), VideoFit.sizes(1080, 2400, 720))
        assertEquals(listOf(1920 to 1088, 1920 to 1080), VideoFit.sizes(3840, 2160, 1080))
    }

    @Test
    fun stricter() {
        assertEquals(720, VideoFit.stricter(0, 720))
        assertEquals(720, VideoFit.stricter(720, 0))
        assertEquals(720, VideoFit.stricter(1080, 720))
        assertEquals(0, VideoFit.stricter(0, 0))
    }
}
