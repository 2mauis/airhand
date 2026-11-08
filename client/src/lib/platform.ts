// 本机（控制台所在的电脑）是什么系统：决定窗口按钮怎么画。
import { api } from "./api";

export type Os = "mac" | "win" | "linux";

const ua = typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent;

export const OS: Os = /Mac/.test(ua) ? "mac" : /Win/.test(ua) ? "win" : "linux";

/**
 * 桌面版 macOS：保留系统的红绿灯按钮（叠在标签栏左上角，界面留出位置）；
 * Windows / Linux 是无边框窗口，最小化、最大化、关闭按钮由界面画。网页版没有窗口按钮
 */
export const NATIVE_WINDOW_BUTTONS = api.frame !== null && OS === "mac";

/** 给 <html> 加上系统的 class（样式里区分，例如 Linux 的无边框窗口要自己画边框） */
export function markPlatform() {
  document.documentElement.classList.add(api.frame ? `os-${OS}` : "os-web");
}
