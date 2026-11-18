const MEGABYTE = 1024 * 1024;

export function bytes(value: number): string {
  const megabytes = value / MEGABYTE;
  return megabytes < 1000 ? `${Math.round(megabytes)} MB` : `${(megabytes / 1024).toFixed(2)} GB`;
}

export function gigabytes(value: number): string {
  return `${Math.round(value / MEGABYTE / 1024)} GB`;
}

export function remaining(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "";
  if (seconds < 60) return `약 ${Math.max(Math.round(seconds), 1)}초 남음`;
  return `약 ${Math.ceil(seconds / 60)}분 남음`;
}

export interface PackSummary {
  minecraftVersion: string;
  mods: number;
  shaderpacks: number;
}

export function summary(pack: PackSummary | null): string {
  if (!pack) return "";
  const parts = [`마인크래프트 ${pack.minecraftVersion}`];
  if (pack.mods > 0) parts.push(`모드 ${pack.mods}개`);
  if (pack.shaderpacks > 0) parts.push(`쉐이더 ${pack.shaderpacks}개`);
  return parts.join(" · ");
}
