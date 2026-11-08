package app.airhand.android.controller

import android.graphics.Bitmap
import android.graphics.Color
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import app.airhand.android.AirhandApp
import app.airhand.android.core.Codec
import app.airhand.android.core.DisplayInfo
import app.airhand.android.core.EncoderInfo
import app.airhand.android.core.HostHello
import app.airhand.android.core.Native
import app.airhand.android.core.Notice
import app.airhand.android.core.OsKind
import app.airhand.android.core.Preset
import app.airhand.android.core.Problem
import app.airhand.android.core.Quality
import app.airhand.android.core.ServerState
import app.airhand.android.core.Stats
import app.airhand.android.core.SystemAction
import app.airhand.android.core.optStringOrNull
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.concurrent.thread

/** 远端光标：形状由被控端发来，位置由本机决定（触控板模式下本机画出来） */
data class CursorState(val visible: Boolean = true, val bitmap: Bitmap? = null, val hotX: Int = 0, val hotY: Int = 0)

/**
 * 一个远程会话（看 / 操作一台设备）。连接、断线重连、等对方上线都在这里，界面只观察 [phase] 等状态。
 *
 * * 对方要先问它的用户（手机默认这样）：[Phase.Approving]，同意之后才是 [Phase.Live]；
 * * 第一次连接失败（控制码不对、对方不在线、对方拒绝……）：[Phase.Failed]，回到首页显示原因；
 * * 连上之后断了：带着重连凭证自动重连（[Phase.Reconnecting]）；对方离线了就等它上线（[Phase.WaitingHost]）；
 * * 对方断开了我们 / 换了控制码：[Phase.Ended]，不再重连。
 *
 * 对方同意、开始会话时把设备（和重连凭证）记进"我的设备"，下次点一下就能连；对方不在线时也能等它上线。
 */
class RemoteSession(
    private val app: AirhandApp,
    val code: String,
    resume: String?,
    /** 只看画面（电视）：不发送任何输入 */
    val viewOnly: Boolean,
) {
    sealed interface Phase {
        data object Connecting : Phase

        /** 连上了，在等对方的用户同意 */
        data object Approving : Phase
        data object Live : Phase
        data class Reconnecting(val reason: Problem) : Phase
        data class WaitingHost(val reason: Problem?) : Phase
        data class Failed(val reason: Problem) : Phase
        data class Ended(val reason: Problem) : Phase
        data object Closed : Phase
    }

    private val client = app.controller
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    private val _phase = MutableStateFlow<Phase>(Phase.Connecting)
    val phase: StateFlow<Phase> = _phase.asStateFlow()

    private val saved = app.devices.get(code)
    val hostName = MutableStateFlow(saved?.title() ?: "")
    val hostOs = MutableStateFlow(saved?.hostOs ?: "")
    val hello = MutableStateFlow<HostHello?>(null)
    val displays = MutableStateFlow<List<DisplayInfo>>(emptyList())
    val currentDisplay = MutableStateFlow(0)
    val encoder = MutableStateFlow<EncoderInfo?>(null)
    val stats = MutableStateFlow<Stats?>(null)

    /** 被控端的提示，按类别（capture / audio / permissions / other）各保留一条 */
    val notices = MutableStateFlow<Map<String, Notice>>(emptyMap())
    val videoSize = MutableStateFlow<Pair<Int, Int>?>(null)
    val cursor = MutableStateFlow(CursorState())
    val audioOn = MutableStateFlow(app.settings.value.audio)
    val avSyncOn = MutableStateFlow(app.settings.value.avSync)

    /** 本机播放声音出错 */
    val audioError = MutableStateFlow<String?>(null)
    val quality = MutableStateFlow(Preset.of(app.settings.value.preset).quality(VideoDecoder.decodable.firstOrNull { it in Codec.PRIORITY } ?: Codec.H264))

    private val _clipboard = MutableSharedFlow<String>(extraBufferCapacity = 4)

    /** 被控端复制了文本（已经写进本机剪贴板），界面提示一下 */
    val clipboardIn: SharedFlow<String> = _clipboard.asSharedFlow()

    /** 对方的平台：连上之后以对方报告的为准，之前按系统名称猜 */
    val os: OsKind get() = hello.value?.platform?.takeIf { it != OsKind.UNKNOWN } ?: OsKind.of(hostOs.value)

    @Volatile
    private var resume: String? = resume

    @Volatile
    private var handle = 0L

    @Volatile
    private var sid = -1

    @Volatile
    private var closed = false

    @Volatile
    private var background = false
    private var connectedOnce = false
    private val wake = Object()
    private var woken = false

    private val sync = AvSync()
    private val decoder = VideoDecoder(sync, requestKeyframe = { command(JSONObject().put("op", "keyframe")) })
    @Volatile private var audio: AudioPlayer? = null
    private var audioToken = 0L
    private val sender = InputSender { events ->
        if (!viewOnly) command(JSONObject().put("op", "input").put("events", events))
    }

    fun start() {
        // 等的被控端上线了 / 与服务器重新连上了：叫醒等待中的连接线程
        scope.launch {
            client.hostOnline.collect { if (it == code) signal() }
        }
        scope.launch {
            var was: ServerState? = null
            client.server.collect {
                if (it is ServerState.Connected && was !is ServerState.Connected) signal()
                was = it
            }
        }
        thread(name = "airhand-session") { run() }
    }

    // ───────────────────────────── 连接 ─────────────────────────────

    private fun signal() = synchronized(wake) {
        woken = true
        wake.notifyAll()
    }

    /** 最多等 `ms`，被 [signal] 叫醒时提前返回 */
    private fun sleep(ms: Long) = synchronized(wake) {
        if (!woken && !closed) wake.wait(ms)
        woken = false
    }

    private fun run() {
        client.acquire()
        try {
            var attempts = 0
            while (!closed) {
                val serverProblem = waitServer(if (connectedOnce) Long.MAX_VALUE else FIRST_SERVER_WAIT_MS)
                if (closed) break
                if (serverProblem != null) {
                    _phase.value = Phase.Failed(serverProblem)
                    break
                }
                val h = client.handle()
                val cmd = JSONObject()
                    .put("op", "connect")
                    .put("code", code)
                    .put("resume", resume ?: JSONObject.NULL)
                    .put("settings", currentSettings().toJson())
                val r = runCatching { JSONObject(Native.ctlCall(h, cmd.toString())) }
                    .getOrElse { JSONObject().put("err", JSONObject().put("kind", "failed").put("detail", it.toString())) }
                if (closed) {
                    r.optJSONObject("ok")?.let { Native.ctlCall(h, JSONObject().put("op", "close").put("sid", it.getInt("sid")).toString()) }
                    break
                }
                val ok = r.optJSONObject("ok")
                if (ok != null) {
                    attempts = 0
                    onConnected(h, ok)
                    val end = pump(h, ok.getInt("sid")) ?: break
                    sid = -1
                    decoder.resetStream()
                    stopAudio()
                    if (closed) break
                    if (!end.second) {
                        // 对方拒绝 / 没回应 / 断开了我们：第一次就没进去的话回到首页说明原因
                        _phase.value = if (connectedOnce) Phase.Ended(end.first) else Phase.Failed(end.first)
                        break
                    }
                    _phase.value = Phase.Reconnecting(end.first)
                    sleep(RETRY_MS)
                    continue
                }
                val p = Problem.parse(r.optJSONObject("err"))
                attempts++
                when (p.kind) {
                    "hostOffline" -> {
                        _phase.value = Phase.WaitingHost(p)
                        // 对方上线时服务器通知（signal）；保险起见隔一会儿也再问一次
                        sleep(OFFLINE_RECHECK_MS)
                    }
                    "notConnected", "serverTimeout", "serverLost" -> {
                        if (!connectedOnce && attempts >= 3) {
                            _phase.value = Phase.Failed(p)
                            break
                        }
                        if (connectedOnce) _phase.value = Phase.Reconnecting(p)
                        sleep(RETRY_MS * 2)
                    }
                    else -> {
                        _phase.value = if (connectedOnce) Phase.Ended(p) else Phase.Failed(p)
                        break
                    }
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "session thread failed", e)
            _phase.value = Phase.Failed(Problem("failed", e.toString()))
        } finally {
            stopAudio()
            client.release()
            if (closed) _phase.value = Phase.Closed
        }
    }

    /** 等到连上服务器。返回 null 表示连上了；第一次连接时等不到就返回原因 */
    private fun waitServer(limitMs: Long): Problem? {
        val start = SystemClock.elapsedRealtime()
        while (!closed) {
            when (val s = client.server.value) {
                is ServerState.Connected -> if (client.handle() != 0L) return null
                is ServerState.Disconnected -> {
                    // 版本不一致、证书不对、设置写错了：等下去也不会好
                    if (!connectedOnce && s.reason.kind in listOf("incompatible", "certMismatch", "failed")) return s.reason
                    if (SystemClock.elapsedRealtime() - start > limitMs) return s.reason
                }
                else -> if (SystemClock.elapsedRealtime() - start > limitMs) return Problem("serverTimeout")
            }
            Thread.sleep(100)
        }
        return null
    }

    /** 服务器给出的对方名称和系统，开始会话时记进"我的设备" */
    private var grantName = ""
    private var grantOs = ""

    /** 连上了被控端（还没开始：对方可能要先问它的用户，见 [onHello]） */
    private fun onConnected(h: Long, ok: JSONObject) {
        handle = h
        sid = ok.getInt("sid")
        resume = ok.optStringOrNull("resume")
        grantName = ok.optString("hostName")
        grantOs = ok.optString("hostOs")
        if (grantName.isNotBlank() && saved?.alias.isNullOrBlank()) hostName.value = grantName
        hostOs.value = grantOs
        // 声音和后台帧率现在就告诉对方（对方同意之前只是记下来）
        if (audioOn.value) startAudio()
        if (background) applyBackground()
        Log.i(TAG, "connected to $code ($grantName, $grantOs)")
    }

    /** 会话开始（对方同意了，或者不需要同意） */
    private fun onHello() {
        app.devices.connected(code, grantName, grantOs, resume ?: "")
        connectedOnce = true
        _phase.value = Phase.Live
    }

    /** 读会话数据直到会话结束。返回 (原因, 是否重连)；用户关闭时返回 null */
    private fun pump(h: Long, sid: Int): Pair<Problem, Boolean>? {
        var quickNulls = 0
        while (!closed) {
            if (client.handle() != h) return Problem("serverLost", "connection restarted") to true
            val t0 = SystemClock.elapsedRealtime()
            val b = Native.ctlPollSession(h, sid, 250)
            if (b == null) {
                // 队列不在了（不应该发生）：别空转
                if (SystemClock.elapsedRealtime() - t0 < 50 && ++quickNulls > 20) return Problem("sessionEnded") to true
                continue
            }
            quickNulls = 0
            if (b.isEmpty()) continue
            if (b[0] == '{'.code.toByte()) {
                val e = runCatching { JSONObject(String(b, Charsets.UTF_8)) }.getOrNull() ?: continue
                if (e.optString("type") == "closed") {
                    Native.ctlCall(h, JSONObject().put("op", "close").put("sid", sid).toString())
                    return Problem.parse(e.optJSONObject("reason")) to e.optBoolean("retry", true)
                }
                handleEvent(e)
            } else {
                handlePacket(h, sid, b)
            }
        }
        return null
    }

    private fun handlePacket(h: Long, sid: Int, b: ByteArray) {
        if (b.size < HEADER) return
        val bb = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN)
        when (b[0].toInt()) {
            1 -> {
                val seq = bb.getInt(4)
                val w = bb.getShort(8).toInt() and 0xffff
                val ht = bb.getShort(10).toInt() and 0xffff
                if (videoSize.value != w to ht) videoSize.value = w to ht
                decoder.submit(
                    VideoPacket(
                        seq = seq,
                        key = b[1].toInt() == 1,
                        codec = Codec.fromId(bb.getShort(2).toInt()),
                        width = w,
                        height = ht,
                        ptsUs = bb.getLong(16),
                        data = b,
                        offset = HEADER,
                        length = b.size - HEADER,
                        ack = { Native.ctlAck(h, sid, seq) },
                    ),
                )
            }
            2 -> {
                val visible = b[1].toInt() == 1
                val hasShape = bb.getShort(2).toInt() != 0
                if (!hasShape) {
                    cursor.value = cursor.value.copy(visible = visible)
                } else {
                    val w = bb.getShort(8).toInt() and 0xffff
                    val ht = bb.getShort(10).toInt() and 0xffff
                    val bmp = cursorBitmap(b, w, ht)
                    cursor.value = CursorState(visible, bmp, bb.getShort(12).toInt() and 0xffff, bb.getShort(14).toInt() and 0xffff)
                }
            }
            3 -> audio?.submit(AudioPacket(b[1].toInt() == 1, bb.getLong(16), b, HEADER, b.size - HEADER))
        }
    }

    private fun handleEvent(e: JSONObject) {
        when (e.optString("type")) {
            "hello" -> {
                val codecs = e.optJSONArray("codecs").strings().mapNotNull { Codec.fromWire(it) }
                val h = HostHello(
                    hostname = e.optString("hostname"),
                    os = e.optString("os"),
                    version = e.optString("version"),
                    displays = DisplayInfo.parseList(e.optJSONArray("displays")),
                    current = e.optInt("current"),
                    codecs = codecs,
                    platform = OsKind.fromWire(e.optString("platform")),
                    touch = e.optBoolean("touch"),
                    actions = e.optJSONArray("actions").strings().mapNotNull { SystemAction.fromWire(it) },
                )
                hello.value = h
                onHello()
                displays.value = h.displays
                currentDisplay.value = h.current
                if (h.hostname.isNotBlank() && saved?.alias.isNullOrBlank()) hostName.value = h.hostname
                // 选双方都支持的压缩率最高的格式（对方能编码、本机能硬件解码）
                val best = Codec.PRIORITY.firstOrNull { it in codecs && it in VideoDecoder.decodable } ?: Codec.H264
                if (quality.value.codec != best) setQuality(quality.value.copy(codec = best))
            }
            "awaitingApproval" -> _phase.value = Phase.Approving
            "encoder" -> encoder.value = EncoderInfo(
                Codec.fromWire(e.optString("codec")) ?: Codec.H264,
                e.optString("name"),
                e.optBoolean("hardware"),
                e.optInt("width"),
                e.optInt("height"),
            )
            "displays" -> {
                displays.value = DisplayInfo.parseList(e.optJSONArray("displays"))
                currentDisplay.value = e.optInt("current")
            }
            "stats" -> stats.value = Stats(
                e.optDouble("fps").toFloat(),
                e.optDouble("kbps").toFloat(),
                if (e.isNull("rttMs")) null else e.optDouble("rttMs").toFloat(),
                e.optInt("width"),
                e.optInt("height"),
            )
            "notice" -> {
                val o = e.optJSONObject("notice") ?: return
                val n = Notice(o.optString("kind"), o.optStringOrNull("detail"), o.optBoolean("screen"), o.optBoolean("input"))
                val m = notices.value.toMutableMap()
                when (n.kind) {
                    "captureRecovered" -> m.remove("capture")
                    "audioRecovered" -> m.remove("audio")
                    "captureFailed" -> m["capture"] = n
                    "audioFailed" -> m["audio"] = n
                    "missingPermissions" -> if (!n.screen && !n.input) m.remove("permissions") else m["permissions"] = n
                    else -> m[n.kind] = n
                }
                notices.value = m
            }
            "clipboard" -> {
                val text = e.optString("text")
                if (text.isNotEmpty()) {
                    scope.launch {
                        app.clipboard.write(text)
                        _clipboard.tryEmit(text)
                    }
                }
            }
        }
    }

    private fun currentSettings(): Quality {
        val q = quality.value
        return if (background) q.copy(fps = BACKGROUND_FPS) else q
    }

    // ───────────────────────────── 命令 ─────────────────────────────

    /** 给当前会话发命令（`sid` 自动填上）。没连上时丢掉 */
    private fun command(cmd: JSONObject) {
        val h = handle
        val s = sid
        if (h == 0L || s < 0 || closed) return
        cmd.put("sid", s)
        val r = Native.ctlCall(h, cmd.toString())
        if (r.contains("\"err\"")) Log.w(TAG, "${cmd.optString("op")} failed: $r")
    }

    fun setSurface(surface: Surface?) = decoder.setSurface(surface)

    /** 输入事件（触控、键盘）。`now=false` 的会和后面的鼠标移动合并 */
    fun input(events: List<Any>, now: Boolean = true) {
        if (!viewOnly && _phase.value == Phase.Live) sender.send(events, now)
    }

    fun setQuality(q: Quality) {
        quality.value = q
        command(JSONObject().put("op", "settings").put("settings", currentSettings().toJson()))
    }

    fun selectDisplay(index: Int) {
        command(JSONObject().put("op", "display").put("index", index))
    }

    fun setAudio(on: Boolean) {
        audioOn.value = on
        if (on) startAudio() else stopAudio()
    }

    @Synchronized
    fun setAvSync(on: Boolean) {
        avSyncOn.value = on
        app.updateSettings { it.copy(avSync = on) }
        sync.setEnabled(on && audio != null)
        decoder.refreshTiming()
    }

    @Synchronized
    private fun startAudio() {
        if (closed || background || !audioOn.value) return
        if (audio == null) {
            audioError.value = null
            val token = sync.reset()
            audioToken = token
            sync.setEnabled(avSyncOn.value)
            audio = AudioPlayer(sync, token) { error ->
                synchronized(this) {
                    if (audioToken == token) {
                        audioError.value = error
                        audio = null
                        audioToken = sync.reset()
                        sync.setEnabled(false)
                        decoder.refreshTiming()
                    }
                }
            }
        }
        command(JSONObject().put("op", "audio").put("enabled", true))
    }

    @Synchronized
    private fun stopAudio() {
        command(JSONObject().put("op", "audio").put("enabled", false))
        audio?.release()
        audio = null
        audioToken = sync.reset()
        sync.setEnabled(false)
        decoder.refreshTiming()
    }

    /** 把文本同步到对方的剪贴板（对方之后可以粘贴） */
    /** 系统操作（只有对方在 Hello 里列出的才发） */
    fun system(action: SystemAction) {
        if (hello.value?.actions?.contains(action) == true) input(listOf(Input.system(action)))
    }

    fun sendClipboard(text: String) {
        if (text.isNotEmpty()) command(JSONObject().put("op", "clipboard").put("text", text))
    }

    /** 以键盘输入的方式把文本打到对方当前的输入框里 */
    fun typeText(text: String) {
        text.chunked(512).forEach { input(listOf(Input.text(it))) }
    }

    /** 立即重试（等对方上线时用户点了"重试"） */
    fun retryNow() = signal()

    /** 画面不在屏幕上（应用切到后台）：降到很低的帧率，回来时恢复并要一个关键帧 */
    fun setBackground(bg: Boolean) {
        if (background == bg) return
        background = bg
        applyBackground()
    }

    private fun applyBackground() {
        command(JSONObject().put("op", "settings").put("settings", currentSettings().toJson()))
        if (background) stopAudio() else if (audioOn.value) startAudio()
    }

    fun close() {
        if (closed) return
        closed = true
        signal()
        val (h, s) = handle to sid
        sender.release()
        if (h != 0L && s >= 0) {
            thread(name = "airhand-close") {
                Native.ctlCall(h, JSONObject().put("op", "close").put("sid", s).toString())
            }
        }
        stopAudio()
        decoder.release()
        scope.cancel()
        if (_phase.value !is Phase.Failed) _phase.value = Phase.Closed
    }

    companion object {
        private const val TAG = "airhand"
        private const val HEADER = 24
        private const val FIRST_SERVER_WAIT_MS = 12_000L
        private const val RETRY_MS = 800L
        private const val OFFLINE_RECHECK_MS = 60_000L
        private const val BACKGROUND_FPS = 2

        /** 光标形状：RGBA（非预乘）→ Bitmap */
        fun cursorBitmap(b: ByteArray, w: Int, h: Int): Bitmap? {
            if (w == 0 || h == 0 || b.size < HEADER + w * h * 4) return null
            val px = IntArray(w * h)
            var o = HEADER
            for (i in px.indices) {
                px[i] = Color.argb(b[o + 3].toInt() and 0xff, b[o].toInt() and 0xff, b[o + 1].toInt() and 0xff, b[o + 2].toInt() and 0xff)
                o += 4
            }
            return Bitmap.createBitmap(px, w, h, Bitmap.Config.ARGB_8888)
        }
    }
}

private fun JSONArray?.strings(): List<String> = if (this == null) emptyList() else (0 until length()).map { optString(it) }
