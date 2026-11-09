package app.airhand.android.controller

import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * 让对方发来的画面尺寸落在本机解码器的能力范围内。
 *
 * 电视的硬件解码器常常只到 1080p（约 8160 个宏块），或者只到 4096×2304 这类横着的尺寸，
 * 竖屏手机原样的 1080×2400 放不下：解码器起不来或者解不出画面，一直黑屏（最低画质的 720×1600 就没事）。
 * 被控端只认"短边上限"，所以这里算一个放得下的短边上限发过去。
 */
object VideoFit {
    /** 缩到这么小还放不下，多半是解码器报的能力不对：不再缩，照原样 */
    private const val MIN_SHORT = 360
    private const val STEP = 8

    /**
     * 对方屏幕 `srcW`×`srcH`、想要的短边上限 `maxShort`（0 = 不限）：放得下就原样返回，
     * 否则返回放得下的最大短边上限。`fits` 判断解码器支不支持某个尺寸。
     */
    fun maxShort(srcW: Int, srcH: Int, maxShort: Int, fits: (Int, Int) -> Boolean): Int {
        val short = min(srcW, srcH)
        if (short <= 0) return maxShort
        fun ok(s: Int) = sizes(srcW, srcH, s).all { (w, h) -> fits(w, h) }
        val start = if (maxShort in 1 until short) maxShort else short
        if (ok(start)) return maxShort
        var s = (start - 1) / STEP * STEP
        while (s >= MIN_SHORT) {
            if (ok(s)) return s
            s -= STEP
        }
        return maxShort
    }

    /**
     * 短边上限为 `s` 时被控端编出来的尺寸，两种被控端都算：
     * Android（`ScreenCapture.encodeSize`，16 对齐）和桌面（`encoder::target_size`，偶数）。
     */
    fun sizes(srcW: Int, srcH: Int, s: Int): List<Pair<Int, Int>> {
        val short = min(srcW, srcH)
        val scale = if (s in 1 until short) s.toDouble() / short else 1.0
        fun align16(v: Double) = max(16, (v / 16).roundToInt() * 16)
        fun even(v: Double) = max(64, v.roundToInt() and 1.inv())
        return listOf(
            align16(srcW * scale) to align16(srcH * scale),
            even(srcW * scale) to even(srcH * scale),
        )
    }

    /** 短边上限取较严的那个（0 = 不限） */
    fun stricter(a: Int, b: Int): Int = when {
        a == 0 -> b
        b == 0 -> a
        else -> min(a, b)
    }
}
