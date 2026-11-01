// 全局状态：会话列表 / 当前视图 / 服务器状态（不持久化：控制端不在磁盘上留下任何数据）
import { useCallback, useSyncExternalStore } from "react";
import type { AppInfo, Codec, ServerState } from "./api";
import type { RemoteSession } from "./session";

/** "new" = 新建连接页，"grid" = 多屏总览，其余为会话 id */
export type View = "new" | "grid" | string;

interface State {
  sessions: RemoteSession[];
  view: View;
  server: ServerState;
  app: AppInfo | null;
  decoders: Codec[];
  immersive: boolean;
}

let state: State = {
  sessions: [],
  view: "new",
  server: { state: "connecting" },
  app: null,
  decoders: ["H264"],
  immersive: false,
};

const listeners = new Set<() => void>();

function set(patch: Partial<State>) {
  state = { ...state, ...patch };
  syncVisibility();
  for (const l of listeners) l();
}

/** 只有正在显示的会话保持全帧率；只有当前查看的会话（不含多屏总览）播放声音 */
function syncVisibility() {
  for (const s of state.sessions) {
    s.setVisible(state.view === "grid" || state.view === s.id);
    s.setForeground(state.view === s.id);
  }
}

export const store = {
  get: () => state,
  subscribe: (fn: () => void) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  },
  setServer: (server: ServerState) => set({ server }),
  setApp: (app: AppInfo) => set({ app }),
  setDecoders: (decoders: Codec[]) => set({ decoders }),
  setView: (view: View) => set({ view, immersive: view === "new" || view === "grid" ? false : state.immersive }),
  setImmersive: (immersive: boolean) => set({ immersive }),
  add: (s: RemoteSession) => set({ sessions: [...state.sessions, s], view: s.id }),
  remove: (id: string) => {
    const idx = state.sessions.findIndex((s) => s.id === id);
    if (idx < 0) return;
    const s = state.sessions[idx];
    s.dispose();
    const sessions = state.sessions.filter((x) => x.id !== id);
    let view = state.view;
    if (view === id) {
      const next = sessions[Math.min(idx, sessions.length - 1)];
      view = next ? next.id : "new";
    }
    if (view === "grid" && sessions.length === 0) view = "new";
    set({ sessions, view, immersive: view === state.view ? state.immersive : false });
  },
};

export function useStore<T>(select: (s: State) => T): T {
  return useSyncExternalStore(store.subscribe, () => select(state));
}

/** 订阅单个会话的变化 */
export function useSession(s: RemoteSession | undefined): number {
  const sub = useCallback(
    (fn: () => void) => {
      if (!s) return () => {};
      const off = s.subscribe(fn);
      return () => void off();
    },
    [s],
  );
  const get = useCallback(() => (s ? s.getVersion() : 0), [s]);
  return useSyncExternalStore(sub, get);
}
