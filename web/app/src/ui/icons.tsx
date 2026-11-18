// Copied from src/shared/ui/icons.tsx at ca73ac2b
// Trimmed to the icons the web session screens use (the rest of the copy was dropped as unused). Add a glyph here when a screen needs one.
import { forwardRef, type Ref } from "react";
import { HugeiconsIcon, type HugeiconsIconProps, type IconSvgElement } from "@hugeicons/react";
import ArrowLeft01Icon from "@hugeicons/core-free-icons/ArrowLeft01Icon";
import Tick02Icon from "@hugeicons/core-free-icons/Tick02Icon";
import ArrowDown01Icon from "@hugeicons/core-free-icons/ArrowDown01Icon";
import ArrowRight01Icon from "@hugeicons/core-free-icons/ArrowRight01Icon";
import File01Icon from "@hugeicons/core-free-icons/File01Icon";
import Loading03Icon from "@hugeicons/core-free-icons/Loading03Icon";
import Comment01Icon from "@hugeicons/core-free-icons/Comment01Icon";
import Refresh01Icon from "@hugeicons/core-free-icons/Refresh01Icon";
import Search01Icon from "@hugeicons/core-free-icons/Search01Icon";
import ShieldAlertIcon from "@hugeicons/core-free-icons/ShieldAlertIcon";
import SparklesIcon from "@hugeicons/core-free-icons/SparklesIcon";
import Cancel01Icon from "@hugeicons/core-free-icons/Cancel01Icon";
import PencilEdit02Icon from "@hugeicons/core-free-icons/PencilEdit02Icon";
import SquareLock02Icon from "@hugeicons/core-free-icons/SquareLock02Icon";

/** Props shared by every chrome icon. `icon` is filled in by the named wrappers. */
export type IconProps = Omit<HugeiconsIconProps, "icon">;

export type IconComponent = ReturnType<typeof wrap>;

function wrap(icon: IconSvgElement, name: string) {
  const Component = forwardRef(function Icon(
    { strokeWidth = 1.75, ...props }: IconProps,
    ref: Ref<SVGSVGElement>,
  ) {
    return (
      <HugeiconsIcon
        ref={ref}
        icon={icon}
        strokeWidth={strokeWidth}
        {...props}
      />
    );
  });
  Component.displayName = name;
  return Component;
}

export const Check = wrap(Tick02Icon, "Check");
export const ChevronDown = wrap(ArrowDown01Icon, "ChevronDown");
export const ChevronLeft = wrap(ArrowLeft01Icon, "ChevronLeft");
export const ChevronRight = wrap(ArrowRight01Icon, "ChevronRight");
export const File = wrap(File01Icon, "File");
export const LoaderCircle = wrap(Loading03Icon, "LoaderCircle");
export const MessageSquare = wrap(Comment01Icon, "MessageSquare");
export const RefreshCw = wrap(Refresh01Icon, "RefreshCw");
export const Search = wrap(Search01Icon, "Search");
export const Shield = wrap(ShieldAlertIcon, "Shield");
export const Sparkles = wrap(SparklesIcon, "Sparkles");
export const X = wrap(Cancel01Icon, "X");
export const Pencil = wrap(PencilEdit02Icon, "Pencil");
export const Lock = wrap(SquareLock02Icon, "Lock");
