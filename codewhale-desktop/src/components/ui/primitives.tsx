import { Tooltip as RTooltip } from "radix-ui";
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";

export function Tip({ label, children, side = "bottom" }: { label: ReactNode; children: ReactNode; side?: "top" | "bottom" | "left" | "right" }) {
  return (
    <RTooltip.Root delayDuration={400}>
      <RTooltip.Trigger asChild>{children}</RTooltip.Trigger>
      <RTooltip.Portal>
        <RTooltip.Content
          side={side}
          sideOffset={6}
          className="z-50 rounded-[var(--radius-sm)] border border-line bg-overlay px-2 py-1 text-[11px] text-fg shadow-[var(--shadow-md)]"
        >
          {label}
        </RTooltip.Content>
      </RTooltip.Portal>
    </RTooltip.Root>
  );
}

type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  active?: boolean;
  size?: "sm" | "md";
  tipSide?: "top" | "bottom" | "left" | "right";
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, active, size = "md", className = "", children, tipSide, ...rest },
  ref,
) {
  const dim = size === "sm" ? "h-7 w-7" : "h-8 w-8";
  return (
    <Tip label={label} side={tipSide}>
      <button
        ref={ref}
        aria-label={label}
        className={`${dim} inline-flex shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-fg2 transition-colors duration-100 hover:bg-hover hover:text-fg disabled:opacity-40 ${
          active ? "bg-hover text-fg" : ""
        } ${className}`}
        {...rest}
      >
        {children}
      </button>
    </Tip>
  );
});

export function Button({
  variant = "default",
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "default" | "primary" | "success" | "danger" | "ghost" }) {
  const styles: Record<string, string> = {
    default: "border border-line bg-elevated hover:bg-hover text-fg",
    primary: "bg-blue text-white hover:brightness-110",
    success: "bg-[#238636] text-white hover:brightness-110",
    danger: "border border-[color-mix(in_srgb,var(--accent-red)_50%,transparent)] text-red hover:bg-[color-mix(in_srgb,var(--accent-red)_12%,transparent)]",
    ghost: "text-fg2 hover:bg-hover hover:text-fg",
  };
  return (
    <button
      className={`inline-flex items-center justify-center gap-1.5 rounded-[var(--radius-sm)] px-3 py-1.5 text-[13px] font-medium transition-[background-color,filter] duration-100 disabled:opacity-50 ${styles[variant]} ${className}`}
      {...rest}
    />
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd>{children}</kbd>;
}

export function Dot({ color, pulse }: { color: string; pulse?: boolean }) {
  return (
    <span className="relative inline-flex h-2 w-2">
      {pulse && <span className="absolute inline-flex h-full w-full animate-ping rounded-full opacity-60" style={{ background: color }} />}
      <span className="relative inline-flex h-2 w-2 rounded-full" style={{ background: color }} />
    </span>
  );
}

export function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode; hint?: ReactNode }) {
  return (
    <label className="flex cursor-pointer items-start gap-2.5 py-1.5 text-[13px]">
      <button
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`mt-0.5 inline-flex h-4 w-7 shrink-0 items-center rounded-full border transition-colors ${
          checked ? "border-blue bg-blue" : "border-line-strong bg-overlay"
        }`}
      >
        <span className={`h-3 w-3 rounded-full bg-white shadow transition-transform ${checked ? "translate-x-3" : "translate-x-0.5"}`} />
      </button>
      <span className="min-w-0">
        <span className="text-fg">{label}</span>
        {hint && <span className="block text-[11px] text-fg3">{hint}</span>}
      </span>
    </label>
  );
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <div className="mb-1 mt-4 text-[11px] font-semibold uppercase tracking-wide text-fg3 first:mt-0">{children}</div>;
}
