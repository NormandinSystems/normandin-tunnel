// A small set of line icons, drawn here so the app ships no icon font and
// loads nothing from the network.

import type { ReactNode, SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function make(children: ReactNode) {
  return function Icon({ size = 20, ...rest }: IconProps) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8}
        strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false" {...rest}>
        {children}
      </svg>
    );
  };
}

export const HomeIcon = make(<><path d="M3 11.5 12 4l9 7.5" /><path d="M5.5 10v9.5h13V10" /><path d="M10 19.5v-5h4v5" /></>);
export const SessionIcon = make(<><path d="M4 16V11a8 8 0 0 1 16 0v5" /><path d="M8.5 16v-4.5a3.5 3.5 0 0 1 7 0V16" /><path d="M3 19.5h18" /></>);
export const HistoryIcon = make(<><path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1" /><path d="M3.5 4.5v4h4" /><path d="M12 7.5V12l3 2" /></>);
export const SettingsIcon = make(<><circle cx="12" cy="12" r="3" /><path d="M19.4 14.6a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.9 2.9l-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.3a2 2 0 1 1-4 0v-.2a1.7 1.7 0 0 0-1.100-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.9-2.9l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.6-1H2.900a2 2 0 1 1 0-4h.2a1.7 1.7 0 0 0 1.6-1.100 1.700 1.700 0 0 0-.3-1.900l-.1-.1a2 2 0 1 1 2.900-2.900l.1.1a1.700 1.700 0 0 0 1.900.3h.1a1.700 1.700 0 0 0 1-1.600V2.900a2 2 0 1 1 4 0v.2a1.700 1.700 0 0 0 1 1.600 1.700 1.700 0 0 0 1.900-.3l.1-.1a2 2 0 1 1 2.900 2.900l-.1.1a1.700 1.700 0 0 0-.3 1.900v.1a1.700 1.700 0 0 0 1.600 1h.3a2 2 0 1 1 0 4h-.2a1.700 1.700 0 0 0-1.600 1Z" /></>);
export const ShowCodeIcon = make(<><rect x="3.500" y="5" width="17" height="11.500" rx="2" /><path d="M8.500 20h7" /><path d="M12 16.500V20" /><path d="M7.500 9.500h1.500M11.250 9.500h1.500M15 9.500h1.500M7.500 12.500h9" /></>);
export const EnterCodeIcon = make(<><rect x="3" y="8" width="18" height="9" rx="2" /><path d="M7 12.500h.010M10.500 12.500h.010M14 12.500h.010" strokeWidth={2.600} /><path d="M17.500 11v3" /></>);
export const CopyIcon = make(<><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15H4.500A1.500 1.500 0 0 1 3 13.500v-9A1.500 1.500 0 0 1 4.500 3h9A1.500 1.500 0 0 1 15 4.500V5" /></>);
export const CheckIcon = make(<path d="m5 12.500 4.500 4.500L19 7.500" />);
export const XIcon = make(<path d="M6 6l12 12M18 6 6 18" />);
export const ShieldIcon = make(<><path d="M12 3 5 5.500v6c0 4.500 3 7.500 7 9.500 4-2 7-5 7-9.500v-6L12 3Z" /><path d="m9 12 2.200 2.200L15 10.400" /></>);
export const AlertIcon = make(<><path d="M12 4 2.800 19.500h18.400L12 4Z" /><path d="M12 10v4.500" /><path d="M12 17.200h.010" strokeWidth={2.400} /></>);
export const FileIcon = make(<><path d="M13.500 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.500L13.500 3Z" /><path d="M13.500 3v5.500H19" /></>);
export const FolderIcon = make(<path d="M3 7a2 2 0 0 1 2-2h4.200l2 2.500H19a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z" />);
export const UploadIcon = make(<><path d="M12 16V5" /><path d="m7.500 9.500 4.500-4.500 4.500 4.500" /><path d="M5 19h14" /></>);
export const DownloadIcon = make(<><path d="M12 5v11" /><path d="m7.500 11.500 4.500 4.500 4.500-4.500" /><path d="M5 19h14" /></>);
export const PauseIcon = make(<path d="M8.500 5.500v13M15.500 5.500v13" />);
export const PlayIcon = make(<path d="M7.500 5.500v13l11-6.500-11-6.500Z" />);
export const SendIcon = make(<><path d="M20.500 3.500 3.500 10.200l6.500 2.800 2.800 6.500 7.700-16Z" /><path d="m10 13 4-4" /></>);
export const DirectIcon = make(<><circle cx="5.500" cy="12" r="2.500" /><circle cx="18.500" cy="12" r="2.500" /><path d="M8 12h8" /></>);
export const RelayIcon = make(<><circle cx="5" cy="17" r="2.300" /><circle cx="19" cy="17" r="2.300" /><circle cx="12" cy="6.500" r="2.300" /><path d="m6.300 15 4.400-6.500M17.700 15l-4.400-6.500" /></>);
export const DesktopIcon = make(<><rect x="3" y="4.500" width="18" height="12" rx="2" /><path d="M8.500 20h7M12 16.500V20" /></>);
export const PhoneIcon = make(<><rect x="7" y="3" width="10" height="18" rx="2.200" /><path d="M11 17.800h2" /></>);
export const ServerIcon = make(<><rect x="4" y="4" width="16" height="6.500" rx="1.500" /><rect x="4" y="13.500" width="16" height="6.500" rx="1.500" /><path d="M7.500 7.250h.010M7.500 16.750h.010" strokeWidth={2.400} /></>);
export const ClockIcon = make(<><circle cx="12" cy="12" r="8.500" /><path d="M12 7.500V12l3 2" /></>);
export const TextIcon = make(<path d="M5 6.500h14M5 11h14M5 15.500h9" />);
export const ChevronIcon = make(<path d="m9 6 6 6-6 6" />);
export const ArrowLeftIcon = make(<path d="M19 12H5M11 6l-6 6 6 6" />);
export const LockIcon = make(<><rect x="5" y="10.500" width="14" height="9.500" rx="2" /><path d="M8.500 10.500V8a3.500 3.500 0 0 1 7 0v2.500" /></>);
export const RefreshIcon = make(<><path d="M20 12a8 8 0 1 1-2.400-5.700" /><path d="M20 4.500v4.500h-4.500" /></>);
export const LinkIcon = make(<><path d="M10 14a4 4 0 0 0 5.700 0l3-3a4 4 0 0 0-5.700-5.700l-1 1" /><path d="M14 10a4 4 0 0 0-5.700 0l-3 3a4 4 0 0 0 5.700 5.700l1-1" /></>);

export function DeviceIcon({ type, size }: { type: string; size?: number }) {
  if (type === "phone" || type === "tablet") return <PhoneIcon size={size} />;
  if (type === "server") return <ServerIcon size={size} />;
  return <DesktopIcon size={size} />;
}

/** The app mark: a tunnel mouth. */
export function Logo({ size = 32 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" focusable="false">
      <rect width="32" height="32" rx="8" className="logo-bg" />
      <path d="M7 23.500V16a9 9 0 0 1 18 0v7.500" fill="none" className="logo-arc" strokeWidth="2.400" strokeLinecap="round" />
      <path d="M11.500 23.500V16.300a4.500 4.500 0 0 1 9 0v7.200" fill="none" className="logo-arc" strokeWidth="2" strokeLinecap="round" opacity=".55" />
      <path d="M6 24.200h20" className="logo-arc" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
