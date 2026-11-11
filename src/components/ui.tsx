import { useRef, type ReactNode } from "react";
import { Icon } from "./Icon";
import { change, percent } from "../format";

export function PageHeader({ title, subtitle, actions, eyebrow, children }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode; eyebrow?: ReactNode; children?: ReactNode }) {
  return <header className="kd-page-header">
    <div className="kd-page-heading">{eyebrow && <span className="kd-page-eyebrow">{eyebrow}</span>}<h1>{title}</h1>{subtitle && <p>{subtitle}</p>}</div>
    {actions && <div className="kd-page-actions">{actions}</div>}
    {children}
  </header>;
}

/** Keyboard-navigable segmented control with tab semantics. */
export function Segmented<T extends string>({ value, options, onChange, label, className = "" }: { value: T; options: { value: T; label: ReactNode }[]; onChange: (value: T) => void; label: string; className?: string }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  return <div className={`kd-segmented ${className}`} role="tablist" aria-label={label}>{options.map((option, index) => <button key={option.value} ref={element => { refs.current[index] = element; }} type="button" role="tab" aria-selected={value === option.value} tabIndex={value === option.value ? 0 : -1} onClick={() => onChange(option.value)} onKeyDown={event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + options.length) % options.length;
    onChange(options[next].value); refs.current[next]?.focus();
  }}>{option.label}</button>)}</div>;
}

/** Up/down change versus a comparable period. Less screen time is not judged as good or bad. */
export function DeltaBadge({ current, previous, label }: { current: number; previous: number; label: string }) {
  const value = change(current, previous);
  if (value === null) return <span className="kd-delta is-neutral" title={label}>暂无对比</span>;
  const direction = Math.abs(value) < .005 ? "flat" : value > 0 ? "up" : "down";
  return <span className={`kd-delta is-${direction}`} title={label}>
    {direction !== "flat" && <Icon name={direction === "up" ? "arrowUp" : "arrowDown"} size={11} strokeWidth={2.4}/>}
    {direction === "flat" ? "持平" : percent(Math.abs(value))}<span className="kd-delta-label">{label}</span>
  </span>;
}

export function StatTile({ icon, label, value, detail, tone }: { icon: string; label: string; value: ReactNode; detail?: ReactNode; tone?: string }) {
  return <div className="kd-stat-tile" style={tone ? { ["--tile-tone" as string]: tone } : undefined}>
    <span className="kd-stat-icon"><Icon name={icon} size={15} strokeWidth={1.9}/></span>
    <span className="kd-stat-label">{label}</span>
    <strong className="kd-stat-value">{value}</strong>
    {detail && <small className="kd-stat-detail">{detail}</small>}
  </div>;
}

export function Card({ title, aside, children, className = "", ...rest }: { title?: ReactNode; aside?: ReactNode; children: ReactNode; className?: string } & Omit<React.HTMLAttributes<HTMLElement>, "title">) {
  return <section className={`kd-card ${className}`} {...rest}>
    {(title || aside) && <header className="kd-card-header">{title && <h2>{title}</h2>}{aside && <div className="kd-card-aside">{aside}</div>}</header>}
    {children}
  </section>;
}

export function Skeleton({ width = "100%", height = 14 }: { width?: number | string; height?: number }) {
  return <span className="kd-skeleton" style={{ width, height }} aria-hidden="true"/>;
}
