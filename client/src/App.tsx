import { useEffect } from "react";
import { ConnectView } from "./components/ConnectView";
import { GridView } from "./components/GridView";
import { SessionView } from "./components/SessionView";
import { Sidebar } from "./components/Sidebar";
import { api } from "./lib/api";
import { supportedDecoders } from "./lib/codec";
import { setLang, useLang } from "./lib/i18n";
import { store, useStore } from "./lib/store";

export function App() {
  const view = useStore((s) => s.view);
  const sessions = useStore((s) => s.sessions);
  const immersive = useStore((s) => s.immersive);
  // 语言切换时整个界面重新渲染
  useLang();

  useEffect(() => {
    const unLang = api.onLangChanged(setLang);
    void api.appInfo().then((info) => {
      store.setApp(info);
      setLang(info.lang);
    });
    void api.serverStatus().then(store.setServer);
    void supportedDecoders().then(store.setDecoders);
    const un = api.onServerStatus(store.setServer);
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
    return () => {
      void un.then((f) => f());
      void unLang.then((f) => f());
      window.removeEventListener("keydown", block);
      window.removeEventListener("contextmenu", noMenu);
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
