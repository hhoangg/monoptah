// Copied from src/shared/ui/icons.tsx at ca73ac2b
// Trimmed to the icons the web session screens use. Add a glyph here when a screen needs one.
import { forwardRef, type Ref } from "react";
import { HugeiconsIcon, type HugeiconsIconProps, type IconSvgElement } from "@hugeicons/react";
import AlertCircleIcon from "@hugeicons/core-free-icons/AlertCircleIcon";
import Archive02Icon from "@hugeicons/core-free-icons/Archive02Icon";
import CircleArrowDown01Icon from "@hugeicons/core-free-icons/CircleArrowDown01Icon";
import ArrowLeft01Icon from "@hugeicons/core-free-icons/ArrowLeft01Icon";
import ArrowUp01Icon from "@hugeicons/core-free-icons/ArrowUp01Icon";
import BotIcon from "@hugeicons/core-free-icons/BotIcon";
import Tick02Icon from "@hugeicons/core-free-icons/Tick02Icon";
import CheckmarkCircle02Icon from "@hugeicons/core-free-icons/CheckmarkCircle02Icon";
import ArrowDown01Icon from "@hugeicons/core-free-icons/ArrowDown01Icon";
import ArrowRight01Icon from "@hugeicons/core-free-icons/ArrowRight01Icon";
import UnfoldMoreIcon from "@hugeicons/core-free-icons/UnfoldMoreIcon";
import CircleDashedIcon from "@hugeicons/core-free-icons/CircleDashedIcon";
import CircleDotIcon from "@hugeicons/core-free-icons/CircleDotIcon";
import HelpCircleIcon from "@hugeicons/core-free-icons/HelpCircleIcon";
import CancelCircleIcon from "@hugeicons/core-free-icons/CancelCircleIcon";
import Clock01Icon from "@hugeicons/core-free-icons/Clock01Icon";
import CloudIcon from "@hugeicons/core-free-icons/CloudIcon";
import CodeXmlIcon from "@hugeicons/core-free-icons/CodeXmlIcon";
import Copy01Icon from "@hugeicons/core-free-icons/Copy01Icon";
import ArrowTurnForwardIcon from "@hugeicons/core-free-icons/ArrowTurnForwardIcon";
import LinkSquare02Icon from "@hugeicons/core-free-icons/LinkSquare02Icon";
import File01Icon from "@hugeicons/core-free-icons/File01Icon";
import FileDiffIcon from "@hugeicons/core-free-icons/FileDiffIcon";
import FileScriptIcon from "@hugeicons/core-free-icons/FileScriptIcon";
import Folder01Icon from "@hugeicons/core-free-icons/Folder01Icon";
import FolderOpenIcon from "@hugeicons/core-free-icons/FolderOpenIcon";
import ViewIcon from "@hugeicons/core-free-icons/ViewIcon";
import GitBranchIcon from "@hugeicons/core-free-icons/GitBranchIcon";
import ImageAdd01Icon from "@hugeicons/core-free-icons/ImageAdd01Icon";
import Link01Icon from "@hugeicons/core-free-icons/Link01Icon";
import LeftToRightListBulletIcon from "@hugeicons/core-free-icons/LeftToRightListBulletIcon";
import FilterIcon from "@hugeicons/core-free-icons/FilterIcon";
import Loading03Icon from "@hugeicons/core-free-icons/Loading03Icon";
import Layers01Icon from "@hugeicons/core-free-icons/Layers01Icon";
import Comment01Icon from "@hugeicons/core-free-icons/Comment01Icon";
import CommentAdd01Icon from "@hugeicons/core-free-icons/CommentAdd01Icon";
import MinusSignIcon from "@hugeicons/core-free-icons/MinusSignIcon";
import MoreHorizontalIcon from "@hugeicons/core-free-icons/MoreHorizontalIcon";
import Attachment01Icon from "@hugeicons/core-free-icons/Attachment01Icon";
import PauseIcon from "@hugeicons/core-free-icons/PauseIcon";
import Add01Icon from "@hugeicons/core-free-icons/Add01Icon";
import Refresh01Icon from "@hugeicons/core-free-icons/Refresh01Icon";
import Search01Icon from "@hugeicons/core-free-icons/Search01Icon";
import Settings01Icon from "@hugeicons/core-free-icons/Settings01Icon";
import ShieldAlertIcon from "@hugeicons/core-free-icons/ShieldAlertIcon";
import PreferenceHorizontalIcon from "@hugeicons/core-free-icons/PreferenceHorizontalIcon";
import SparklesIcon from "@hugeicons/core-free-icons/SparklesIcon";
import SquareIcon from "@hugeicons/core-free-icons/SquareIcon";
import StarIcon from "@hugeicons/core-free-icons/StarIcon";
import ComputerTerminal01Icon from "@hugeicons/core-free-icons/ComputerTerminal01Icon";
import Delete02Icon from "@hugeicons/core-free-icons/Delete02Icon";
import Wrench01Icon from "@hugeicons/core-free-icons/Wrench01Icon";
import Cancel01Icon from "@hugeicons/core-free-icons/Cancel01Icon";
import PencilEdit02Icon from "@hugeicons/core-free-icons/PencilEdit02Icon";
import GaugeIcon from "@hugeicons/core-free-icons/GaugeIcon";
import ChartBreakoutSquareIcon from "@hugeicons/core-free-icons/ChartBreakoutSquareIcon";
import SquareLock02Icon from "@hugeicons/core-free-icons/SquareLock02Icon";
import GlobeIcon from "@hugeicons/core-free-icons/GlobeIcon";
import DownloadIcon from "@hugeicons/core-free-icons/Download01Icon";

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

const stroke = {
  stroke: "currentColor",
  strokeLinecap: "round",
  strokeLinejoin: "round",
  strokeWidth: "1.5",
} as const;

/** Catalog FoldVertical/UnfoldVertical use filled chevrons; keep these stroke-only. */
const foldDashes: IconSvgElement = [
  ["path", { d: "M2 12H4", ...stroke, key: "0" }],
  ["path", { d: "M8 12H10", ...stroke, key: "1" }],
  ["path", { d: "M14 12H16", ...stroke, key: "2" }],
  ["path", { d: "M20 12H22", ...stroke, key: "3" }],
];

const FoldVerticalIcon: IconSvgElement = [
  ...foldDashes,
  ["path", { d: "M7 2L12 7L17 2", ...stroke, key: "4" }],
  ["path", { d: "M7 22L12 17L17 22", ...stroke, key: "5" }],
];

const UnfoldVerticalIcon: IconSvgElement = [
  ...foldDashes,
  ["path", { d: "M7 7L12 2L17 7", ...stroke, key: "4" }],
  ["path", { d: "M7 17L12 22L17 17", ...stroke, key: "5" }],
];

export const AlertCircle = wrap(AlertCircleIcon, "AlertCircle");
export const Archive = wrap(Archive02Icon, "Archive");
export const ArrowDownCircle = wrap(CircleArrowDown01Icon, "ArrowDownCircle");
export const ArrowLeft = wrap(ArrowLeft01Icon, "ArrowLeft");
export const ArrowUp = wrap(ArrowUp01Icon, "ArrowUp");
export const Bot = wrap(BotIcon, "Bot");
export const Check = wrap(Tick02Icon, "Check");
export const CheckCircle = wrap(CheckmarkCircle02Icon, "CheckCircle");
export const ChevronDown = wrap(ArrowDown01Icon, "ChevronDown");
export const ChevronLeft = wrap(ArrowLeft01Icon, "ChevronLeft");
export const ChevronRight = wrap(ArrowRight01Icon, "ChevronRight");
export const ChevronUp = wrap(ArrowUp01Icon, "ChevronUp");
export const ChevronsUpDown = wrap(UnfoldMoreIcon, "ChevronsUpDown");
export const CircleAlert = wrap(AlertCircleIcon, "CircleAlert");
export const CircleDashed = wrap(CircleDashedIcon, "CircleDashed");
export const CircleDot = wrap(CircleDotIcon, "CircleDot");
export const CircleHelp = wrap(HelpCircleIcon, "CircleHelp");
export const CircleX = wrap(CancelCircleIcon, "CircleX");
export const Clock = wrap(Clock01Icon, "Clock");
export const Cloud = wrap(CloudIcon, "Cloud");
export const Code = wrap(CodeXmlIcon, "Code");
export const Copy = wrap(Copy01Icon, "Copy");
export const CornerDownRight = wrap(ArrowTurnForwardIcon, "CornerDownRight");
export const ExternalLink = wrap(LinkSquare02Icon, "ExternalLink");
export const File = wrap(File01Icon, "File");
export const FileDiff = wrap(FileDiffIcon, "FileDiff");
export const FileScript = wrap(FileScriptIcon, "FileScript");
export const FoldVertical = wrap(FoldVerticalIcon, "FoldVertical");
export const UnfoldVertical = wrap(UnfoldVerticalIcon, "UnfoldVertical");
export const Folder = wrap(Folder01Icon, "Folder");
export const FolderOpen = wrap(FolderOpenIcon, "FolderOpen");
export const Eye = wrap(ViewIcon, "Eye");
export const GitBranch = wrap(GitBranchIcon, "GitBranch");
export const ImagePlus = wrap(ImageAdd01Icon, "ImagePlus");
export const Link = wrap(Link01Icon, "Link");
export const ListBullet = wrap(LeftToRightListBulletIcon, "ListBullet");
export const ListFilter = wrap(FilterIcon, "ListFilter");
export const LoaderCircle = wrap(Loading03Icon, "LoaderCircle");
export const Layers = wrap(Layers01Icon, "Layers");
export const MessageSquare = wrap(Comment01Icon, "MessageSquare");
export const MessageSquarePlus = wrap(CommentAdd01Icon, "MessageSquarePlus");
export const Minus = wrap(MinusSignIcon, "Minus");
export const MoreHorizontal = wrap(MoreHorizontalIcon, "MoreHorizontal");
export const Paperclip = wrap(Attachment01Icon, "Paperclip");
export const Pause = wrap(PauseIcon, "Pause");
export const Plus = wrap(Add01Icon, "Plus");
export const RefreshCw = wrap(Refresh01Icon, "RefreshCw");
export const Search = wrap(Search01Icon, "Search");
export const Settings = wrap(Settings01Icon, "Settings");
export const Shield = wrap(ShieldAlertIcon, "Shield");
export const SlidersHorizontal = wrap(PreferenceHorizontalIcon, "SlidersHorizontal");
export const Sparkles = wrap(SparklesIcon, "Sparkles");
export const Square = wrap(SquareIcon, "Square");
export const Star = wrap(StarIcon, "Star");
export const Terminal = wrap(ComputerTerminal01Icon, "Terminal");
export const Trash2 = wrap(Delete02Icon, "Trash2");
export const Wrench = wrap(Wrench01Icon, "Wrench");
export const X = wrap(Cancel01Icon, "X");
export const Pencil = wrap(PencilEdit02Icon, "Pencil");
export const Gauge = wrap(GaugeIcon, "Gauge");
export const ChartBreakoutSquare = wrap(ChartBreakoutSquareIcon, "ChartBreakoutSquare");
export const Lock = wrap(SquareLock02Icon, "Lock");
export const Globe = wrap(GlobeIcon, "Globe");
export const Download = wrap(DownloadIcon, "Download");
