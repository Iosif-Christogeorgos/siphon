import type { KeyboardEvent } from "react";

/**
 * Arrow keys inside a row of radio buttons: move to the previous or next one
 * and pick it, as a native radio group does. Put on the `radiogroup` element.
 */
export function onRadioKeys(event: KeyboardEvent<HTMLElement>) {
  const step = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
  if (step === undefined) return;
  const radios = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("[role=radio]:not(:disabled):not([aria-disabled=true])")];
  const index = radios.indexOf(document.activeElement as HTMLButtonElement);
  if (index === -1) return;
  event.preventDefault();
  event.stopPropagation();
  const next = radios[(index + step + radios.length) % radios.length];
  next?.focus();
  next?.click();
}
