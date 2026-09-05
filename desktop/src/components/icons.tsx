import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Icon({ size = 18, children, ...props }: IconProps) {
  return <svg viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>{children}</svg>;
}

export const HomeIcon = (props: IconProps) => <Icon {...props}><path d="m3 11 9-8 9 8" /><path d="M5 10v10h14V10M9 20v-6h6v6" /></Icon>;
export const SearchIcon = (props: IconProps) => <Icon {...props}><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></Icon>;
export const ChartIcon = (props: IconProps) => <Icon {...props}><path d="M4 20V10h4v10M10 20V4h4v16M16 20v-7h4v7" /></Icon>;
export const DownloadIcon = (props: IconProps) => <Icon {...props}><path d="M12 3v12m-5-5 5 5 5-5" /><path d="M4 20h16" /></Icon>;
export const FolderIcon = (props: IconProps) => <Icon {...props}><path d="M3 7h7l2 2h9v10H3z" /><path d="M3 7V5h7l2 2" /></Icon>;
export const PauseIcon = (props: IconProps) => <Icon {...props}><path d="M8 5v14M16 5v14" /></Icon>;
export const PlayIcon = (props: IconProps) => <Icon {...props}><path d="m8 5 11 7-11 7z" /></Icon>;
export const TrashIcon = (props: IconProps) => <Icon {...props}><path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13" /></Icon>;
export const RetryIcon = (props: IconProps) => <Icon {...props}><path d="M20 11a8 8 0 1 0-2 5" /><path d="M20 5v6h-6" /></Icon>;
export const MoreIcon = (props: IconProps) => <Icon {...props}><circle cx="5" cy="12" r="1" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1" fill="currentColor" stroke="none" /></Icon>;
export const CheckIcon = (props: IconProps) => <Icon {...props}><path d="m5 12 4 4L19 6" /></Icon>;
export const AlertIcon = (props: IconProps) => <Icon {...props}><path d="M12 3 2.7 20h18.6z" /><path d="M12 9v4m0 3h.01" /></Icon>;
export const QueueIcon = (props: IconProps) => <Icon {...props}><path d="M8 6h13M8 12h13M8 18h13" /><circle cx="3" cy="6" r="1" fill="currentColor" stroke="none" /><circle cx="3" cy="12" r="1" fill="currentColor" stroke="none" /><circle cx="3" cy="18" r="1" fill="currentColor" stroke="none" /></Icon>;
export const ChevronLeftIcon = (props: IconProps) => <Icon {...props}><path d="m15 18-6-6 6-6" /></Icon>;
export const CloseIcon = (props: IconProps) => <Icon {...props}><path d="m6 6 12 12M18 6 6 18" /></Icon>;
export const BellIcon = (props: IconProps) => <Icon {...props}><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9" /><path d="M10 21h4" /></Icon>;
export const SettingsIcon = (props: IconProps) => <Icon {...props}><circle cx="12" cy="12" r="3" /><path d="M19 14.5a1.7 1.7 0 0 0 .3 1.9l.1.1-2.9 2.9-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6V21H9.5v-.4a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1-2.9-2.9.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.6-1H2.1V9.5h.3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9l-.1-.1 2.9-2.9.1.1a1.7 1.7 0 0 0 1.9.3 1.7 1.7 0 0 0 1-1.6V2h4v.4a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1 2.9 2.9-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.4v4h-.4a1.7 1.7 0 0 0-1.6 1Z" /></Icon>;
