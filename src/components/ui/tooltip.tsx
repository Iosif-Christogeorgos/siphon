import { Tooltip as TooltipPrimitive } from "radix-ui";
import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * A hint closes as soon as the pointer leaves what it explains, even onto the hint
 * itself: Radix keeps it open over its own text by default, which made one linger.
 */
function TooltipProvider({
  delayDuration = 350,
  disableHoverableContent = true,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      delayDuration={delayDuration}
      disableHoverableContent={disableHoverableContent}
      {...props}
    />
  );
}

function TooltipContent({
  className,
  sideOffset = 6,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        sideOffset={sideOffset}
        className={cn(
          "z-50 w-fit origin-(--radix-tooltip-content-transform-origin) rounded-md border border-border bg-popover px-2 py-1 text-xs text-popover-foreground shadow-pop",
          "animate-in fade-in-0 zoom-in-95 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95",
          className,
        )}
        {...props}
      />
    </TooltipPrimitive.Portal>
  );
}

/** A hover hint for icon-only controls, or the longer explanation behind a short label. */
function Hint({
  label,
  children,
  side,
  className,
  when,
}: {
  label: React.ReactNode;
  children: React.ReactNode;
  side?: React.ComponentProps<typeof TooltipPrimitive.Content>["side"];
  className?: string;
  /** Asked each time the hint would open; it opens only if this says so (text that may or may not be cut off). */
  when?: () => boolean;
}) {
  const [open, setOpen] = React.useState(false);
  return (
    <TooltipPrimitive.Root
      open={when ? open : undefined}
      onOpenChange={when ? (next) => setOpen(next && when()) : undefined}
    >
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipContent side={side} className={className}>
        {label}
      </TooltipContent>
    </TooltipPrimitive.Root>
  );
}

export { Hint, TooltipProvider };
