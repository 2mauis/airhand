package app.airhand.android.controller

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** 一个 Opus 包（48 kHz 立体声，20 ms） */
class AudioPacket(val start: Boolean, val data: ByteArray, val offset: Int, val length: Int)

/**
 * 播放被控端的声音：MediaCodec 解 Opus（所有 Android 版本都自带），AudioTrack 播放。
 *
 * 简单的抖动缓冲：每次开始（第一包、停顿之后、播空了）先垫 [CUSHION_MS] 的静音；
 * 积压超过 [MAX_BUFFER_MS] 时丢包追上，延迟不会越来越大。
 */
class AudioPlayer(private val onFailure: (String) -> Unit) {
    private val queue = LinkedBlockingQueue<AudioPacket>(64)

    @Volatile
    private var running = true
    private val thread = Thread({ loop() }, "airhand-audio").apply { start() }

    fun submit(p: AudioPacket) {
        if (!queue.offer(p)) {
            queue.clear()
            queue.offer(p)
        }
    }

    fun release() {
        running = false
        thread.interrupt()
    }

    private fun loop() {
        var dec: MediaCodec? = null
        var track: AudioTrack? = null
        try {
            dec = createDecoder()
            track = createTrack()
            track.play()
            val info = MediaCodec.BufferInfo()
            var written = 0L
            var pcm = ByteArray(0)
            while (running) {
                val p = try {
                    queue.poll(200, TimeUnit.MILLISECONDS)
                } catch (_: InterruptedException) {
                    break
                } ?: continue
                val idx = dec.dequeueInputBuffer(10_000)
                if (idx < 0) continue
                dec.getInputBuffer(idx)!!.apply { clear(); put(p.data, p.offset, p.length) }
                dec.queueInputBuffer(idx, 0, p.length, 0, 0)
                var start = p.start
                while (true) {
                    val o = dec.dequeueOutputBuffer(info, 5_000)
                    if (o < 0) break
                    val out = dec.getOutputBuffer(o)
                    if (out != null && info.size > 0) {
                        if (pcm.size < info.size) pcm = ByteArray(info.size)
                        out.position(info.offset)
                        out.get(pcm, 0, info.size)
                        val head = track.playbackHeadPosition.toLong() and 0xffffffffL
                        val buffered = (written - head).coerceAtLeast(0)
                        if (buffered == 0L) start = true
                        if (buffered > MAX_BUFFER_MS * RATE / 1000) {
                            // 积压太多（网络卡了一下之后一起到了）：丢掉这一包追上
                        } else {
                            if (start) {
                                written += write(track, ByteArray(CUSHION_MS * RATE / 1000 * FRAME_BYTES))
                                start = false
                            }
                            written += write(track, pcm, info.size)
                        }
                    }
                    dec.releaseOutputBuffer(o, false)
                }
            }
        } catch (e: Exception) {
            if (running) {
                Log.w(TAG, "audio playback failed: $e")
                onFailure(e.toString())
            }
        } finally {
            runCatching { track?.stop() }
            runCatching { track?.release() }
            runCatching { dec?.stop() }
            runCatching { dec?.release() }
        }
    }

    /** 写入 PCM，返回写入的帧数（不阻塞，写不下的部分丢掉） */
    private fun write(track: AudioTrack, b: ByteArray, len: Int = b.size): Long {
        val n = track.write(b, 0, len, AudioTrack.WRITE_NON_BLOCKING)
        return if (n > 0) (n / FRAME_BYTES).toLong() else 0L
    }

    private fun createTrack(): AudioTrack {
        val min = AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        val size = maxOf(min, (MAX_BUFFER_MS + 60) * RATE / 1000 * FRAME_BYTES)
        val b = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build(),
            )
            .setBufferSizeInBytes(size)
            .setTransferMode(AudioTrack.MODE_STREAM)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) b.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
        return b.build()
    }

    companion object {
        private const val TAG = "airhand"
        const val RATE = 48_000
        private const val FRAME_BYTES = 4
        private const val CUSHION_MS = 40
        private const val MAX_BUFFER_MS = 160

        /** Opus 解码器：csd-0 是 OpusHead，csd-1 / csd-2 是编码延迟和 seek 预滚（纳秒，本机字节序） */
        fun createDecoder(): MediaCodec {
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 2)
            val preSkip = 312
            val head = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN)
                .put("OpusHead".toByteArray()).put(1).put(2).putShort(preSkip.toShort())
                .putInt(RATE).putShort(0).put(0)
            head.flip()
            format.setByteBuffer("csd-0", head)
            fun ns(v: Long) = ByteBuffer.allocate(8).order(ByteOrder.nativeOrder()).putLong(v).flip() as ByteBuffer
            format.setByteBuffer("csd-1", ns(preSkip * 1_000_000_000L / RATE))
            format.setByteBuffer("csd-2", ns(80_000_000L))
            val mc = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
            mc.configure(format, null, null, 0)
            mc.start()
            return mc
        }
    }
}
