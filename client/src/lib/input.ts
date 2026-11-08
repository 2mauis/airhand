// Browser input ownership and batching. Disconnected/replaced sessions discard
// pending input; a delayed timer must never send old input to a new SID.
import type { InputEvent, MouseButton } from "./api";
const BUTTONS: MouseButton[] = ["Left", "Middle", "Right", "Back", "Forward"];
const IS_MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || navigator.userAgent);
const META_KEYS = new Set(["MetaLeft", "MetaRight"]);
const MODIFIERS = new Set(["ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight", "CapsLock", "Fn"]);

export class RemoteInput {
  private canvas: HTMLCanvasElement;
  private send: (events: InputEvent[]) => void;
  private connected: () => boolean;
  interactive = false;
  private queue: InputEvent[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  private scheduled = false;
  private generation = 0;
  private wheel = { x: 0, y: 0 };
  private pressed = new Set<string>();
  private abort = new AbortController();

  constructor(canvas: HTMLCanvasElement, connected: () => boolean, send: (events: InputEvent[]) => void) {
    this.canvas = canvas; this.connected = connected; this.send = send;
    this.bind();
  }
  reset() {
    this.generation++;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.scheduled = false;
    this.queue = [];
    this.pressed.clear();
    this.wheel = { x: 0, y: 0 };
  }
  dispose() { this.reset(); this.abort.abort(); }
  releaseAll() { this.pressed.clear(); this.enqueue(["ReleaseAll"], true); }
  focus() { this.canvas.focus({ preventScroll: true }); }
  enqueue(events: InputEvent[], urgent: boolean) {
    if (!this.connected()) return;
    for (const event of events) {
      const last = this.queue[this.queue.length - 1];
      if (typeof event === "object" && "MouseMove" in event && last && typeof last === "object" && "MouseMove" in last) {
        this.queue[this.queue.length - 1] = event;
      } else this.queue.push(event);
      // Bound even a synchronous sendText loop. Keyboard edges stay ordered.
      if (this.queue.length >= 64) this.flush();
    }
    const gen = this.generation;
    if (urgent && !this.scheduled) {
      this.scheduled = true;
      queueMicrotask(() => { if (gen === this.generation) this.flush(); });
    } else if (!urgent && this.timer === undefined) {
      this.timer = setTimeout(() => { if (gen === this.generation) this.flush(); }, 8);
    }
  }
  private flush() {
    this.scheduled = false;
    clearTimeout(this.timer); this.timer = undefined;
    const batch = this.queue; this.queue = [];
    if (batch.length && this.connected()) this.send(batch);
  }
  private pos(e: MouseEvent) {
    const r = this.canvas.getBoundingClientRect();
    return { x: Math.min(1, Math.max(0, (e.clientX - r.left) / Math.max(1, r.width))),
      y: Math.min(1, Math.max(0, (e.clientY - r.top) / Math.max(1, r.height))) };
  }
  private tapCaps() {
    this.enqueue([{ Key: { code: "CapsLock", down: true } }, { Key: { code: "CapsLock", down: false } }], true);
  }
  private bind() {
    const c = this.canvas;
    const on = <K extends keyof HTMLElementEventMap>(name: K, fn: (e: HTMLElementEventMap[K]) => void, opts?: AddEventListenerOptions) =>
      c.addEventListener(name, fn as EventListener, { ...opts, signal: this.abort.signal });
    on("pointermove", e => { if (this.interactive) this.enqueue([{ MouseMove: this.pos(e) }], false); });
    on("pointerdown", e => {
      if (!this.interactive) return;
      e.preventDefault(); this.focus(); c.setPointerCapture(e.pointerId);
      const button = BUTTONS[e.button];
      if (button) this.enqueue([{ MouseMove: this.pos(e) }, { MouseButton: { button, down: true } }], true);
    });
    on("pointerup", e => {
      if (!this.interactive) return;
      e.preventDefault();
      const button = BUTTONS[e.button];
      if (button) this.enqueue([{ MouseMove: this.pos(e) }, { MouseButton: { button, down: false } }], true);
    });
    on("pointercancel", () => this.releaseAll());
    on("contextmenu", e => e.preventDefault());
    on("auxclick", e => e.preventDefault());
    on("wheel", e => {
      if (!this.interactive) return;
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 360 : 1.2;
      this.wheel.x += e.deltaX * unit; this.wheel.y -= e.deltaY * unit;
      const dx = Math.trunc(this.wheel.x), dy = Math.trunc(this.wheel.y);
      if (!dx && !dy) return;
      this.wheel.x -= dx; this.wheel.y -= dy;
      const clamp = (n: number) => Math.max(-32000, Math.min(32000, n));
      this.enqueue([{ Wheel: { dx: clamp(dx), dy: clamp(dy) } }], true);
    }, { passive: false });
    on("keydown", e => {
      if (!this.interactive) return;
      e.preventDefault(); e.stopPropagation();
      if (!e.code || e.code === "Unidentified") return;
      if (IS_MAC && e.code === "CapsLock") return this.tapCaps();
      if (e.repeat && !this.pressed.has(e.code)) return;
      this.pressed.add(e.code);
      this.enqueue([{ Key: { code: e.code, down: true } }], true);
    });
    on("keyup", e => {
      if (!this.interactive) return;
      e.preventDefault(); e.stopPropagation();
      if (!e.code || e.code === "Unidentified") return;
      if (IS_MAC && e.code === "CapsLock") return this.tapCaps();
      this.pressed.delete(e.code);
      const events: InputEvent[] = [];
      if (IS_MAC && META_KEYS.has(e.code)) {
        for (const code of this.pressed) {
          if (MODIFIERS.has(code)) continue;
          this.pressed.delete(code);
          events.push({ Key: { code, down: false } });
        }
      }
      events.push({ Key: { code: e.code, down: false } });
      this.enqueue(events, true);
    });
    on("blur", () => this.releaseAll());
  }
}
