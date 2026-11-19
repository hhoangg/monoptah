import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  Add01Icon,
  AddSquareIcon,
  AlertCircleIcon,
  Archive01Icon,
  ArrowDown01Icon,
  ArrowRight01Icon,
  ArrowUp02Icon,
  BotIcon,
  BubbleChatIcon,
  Cancel01Icon,
  ChartBreakoutSquareIcon,
  CheckmarkCircle02Icon,
  CircleIcon,
  CloudIcon,
  ComputerIcon,
  ComputerRemoveIcon,
  ComputerTerminal01Icon,
  Copy01Icon,
  DarkModeIcon,
  Delete02Icon,
  Download01Icon,
  PrinterIcon,
  File01Icon,
  FileScriptIcon,
  FilterHorizontalIcon,
  FingerPrintIcon,
  FlashIcon,
  FullScreenIcon,
  Folder01Icon,
  GitBranchIcon,
  Image01Icon,
  InboxIcon,
  InformationCircleIcon,
  Key01Icon,
  Layers01Icon,
  LaptopIcon,
  LayoutAlignRightIcon,
  LayoutTwoColumnIcon,
  LayoutTwoRowIcon,
  LeftToRightListBulletIcon,
  Link01Icon,
  LinkSquare01Icon,
  Loading03Icon,
  LockIcon,
  Logout01Icon,
  Mic01Icon,
  MinimizeScreenIcon,
  MinusSignIcon,
  Moon02Icon,
  MoreHorizontalIcon,
  MoreVerticalIcon,
  Note01Icon,
  Notification01Icon,
  PencilEdit01Icon,
  PencilEdit02Icon,
  QrCodeIcon,
  Refresh01Icon,
  Robot01Icon,
  Search01Icon,
  Settings01Icon,
  Share01Icon,
  SmartPhone01Icon,
  Shield01Icon,
  SourceCodeIcon,
  SparklesIcon,
  Sun03Icon,
  UserCircleIcon,
  TextWrapIcon,
  Tick02Icon,
  UnfoldMoreIcon,
  UserAdd01Icon,
  UserGroupIcon,
  UserShield01Icon,
  ViewIcon,
  Wrench01Icon,
} from "@hugeicons/core-free-icons";

// Logical icon names of design-system.md section 7 (the first block is copied from the mobile app's
// ui/Icon.tsx mapping; the web-only block follows). Add new names here, never import Hugeicons
// exports directly in a screen.
export const ICONS = {
  sidebar: LayoutAlignRightIcon,
  more: MoreHorizontalIcon,
  search: Search01Icon,
  close: Cancel01Icon,
  add: Add01Icon,
  compose: PencilEdit02Icon,
  chat: BubbleChatIcon,
  settings: Settings01Icon,
  folder: Folder01Icon,
  file: File01Icon,
  send: ArrowUp02Icon,
  mic: Mic01Icon,
  chevronDown: ArrowDown01Icon,
  chevronUpDown: UnfoldMoreIcon,
  chevronRight: ArrowRight01Icon,
  branch: GitBranchIcon,
  cloud: CloudIcon,
  computer: ComputerIcon,
  computerOff: ComputerRemoveIcon,
  sparkle: SparklesIcon,
  terminal: ComputerTerminal01Icon,
  list: LeftToRightListBulletIcon,
  explorer: FileScriptIcon,
  source: SourceCodeIcon,
  preview: ViewIcon,
  wrap: TextWrapIcon,
  edit: PencilEdit01Icon,
  copy: Copy01Icon,
  trash: Delete02Icon,
  check: Tick02Icon,
  refresh: Refresh01Icon,
  metrics: ChartBreakoutSquareIcon,
  inbox: InboxIcon,
  image: Image01Icon,
  notifications: Notification01Icon,
  note: Note01Icon,
  automation: FlashIcon,
  bot: BotIcon,
  lock: LockIcon,
  shield: Shield01Icon,
  filter: FilterHorizontalIcon,
  appearance: DarkModeIcon,
  sun: Sun03Icon,
  moon: Moon02Icon,
  info: InformationCircleIcon,
  archive: Archive01Icon,
  people: UserGroupIcon,
  layers: Layers01Icon,
  circle: CircleIcon,
  robot: Robot01Icon,
  wrench: Wrench01Icon,
  minus: MinusSignIcon,
  key: Key01Icon,
  qrCode: QrCodeIcon,
  userAdd: UserAdd01Icon,
  logout: Logout01Icon,
  download: Download01Icon,
  print: PrinterIcon,
  link: Link01Icon,
  admin: UserShield01Icon,
  externalLink: LinkSquare01Icon,
  alert: AlertCircleIcon,
  fingerprint: FingerPrintIcon,
  spinner: Loading03Icon,
  share: Share01Icon,
  addSquare: AddSquareIcon,
  moreVertical: MoreVerticalIcon,
  smartphone: SmartPhone01Icon,
  laptop: LaptopIcon,
  success: CheckmarkCircle02Icon,
  account: UserCircleIcon,
  splitRight: LayoutTwoColumnIcon,
  splitDown: LayoutTwoRowIcon,
  fullscreen: FullScreenIcon,
  exitFullscreen: MinimizeScreenIcon,
} as const satisfies Record<string, IconSvgElement>;

export type IconName = keyof typeof ICONS;

export interface IconProps {
  name: IconName;
  /** Pixels. In use: 22 menu leading, 20 default, 18 settings rows, 16 fold marks, 14 inline. */
  size?: number;
  strokeWidth?: number;
  className?: string;
  /** Icon-only buttons carry the label on the button; set this only for a standalone icon. */
  "aria-label"?: string;
}

export function Icon({ name, size = 20, strokeWidth = 1.75, className, "aria-label": label }: IconProps) {
  return (
    <HugeiconsIcon
      icon={ICONS[name]}
      size={size}
      strokeWidth={strokeWidth}
      color="currentColor"
      className={className}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      role={label ? "img" : undefined}
      data-icon={name}
    />
  );
}
