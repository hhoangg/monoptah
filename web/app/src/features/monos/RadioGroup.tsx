// A radiogroup with the keyboard model of native radios: one tab stop (the selected radio), arrow keys move to the
// next or previous radio and select it, Home and End jump to the ends. The radios are the `role="radio"` buttons in
// the group; each sets `tabIndex={radioTabIndex(selected, index, anySelected)}` (the first radio is the tab stop
// while none is selected, for example a colour the desktop picked that is not one of the swatches).
import type { ComponentProps } from "react";

export const radioTabIndex = (selected: boolean, index: number, anySelected: boolean) => (selected || (!anySelected && index === 0) ? 0 : -1);

export function RadioGroup({ onKeyDown, ...props }: ComponentProps<"div">) {
  return (
    <div
      role="radiogroup"
      onKeyDown={(event) => {
        onKeyDown?.(event);
        if (event.defaultPrevented) return;
        const radios = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]:not(:disabled)')];
        const at = radios.indexOf(document.activeElement as HTMLButtonElement);
        if (at < 0) return;
        let next = -1;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (at + 1) % radios.length;
        else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (at - 1 + radios.length) % radios.length;
        else if (event.key === "Home") next = 0;
        else if (event.key === "End") next = radios.length - 1;
        if (next < 0) return;
        event.preventDefault();
        radios[next]?.focus();
        radios[next]?.click();
      }}
      {...props}
    />
  );
}
