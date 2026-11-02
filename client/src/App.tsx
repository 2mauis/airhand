import { useEffect } from "react";
import { ConnectView } from "./components/ConnectView";
import { GridView } from "./components/GridView";
import { SessionView } from "./components/SessionView";
import { Sidebar } from "./components/Sidebar";
import { WebUnsupported, webUnsupported } from "./components/WebUnsupported";
import { api } from "./lib/api";
import { supportedDecoders } from "./lib/codec";
import { setLang, useLang } from "./lib/i18n";
import { store, useStore } from "./lib/store";

export function App() {
  // 语言切换时整个界面重新渲染
  useLang();
  // 网页版：不是安全页面（HTTPS / localhost）或浏览器太旧，没有视频解码可用
  const unsupported = webUnsupported();
  return unsupported ? <WebUnsupported reason={unsupported} /> : <Console />;
}

function Console() {
  const view = useStore((s) => s.view);
  const sessions = useStore((s) => s.sessions);
  const immersive = useStore((s) => s.immersive);

  useEffect(() => {
    const unLang = api.onLangChanged(setLang);
    void api.appInfo().then((info) => {
      store.setApp(info);
      setLang(info.lang);
    });
    void api.serverStatus().then(store.setServer);
    void supportedDecoders().then(store.setDecoders);
    const unStatus = api.onServerStatus(store.setServer);
    const unOnline = api.onHostOnline(store.hostOnline);
    // 屏蔽 WebView 自带的刷新/查找/打印等快捷键，避免误触
    const block = (e: KeyboardEvent) => {
      if (e.key === "F5" || ((e.ctrlKey || e.metaKey) && ["r", "p", "f", "g", "j", "u"].includes(e.key.toLowerCase()))) {
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", block);
    const noMenu = (e: MouseEvent) => {
      if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
    };
    window.addEventListener("contextmenu", noMenu);
    // 网页版：关闭或刷新标签页会断开所有会话，先让浏览器确认一下
    const confirmLeave = (e: BeforeUnloadEvent) => {
      if (api.kind === "web" && store.get().sessions.length > 0) e.preventDefault();
    };
    window.addEventListener("beforeunload", confirmLeave);
    return () => {
      unStatus();
      unOnline();
      unLang();
      window.removeEventListener("keydown", block);
      window.removeEventListener("contextmenu", noMenu);
      window.removeEventListener("beforeunload", confirmLeave);
    };
  }, []);

  const active = sessions.find((s) => s.id === view);

  return (
    <div className={`app ${immersive ? "immersive" : ""}`}>
      {!immersive && <Sidebar />}
      <main className="main">
        {view === "grid" && sessions.length > 0 ? (
          <GridView />
        ) : active ? (
          <SessionView key={active.id} s={active} />
        ) : (
          <ConnectView />
        )}
      </main>
    </div>
  );
}
