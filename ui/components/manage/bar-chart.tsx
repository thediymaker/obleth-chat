"use client";

// Bars over time for Usage, Insights, Workspaces and personal usage, drawn with
// recharts. Colors are Chat's chart tokens; the legend and the series colors are in
// chart-legend, which a page can show without loading recharts. Each bar's tooltip says
// everything about its day (or hour), and the wrapper carries a one-line summary for
// screen readers.

import { Bar, BarChart, CartesianGrid, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import type { Series } from "./chart-legend";

export function TimeBars<T extends { label: string }>({
  data,
  series,
  label,
  tip,
  format = (v) => String(Math.round(v)),
  limit,
  height = 190,
  every,
}: {
  data: T[];
  series: Series[];
  /** What the chart shows, in one sentence. */
  label: string;
  /** The tooltip text of one bar. */
  tip: (point: T) => React.ReactNode;
  format?: (value: number) => string;
  /** A dashed line across, such as the slots. */
  limit?: { value: number; color?: string };
  height?: number;
  /** Show every nth label, ending on the latest. */
  every?: number;
}) {
  const n = data.length;
  const step = every ?? Math.max(1, Math.ceil(n / 6));
  // The grid at nothing, half and the top, as the earlier charts drew it.
  const top = Math.max(1, limit?.value ?? 0, ...data.map((d) => series.reduce((sum, s) => sum + (Number((d as Record<string, unknown>)[s.key]) || 0), 0)));
  const labels = data.filter((_, i) => (n - 1 - i) % step === 0).map((d) => d.label);
  return (
    <div role="img" aria-label={label} className="w-full min-w-0 text-xs" style={{ height }}>
      <ResponsiveContainer width="100%" height="100%" minWidth={0} initialDimension={{ width: 600, height }}>
        <BarChart data={data} margin={{ top: 8, right: 4, bottom: 0, left: 0 }} barCategoryGap="22%">
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis dataKey="label" ticks={labels} interval={0} tickLine={false} axisLine={false} tick={{ fill: "var(--muted-foreground)", fontSize: 10 }} />
          <YAxis
            width={44}
            tickLine={false}
            axisLine={false}
            tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            tickFormatter={(v: number) => format(v)}
            ticks={[0, top / 2, top]}
            domain={[0, top]}
          />
          <Tooltip
            cursor={{ fill: "var(--secondary)" }}
            isAnimationActive={false}
            content={({ active, payload }) =>
              active && payload?.[0] ? (
                <div className="max-w-64 rounded-md border bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-md">{tip(payload[0].payload as T)}</div>
              ) : null
            }
          />
          {series.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              name={s.name}
              stackId="a"
              fill={s.color}
              isAnimationActive={false}
              stroke="var(--card)"
              strokeWidth={series.length > 1 ? 1 : 0}
              radius={i === series.length - 1 ? [3, 3, 0, 0] : 0}
              maxBarSize={48}
            />
          ))}
          {limit && <ReferenceLine y={limit.value} stroke={limit.color ?? "var(--destructive)"} strokeDasharray="4 3" ifOverflow="extendDomain" />}
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
