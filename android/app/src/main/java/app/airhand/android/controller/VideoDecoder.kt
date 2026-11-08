package app.airhand.android.controller

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import app.airhand.android.core.Codec
import java.nio.ByteBuffer

/** 一帧画面（`airhand_link` 的视频包去掉 24 字节头），`ack` 在送进解码器之后调用 */
class VideoPacket(
    val seq: Int,
    val key: Boolean,
    val codec: Codec,
    val width: Int,
    val height: Int,
    val ptsUs: Long,
    val data: ByteArray,
    val offset: Int,
    val length: Int,
    val ack: () -> Unit,
)

/**
 * 硬件解码（MediaCodec 异步模式），解出来的画面立即渲染到 Surface（TextureView）。
 *
 * * 只有送进解码器之后才确认这一帧：手机解码跟不上时，被控端的在途帧控制会放慢，延迟不会越积越多；
 * * 格式 / 分辨率变了、解码出错、Surface 换了：重建解码器，从下一个关键帧开始（主动向被控端要关键帧）；
 * * 没有 Surface（应用在后台）时收到的帧直接确认、丢掉。
 */
class VideoDecoder(
    private val requestKeyframe: () -> Unit,
    private val onError: (String) -> Unit = {},
) {
    private val thread = HandlerThread("airhand-decode").apply { start() }
    private val handler = Handler(thread.looper)

    private var surface: Surface? = null
    private var codec: MediaCodec? = null
    private var config: Triple<Codec, Int, Int>? = null
    private val pending = ArrayDeque<VideoPacket>()
    private val freeInputs = ArrayDeque<Int>()
    private var needKey = true
    private var lastKeyRequest = 0L
    private var released = false

    fun setSurface(s: Surface?) {
        handler.post {
            if (surface === s) return@post
            surface = s
            releaseCodec()
            needKey = true
            if (s != null) askKeyframe(force = true)
        }
    }

    fun submit(p: VideoPacket) {
        handler.post { onPacket(p) }
    }

    /** 会话断了重连：之前排队的帧属于旧会话，丢掉（不再确认），从关键帧重新开始 */
    fun resetStream() {
        handler.post {
            pending.clear()
            needKey = true
        }
    }

    fun release() {
        handler.post {
            released = true
            pending.forEach { it.ack() }
            pending.clear()
            releaseCodec()
            thread.quitSafely()
        }
    }

    private fun askKeyframe(force: Boolean = false) {
        val now = SystemClock.elapsedRealtime()
        if (force || now - lastKeyRequest > 500) {
            lastKeyRequest = now
            requestKeyframe()
        }
    }

    private fun onPacket(p: VideoPacket) {
        if (released || surface == null) {
            p.ack()
            return
        }
        val c = config
        val changed = c == null || codec == null || c.first != p.codec || (p.key && (c.second != p.width || c.third != p.height))
        if (changed) {
            if (!p.key) {
                p.ack()
                needKey = true
                askKeyframe()
                return
            }
            if (!configure(p)) {
                p.ack()
                return
            }
        }
        if (needKey) {
            if (!p.key) {
                p.ack()
                askKeyframe()
                return
            }
            needKey = false
        }
        pending.addLast(p)
        if (pending.size > MAX_PENDING) {
            // 解码器卡住了：丢掉排队的帧，等下一个关键帧
            Log.w(TAG, "decoder is not keeping up (${pending.size} frames queued), waiting for a keyframe")
            pending.forEach { it.ack() }
            pending.clear()
            needKey = true
            askKeyframe(force = true)
            return
        }
        feed()
    }

    private fun feed() {
        val mc = codec ?: return
        while (pending.isNotEmpty() && freeInputs.isNotEmpty()) {
            val p = pending.removeFirst()
            val idx = freeInputs.removeFirst()
            try {
                val buf: ByteBuffer = mc.getInputBuffer(idx) ?: throw IllegalStateException("no input buffer")
                buf.clear()
                if (buf.capacity() < p.length) {
                    throw IllegalStateException("frame of ${p.length} bytes doesn't fit the ${buf.capacity()} byte input buffer")
                }
                buf.put(p.data, p.offset, p.length)
                mc.queueInputBuffer(idx, 0, p.length, p.ptsUs, if (p.key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
            } catch (e: Exception) {
                Log.w(TAG, "can't queue a frame: $e")
                p.ack()
                fail("queue: $e")
                return
            }
            p.ack()
        }
    }

    private fun fail(why: String) {
        releaseCodec()
        pending.forEach { it.ack() }
        pending.clear()
        needKey = true
        onError(why)
        askKeyframe(force = true)
    }

    private fun configure(p: VideoPacket): Boolean {
        releaseCodec()
        val s = surface ?: return false
        val format = MediaFormat.createVideoFormat(p.codec.mime, p.width, p.height)
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, (p.width * p.height * 3 / 2).coerceAtLeast(p.length * 2))
        csd(p).forEachIndexed { i, b -> format.setByteBuffer("csd-$i", ByteBuffer.wrap(b)) }
        val lowLatency = MediaFormat(format).apply {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            // 各家芯片自己的低延迟开关（不认识的键会被忽略），与 Moonlight 相同
            setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
            setInteger("vendor.qti-ext-dec-picture-order.enable", 1)
            setInteger("vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1)
            setInteger("vendor.rtc-ext-dec-low-latency.enable", 1)
            setInteger("vendor.low-latency.enable", 1)
        }
        for (f in listOf(lowLatency, format)) {
            val mc = try {
                MediaCodec.createDecoderByType(p.codec.mime)
            } catch (e: Exception) {
                Log.w(TAG, "no ${p.codec.mime} decoder: $e")
                onError("no ${p.codec.label} decoder")
                return false
            }
            try {
                mc.setCallback(Callback(mc), handler)
                mc.configure(f, s, null, 0)
                mc.start()
                codec = mc
                config = Triple(p.codec, p.width, p.height)
                freeInputs.clear()
                Log.i(TAG, "decoder ${mc.name} started: ${p.codec.label} ${p.width}x${p.height}${if (f === lowLatency) " (low latency)" else ""}")
                return true
            } catch (e: Exception) {
                Log.w(TAG, "can't start ${mc.name}: $e")
                runCatching { mc.release() }
            }
        }
        onError("can't start the ${p.codec.label} decoder")
        return false
    }

    private fun releaseCodec() {
        val mc = codec ?: return
        codec = null
        config = null
        freeInputs.clear()
        runCatching { mc.stop() }
        runCatching { mc.release() }
    }

    private inner class Callback(private val mc: MediaCodec) : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
            if (codec !== this@VideoDecoder.codec) return
            freeInputs.addLast(index)
            feed()
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            if (codec !== this@VideoDecoder.codec) return
            try {
                codec.releaseOutputBuffer(index, info.size > 0 || (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) == 0)
            } catch (e: Exception) {
                Log.w(TAG, "releaseOutputBuffer: $e")
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            if (codec !== this@VideoDecoder.codec) return
            Log.w(TAG, "decoder error: ${e.diagnosticInfo}")
            fail("decoder error ${e.diagnosticInfo}")
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
    }

    companion object {
        private const val TAG = "airhand"
        private const val MAX_PENDING = 12

        /** 从关键帧里取出参数集，作为 csd-0 / csd-1（有的解码器只认这种方式） */
        fun csd(p: VideoPacket): List<ByteArray> {
            val nals = AnnexB.split(p.data, p.offset, p.length)
            val start = byteArrayOf(0, 0, 0, 1)
            fun nal(r: IntRange) = start + p.data.copyOfRange(r.first, r.last + 1)
            return when (p.codec) {
                Codec.H264 -> {
                    val sps = nals.firstOrNull { (p.data[it.first].toInt() and 0x1f) == 7 }
                    val pps = nals.firstOrNull { (p.data[it.first].toInt() and 0x1f) == 8 }
                    if (sps != null && pps != null) listOf(nal(sps), nal(pps)) else emptyList()
                }
                Codec.H265 -> {
                    val ps = nals.filter { ((p.data[it.first].toInt() shr 1) and 0x3f) in 32..34 }
                    if (ps.size >= 3) listOf(ps.fold(ByteArray(0)) { acc, r -> acc + nal(r) }) else emptyList()
                }
                Codec.AV1 -> emptyList()
            }
        }

        /** 本机能硬件解码的格式（H.264 只要有解码器就算） */
        val decodable: List<Codec> by lazy {
            val infos = runCatching { MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.toList() }.getOrDefault(emptyList())
            Codec.entries.filter { c ->
                infos.any { i ->
                    !i.isEncoder && i.supportedTypes.any { it.equals(c.mime, ignoreCase = true) } &&
                        (c == Codec.H264 || isHardware(i))
                }
            }.ifEmpty { listOf(Codec.H264) }
        }

        fun isHardware(i: MediaCodecInfo): Boolean =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) i.isHardwareAccelerated
            else !i.name.startsWith("OMX.google.") && !i.name.startsWith("c2.android.")
    }
}

/** Annex-B 码流（起始码分隔的 NAL 单元） */
object AnnexB {
    /** 各个 NAL 单元（不含起始码）在 `data` 里的范围 */
    fun split(data: ByteArray, offset: Int, length: Int): List<IntRange> {
        val out = ArrayList<IntRange>()
        val end = offset + length
        var start = -1
        var i = offset
        while (i + 2 < end) {
            if (data[i].toInt() == 0 && data[i + 1].toInt() == 0 && data[i + 2].toInt() == 1) {
                if (start >= 0) {
                    var e = i
                    while (e > start && data[e - 1].toInt() == 0) e--
                    if (e > start) out += start until e
                }
                i += 3
                start = i
            } else {
                i++
            }
        }
        if (start in offset until end) out += start until end
        return out
    }
}
