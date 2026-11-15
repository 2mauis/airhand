import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openSession } from "../lib/actions";
import { getLang } from "../lib/i18n";
import { bestCodec } from "../lib/codec";
import { presetSettings } from "../lib/presets";
import { useStore } from "../lib/store";
type Prompt = { token: number; kind: string; name: string; summary: string; sas: string | null; expires: number };
type Status = { counters: Record<string,number>; remember: boolean; role: string | null; paired: boolean; hasInvitation: boolean; listening: boolean; active: boolean; path: string; prompt: Prompt | null; error: string | null };
export function DirectPairing() {
 const zh = getLang() === "zh"; const tr = (cn: string,en: string) => zh ? cn : en;
 const [status,setStatus] = useState<Status | null>(null); const [error,setError] = useState<string | null>(null);
 const [busy,setBusy] = useState(false); const [input,setInput] = useState(false); const [sas,setSas] = useState<string | null>(null); const [time,setTime] = useState(Date.now());
 const draft = useStore(s => s.draft); const decoders = useStore(s => s.decoders);
 const refresh = () => invoke<Status>("direct_status").then(setStatus).catch(() => setError(tr("无法读取配对状态","Pairing status unavailable")));
 useEffect(() => { void refresh(); const changes = listen("direct-status",() => void refresh()); const codes = listen<string>("direct-sas",e => setSas(e.payload)); const interval = setInterval(() => { setTime(Date.now()); void refresh(); },1000); return () => { clearInterval(interval); void changes.then(f => f()); void codes.then(f => f()); }; },[]);
 useEffect(() => setInput(false),[status?.prompt?.token]); useEffect(() => { if (status?.paired) setSas(null); },[status?.paired]);
 const action = async (action: string) => { setBusy(true); setError(null); try { await invoke("direct_action",{ action }); } catch (e) { setError(String(e)); } finally { setBusy(false); await refresh(); } };
 const decide = async (allowed: boolean) => { if (!status?.prompt) return; try { await invoke("direct_decide",{ token: status.prompt.token,allowed,input: allowed && input }); } catch { setError(tr("请求已过期","Request expired")); } await refresh(); };
 const prompt = status?.prompt;
 const path = status?.path === "direct" ? tr("UDP 直连","UDP direct") : status?.path === "lost" ? tr("直连丢失，输入已停止","Direct path lost; input stopped") : tr("等待直连 / 引导中","Waiting for direct / bootstrapping");
 const button = (actionName: string,cn: string,en: string) => <button disabled={busy} onClick={() => void action(actionName)}>{tr(cn,en)}</button>;
 return <div className="newtab"><div className="nt-wrap"><section className="nt-card" aria-label="Tailcat pairing">
  <h1>AirHand · Tailcat</h1><p>{tr("单对 macOS MVP · 配对不等于会话授权","Single-pair macOS MVP · pairing does not grant session access")}</p><p role="status" aria-live="polite">{path}</p>
  {!status?.role && <div className="direct-actions">{button("request","配对新的被控端…（导出 R 并 AirDrop）","Pair a new host… (export R and AirDrop)")}{button("importRequest","被控端：导入配对请求 R…","Host: import pairing request R…")}</div>}
  {status?.role === "controller" && <div className="direct-actions">{!status.hasInvitation && button("importInvitation","导入邀请 I…（成功后删除文件）","Import invitation I… (delete file after import)")}{status.hasInvitation && <button disabled={busy || status.active || draft?.busy} onClick={() => { setSas(null); void openSession("tailcat",presetSettings("balanced",bestCodec(decoders))); }}>{tr("连接已配对设备","Connect paired device")}</button>}{sas && <p>{tr("请与被控端比较首连校验码：","Compare this first-session code with the host: ")}<strong className="direct-sas">{sas}</strong></p>}</div>}
  {status?.role === "host" && <div className="direct-actions"><p>{status.paired ? tr("已配对 · 每次会话均需本机允许","Paired · local approval required for every session") : tr("等待首次 SAS 确认","Waiting for first-session SAS confirmation")}</p>{button(status.listening ? "stop" : "allow",status.listening ? "关闭允许被连接" : "允许被连接",status.listening ? "Disable connections" : "Allow connections")}<p>{tr("邀请 I 含秘密。AirDrop 完成后删除导出的原始文件。","Invitation I contains a secret. Delete the original export after AirDrop completes.")}</p></div>}
  {prompt && <section className="direct-prompt" role="alertdialog" aria-modal="true" aria-label={tr("本机授权","Local authorization")}><h2>{prompt.kind === "pair" ? tr("允许配对？","Allow pairing?") : prompt.kind === "sas" ? tr("两边校验码一致？","Do both codes match?") : tr("允许本次会话？","Allow this session?")}</h2><p>{tr("对方自报名（不可信）：","Self-reported name (untrusted): ")}{prompt.name}</p><p>{prompt.summary}</p>{prompt.sas && <p className="direct-sas">{prompt.sas}</p>}<p>{tr("剩余秒数：","Seconds remaining: ")}{Math.max(0,prompt.expires - Math.floor(time / 1000))}</p>{prompt.kind === "session" && <label><input type="checkbox" checked={input} onChange={e => setInput(e.target.checked)} />{tr("允许键盘和鼠标控制（默认关闭）","Allow keyboard and mouse control (off by default)")}</label>}<div className="direct-actions"><button autoFocus onClick={() => void decide(false)}>{tr("拒绝","Reject")}</button><button onClick={() => void decide(true)}>{tr("允许","Allow")}</button></div></section>}
  {status?.role && button("remember",status.remember ? "记住配对：开（实验取向，可关闭）" : "记住配对：关（退出后忘记）",status.remember ? "Remember pair: on (experimental; switchable)" : "Remember pair: off (forget on exit)")}
  {status?.active && button("disconnect","立即断开当前会话","Disconnect current session")}{status?.role && button("forget","移除配对并撤销旧密钥","Remove pair and revoke old keys")}
  {(error || status?.error) && <p role="alert">{error || status?.error}</p>}{draft?.error && <p role="alert">{tr("连接未获允许或直连不可用","Connection declined or direct path unavailable")}</p>}
  <details><summary>{tr("诊断计数（不含端点或秘密）","Diagnostic counters (no endpoints or secrets)")}</summary>{Object.entries(status?.counters ?? {}).map(([key,value]) => <p key={key}>{key}: {value}</p>)}</details>
  <p className="muted">{tr("双 Mac 画面与键鼠验收尚待进行。长期 DERP 数据兜底和配对持久化策略待产品决定。","Two-Mac screen/input acceptance is pending. Long-term DERP data fallback and pairing persistence remain product decisions.")}</p>
 </section></div></div>;
}
