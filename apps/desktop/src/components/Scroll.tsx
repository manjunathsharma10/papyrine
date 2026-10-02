import * as ScrollArea from "@radix-ui/react-scroll-area";
import { forwardRef, type ReactNode } from "react";

/** Radix ScrollArea with a ref to the scrolling viewport (needed for virtualisation). */
export const Scroll = forwardRef<HTMLDivElement, { children: ReactNode; label?: string }>(function Scroll({ children, label }, ref) {
  return (
    <ScrollArea.Root className="scroll-root" type="auto">
      <ScrollArea.Viewport ref={ref} className="scroll-viewport" aria-label={label} tabIndex={-1}>
        {children}
      </ScrollArea.Viewport>
      <ScrollArea.Scrollbar className="scrollbar" orientation="vertical">
        <ScrollArea.Thumb className="scrollbar-thumb" />
      </ScrollArea.Scrollbar>
    </ScrollArea.Root>
  );
});
