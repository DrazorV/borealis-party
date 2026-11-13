import { BAND_WORD, healthBand, type HealthBand } from "./common";

/**
 * An HP bar coloured by band (Unhurt / Bloodied / Down, plan D6), shared by the DM tracker and the player's Combat
 * sub-tab. With numbers it shows current/max (+temp); `bandOnly` shows the band alone, for what players may see.
 */
export function HpBar({ current, max, temp = 0, band, bandOnly, label }: {
  current?: number;
  max?: number;
  temp?: number;
  /** Given instead of numbers (a player's view of others). */
  band?: HealthBand | null;
  bandOnly?: boolean;
  label?: string;
}) {
  const b = band ?? (current != null && max != null ? healthBand({ current, max }) : null);
  const pct = current != null && max ? Math.max(0, Math.min(100, (100 * current) / max)) : b === "down" ? 0 : b === "bloodied" ? 40 : 100;
  const word = b ? BAND_WORD[b] : "Unknown";
  return (
    <div className={`cb-hp cb-hp-${b ?? "none"}`}>
      <div className="cb-hp-bar" role="meter" aria-label={label ?? "Hit points"} aria-valuemin={0} aria-valuemax={max ?? 100}
        aria-valuenow={current ?? pct} aria-valuetext={bandOnly || current == null ? word : `${current} of ${max}${temp ? `, ${temp} temporary` : ""}, ${word}`}>
        <i style={{ width: `${pct}%` }} />
      </div>
      {bandOnly || current == null
        ? <span className="cb-hp-text">{word}</span>
        : <span className="cb-hp-text tabular">{current}<span className="muted">/{max}</span>{temp ? <span className="cb-hp-temp"> +{temp}</span> : null}</span>}
    </div>
  );
}
