// 网页版用不了时的说明页。
//
// 浏览器只在安全的页面（HTTPS 或 localhost）上提供 WebCodecs、AudioWorklet、剪贴板这些功能。
// airhand-web 在同一个端口上同时支持 HTTP 和 HTTPS，所以只要把地址换成 https:// 就行。
import { api } from "../lib/api";
import { useT } from "../lib/i18n";
import { IconWarn } from "./Icons";
import { LangSwitch } from "./Sidebar";

export type Unsupported = "insecure" | "noWebCodecs";

export function webUnsupported(): Unsupported | null {
  if (api.kind !== "web") return null;
  if (!window.isSecureContext) return "insecure";
  if (typeof VideoDecoder === "undefined") return "noWebCodecs";
  return null;
}

export function WebUnsupported({ reason }: { reason: Unsupported }) {
  const t = useT();
  const https = new URL(location.href);
  https.protocol = "https:";
  return (
    <div className="app">
      <main className="main connect-view">
        <div className="connect-card">
        <div className="hero-icon">
          <IconWarn size={30} />
        </div>
          {reason === "insecure" ? (
            <>
              <h1>{t("web.insecure.title")}</h1>
              <p className="muted">{t("web.insecure.body")}</p>
              <a className="primary-btn link-btn" href={https.href}>
                {https.href}
              </a>
              <p className="muted">{t("web.insecure.cert")}</p>
            </>
          ) : (
            <>
              <h1>{t("web.unsupported.title")}</h1>
              <p className="muted">{t("web.unsupported.body")}</p>
            </>
          )}
          <LangSwitch />
        </div>
      </main>
    </div>
  );
}
