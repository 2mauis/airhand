import type { Codec, QualitySettings } from "./api";
import { t } from "./i18n";

export type PresetId = "saver" | "balanced" | "smooth" | "hd" | "native";

export interface Preset {
  id: PresetId;
  settings: Omit<QualitySettings, "codec">;
}

/** 预设：从省流到原画。码率是上限，画面静止时几乎不占带宽。名称与说明见 i18n.ts 的 preset.* */
export const PRESETS: Preset[] = [
  { id: "saver", settings: { fps: 24, bitrate_kbps: 1200, max_short: 720, adaptive: true } },
  { id: "balanced", settings: { fps: 30, bitrate_kbps: 4000, max_short: 1080, adaptive: true } },
  { id: "smooth", settings: { fps: 60, bitrate_kbps: 8000, max_short: 1080, adaptive: true } },
  { id: "hd", settings: { fps: 60, bitrate_kbps: 16000, max_short: 1440, adaptive: true } },
  { id: "native", settings: { fps: 60, bitrate_kbps: 30000, max_short: 0, adaptive: true } },
];

export const DEFAULT_PRESET: PresetId = "balanced";

export const presetName = (id: PresetId) => t(`preset.${id}`);
export const presetDesc = (id: PresetId) => t(`preset.${id}.desc`);
export const codecHint = (c: Codec) => t(`codec.hint.${c}`);

export function presetSettings(id: PresetId, codec: Codec): QualitySettings {
  const p = PRESETS.find((p) => p.id === id) ?? PRESETS[1];
  return { ...p.settings, codec };
}

/** 判断当前设置是否与某个预设一致 */
export function matchPreset(s: QualitySettings): PresetId | null {
  const p = PRESETS.find(
    (p) =>
      p.settings.fps === s.fps &&
      p.settings.bitrate_kbps === s.bitrate_kbps &&
      p.settings.max_short === s.max_short,
  );
  return p?.id ?? null;
}

/** 本机能解码的格式里压缩率最高的一个（被控端没有对应的硬件编码器时会自动回退） */
export function preferredCodec(decoders: Codec[]): Codec {
  return (["AV1", "H265"] as const).find((c) => decoders.includes(c)) ?? "H264";
}

export const CODEC_LABEL: Record<Codec, string> = {
  H264: "H.264",
  H265: "H.265 / HEVC",
  AV1: "AV1",
};
