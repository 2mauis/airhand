package app.airhand.android.host

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import app.airhand.android.core.Native

/**
 * 共享手机正在播放的声音（Android 10+ 的 AudioPlaybackCapture，不录麦克风），编成 Opus（48 kHz 立体声，
 * 每包 20 ms）交给 Rust。和桌面版一样，静音时不发包。
 *
 * 只有控制端打开了声音时才运行。有的应用（例如一些视频应用）不允许别人录它的声音，那部分是静音。
 */
class AudioCapture(
    private val context: Context,
    private val projection: MediaProjection,
    private val handle: () -> Long,
    private val onFailure: (String?) -> Unit,
) {
    @Volatile
    private var running = true
    private val thread = Thread({ run() }, "airhand-audio-capture").apply { start() }

    fun release() {
        running = false
        thread.interrupt()
        thread.join(1000)
    }

    private fun run() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            onFailure("sharing sound needs Android 10 or newer")
            return
        }
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            onFailure("the RECORD_AUDIO permission was not granted on the phone")
            return
        }
        try {
            capture()
        } catch (e: Exception) {
            if (running) {
                Log.w(TAG, "audio capture failed: $e")
                onFailure(e.toString())
            }
        }
    }

    @SuppressLint("MissingPermission")
    @RequiresApi(Build.VERSION_CODES.Q)
    private fun capture() {
        val config = AudioPlaybackCaptureConfiguration.Builder(projection)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
            .addMatchingUsage(AudioAttributes.USAGE_GAME)
            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
            .build()
        val format = AudioFormat.Builder()
            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
            .setSampleRate(RATE)
            .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
            .build()
        val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        val rec = AudioRecord.Builder()
            .setAudioFormat(format)
            .setBufferSizeInBytes(maxOf(min, FRAME_BYTES * 8))
            .setAudioPlaybackCaptureConfig(config)
            .build()
        val enc = try {
            MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS).apply {
                val f = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 2)
                f.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
                configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                start()
            }
        } catch (e: Exception) {
            rec.release()
            onFailure("this phone has no Opus encoder: $e")
            return
        }
        try {
            rec.startRecording()
            onFailure(null)
            Log.i(TAG, "audio capture started")
            val pcm = ByteArray(FRAME_BYTES)
            val info = MediaCodec.BufferInfo()
            while (running) {
                var got = 0
                while (got < pcm.size && running) {
                    val n = rec.read(pcm, got, pcm.size - got)
                    if (n < 0) throw IllegalStateException("AudioRecord.read: $n")
                    got += n
                }
                if (!running) break
                val captureUs = System.nanoTime() / 1000 - FRAME_US
                if (!silent(pcm)) {
                    val i = enc.dequeueInputBuffer(10_000)
                    if (i >= 0) {
                        enc.getInputBuffer(i)!!.apply { clear(); put(pcm) }
                        enc.queueInputBuffer(i, 0, pcm.size, captureUs, 0)
                    }
                }
                while (true) {
                    val o = enc.dequeueOutputBuffer(info, 0)
                    if (o < 0) break
                    val buf = enc.getOutputBuffer(o)
                    val h = handle()
                    if (buf != null && info.size > 0 && h != 0L && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        Native.hostAudio(h, buf, info.offset, info.size, info.presentationTimeUs)
                    }
                    enc.releaseOutputBuffer(o, false)
                }
            }
        } finally {
            runCatching { rec.stop() }
            rec.release()
            runCatching { enc.stop() }
            enc.release()
            Log.i(TAG, "audio capture stopped")
        }
    }

    /** 一整包都是（几乎）无声 */
    private fun silent(pcm: ByteArray): Boolean {
        var i = 0
        while (i + 1 < pcm.size) {
            val s = (pcm[i].toInt() and 0xff) or (pcm[i + 1].toInt() shl 8)
            if (s > 3 || s < -3) return false
            i += 2
        }
        return true
    }

    companion object {
        private const val TAG = "airhand"
        private const val RATE = 48_000
        private const val FRAME_US = 20_000L

        /** 20 ms 的 16 位立体声 */
        private const val FRAME_BYTES = RATE / 50 * 4
    }
}
