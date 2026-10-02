import * as Tooltip from "@radix-ui/react-tooltip";
import * as Dropdown from "@radix-ui/react-dropdown-menu";
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "../icons/Icon";
import { ariaShortcut, formatShortcut } from "../commands/shortcuts";

/** Tooltip that also appears on keyboard focus (Radix default). */
export function Tip({ label, shortcut, children }: { label: string; shortcut?: string; children: ReactNode }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="tooltip" sideOffset={6}>
          {label}
          {shortcut ? ` (${formatShortcut(shortcut)})` : ""}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon: IconName;
  label: string;
  shortcut?: string;
  pressed?: boolean;
  mirror?: boolean;
  toolbarItem?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, label, shortcut, pressed, mirror, toolbarItem, className, ...rest },
  ref,
) {
  return (
    <Tip label={label} shortcut={shortcut}>
      <button
        ref={ref}
        type="button"
        className={`btn btn-icon ${className ?? ""}`}
        aria-label={label}
        aria-pressed={pressed}
        aria-keyshortcuts={shortcut ? ariaShortcut(shortcut) : undefined}
        data-tb={toolbarItem ? "" : undefined}
        {...rest}
      >
        <Icon name={icon} className={mirror ? "icon-mirror" : undefined} />
      </button>
    </Tip>
  );
});

export function MenuItem({ icon, shortcut, children, onSelect, disabled }: {
  icon?: IconName;
  shortcut?: string;
  children: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
}) {
  return (
    <Dropdown.Item className="menu-item" onSelect={onSelect} disabled={disabled}>
      {icon && (
        <span className="menu-indicator">
          <Icon name={icon} size={16} />
        </span>
      )}
      {children}
      {shortcut && <span className="menu-shortcut">{formatShortcut(shortcut)}</span>}
    </Dropdown.Item>
  );
}

export function MenuRadio({ value, children }: { value: string; children: ReactNode }) {
  return (
    <Dropdown.RadioItem className="menu-item" value={value}>
      <Dropdown.ItemIndicator className="menu-indicator">
        <Icon name="check" size={16} />
      </Dropdown.ItemIndicator>
      {children}
    </Dropdown.RadioItem>
  );
}
