package app.airhand.android.ui.session

import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.airhand.android.controller.Input
import app.airhand.android.controller.KeyMap

/**
 * 键盘：软键盘打的字、快捷键栏的特殊键、外接键盘，都从这里发给远端。
 *
 * 修饰键（Ctrl / Alt / Shift / Win）在快捷键栏上是"粘滞"的：点一下只对下一个键有效（[Sticky.ONCE]），
 * 再点一下锁定（[Sticky.LOCK]），再点取消。比如点 Ctrl 再在软键盘上打 c，发出去的就是 Ctrl+C。
 */
class KeyboardController(private val send: (List<Any>) -> Unit) {
    enum class Mod(val code: String) { CTRL("ControlLeft"), ALT("AltLeft"), SHIFT("ShiftLeft"), META("MetaLeft") }
    enum class Sticky { OFF, ONCE, LOCK }

    val mods = mutableStateMapOf<Mod, Sticky>()

    /** 输入法正在组字（例如拼音）的内容，还没有发出去，界面上显示出来 */
    var composing by mutableStateOf("")

    fun state(m: Mod): Sticky = mods[m] ?: Sticky.OFF

    fun toggle(m: Mod) {
        mods[m] = when (state(m)) {
            Sticky.OFF -> Sticky.ONCE
            Sticky.ONCE -> Sticky.LOCK
            Sticky.LOCK -> Sticky.OFF
        }
    }

    private fun active(): List<Mod> = Mod.entries.filter { state(it) != Sticky.OFF }

    private fun consumeOnce() {
        Mod.entries.forEach { if (state(it) == Sticky.ONCE) mods[it] = Sticky.OFF }
    }

    /** 按一下某个键（带上当前的修饰键） */
    fun press(code: String, extraShift: Boolean = false) {
        val codes = active().map { it.code }.toMutableList()
        if (extraShift && Mod.SHIFT.code !in codes) codes += Mod.SHIFT.code
        send(Input.chord(codes + code))
        consumeOnce()
    }

    /** 同时按下一组键（快捷键菜单里的组合键） */
    fun chord(codes: List<String>) = send(Input.chord(codes))

    /** 输入法提交的文字 */
    fun text(s: String) {
        if (s.isEmpty()) return
        val withMods = active().any { it != Mod.SHIFT }
        if (withMods || (state(Mod.SHIFT) != Sticky.OFF && s.length == 1)) {
            // 有修饰键：按键的方式发（Ctrl+C 之类）
            for (c in s) {
                val k = KeyMap.forChar(c)
                if (k != null) press(k.first, k.second) else send(listOf(Input.text(c.toString())))
            }
            return
        }
        // 普通文字：换行变成回车键，其余整段发过去
        val parts = s.split('\n')
        parts.forEachIndexed { i, part ->
            if (part.isNotEmpty()) send(listOf(Input.text(part)))
            if (i < parts.size - 1) press("Enter")
        }
    }

    /** 外接键盘 / 软键盘发来的按键事件。返回是否处理了 */
    fun key(e: KeyEvent): Boolean {
        val code = KeyMap.code(e.keyCode) ?: return false
        when (e.action) {
            KeyEvent.ACTION_DOWN -> {
                if (code in MODIFIER_CODES) {
                    send(listOf(Input.key(code, true)))
                } else {
                    val mods = active().map { it.code }.filter { m -> m !in pressedHardware(e) }
                    send(mods.map { Input.key(it, true) } + Input.key(code, true))
                    if (mods.isNotEmpty()) send(listOf(Input.key(code, false)) + mods.reversed().map { Input.key(it, false) })
                    consumeOnce()
                }
            }
            KeyEvent.ACTION_UP -> send(listOf(Input.key(code, false)))
        }
        return true
    }

    private fun pressedHardware(e: KeyEvent): Set<String> = buildSet {
        if (e.isCtrlPressed) add(Mod.CTRL.code)
        if (e.isAltPressed) add(Mod.ALT.code)
        if (e.isShiftPressed) add(Mod.SHIFT.code)
        if (e.isMetaPressed) add(Mod.META.code)
    }

    companion object {
        private val MODIFIER_CODES = setOf(
            "ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight",
        )
    }
}

/**
 * 软键盘的输入连接：本地的编辑框始终是空的，提交的文字立刻发给远端。
 * 拼音这类组字过程留在输入法里（同时显示在界面上），提交时才发；删除键变成退格键。
 */
class RemoteInputConnection(view: View, private val kb: KeyboardController) : BaseInputConnection(view, true) {
    private fun clear() {
        editable?.let {
            removeComposingSpans(it)
            it.clear()
        }
        kb.composing = ""
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        val s = text?.toString() ?: ""
        clear()
        kb.text(s)
        return true
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        super.setComposingText(text, newCursorPosition)
        kb.composing = text?.toString() ?: ""
        return true
    }

    override fun finishComposingText(): Boolean {
        val pending = editable?.toString() ?: ""
        clear()
        super.finishComposingText()
        if (pending.isNotEmpty()) kb.text(pending)
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        if (kb.composing.isNotEmpty()) return super.deleteSurroundingText(beforeLength, afterLength)
        repeat(beforeLength.coerceAtMost(64)) { kb.press("Backspace") }
        repeat(afterLength.coerceAtMost(64)) { kb.press("Delete") }
        return true
    }

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_DEL || event.keyCode == KeyEvent.KEYCODE_ENTER || event.keyCode == KeyEvent.KEYCODE_FORWARD_DEL) {
            if (event.action == KeyEvent.ACTION_DOWN) kb.press(if (event.keyCode == KeyEvent.KEYCODE_DEL) "Backspace" else if (event.keyCode == KeyEvent.KEYCODE_ENTER) "Enter" else "Delete")
            return true
        }
        return kb.key(event) || super.sendKeyEvent(event)
    }

    override fun performEditorAction(actionCode: Int): Boolean {
        kb.press("Enter")
        return true
    }

    companion object {
        fun configure(outAttrs: EditorInfo) {
            outAttrs.inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
            outAttrs.initialSelStart = 0
            outAttrs.initialSelEnd = 0
        }
    }
}
