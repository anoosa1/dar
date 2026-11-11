import type { CSSProperties } from "react";
const paths = {
  agent:"m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z",
  user:"M20 21v-2a6 6 0 0 0-6-6h-4a6 6 0 0 0-6 6v2M16 6a4 4 0 1 1-8 0 4 4 0 0 1 8 0",
  contacts:"M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M13 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0M17 4a4 4 0 0 1 0 8M22 21v-2a4 4 0 0 0-3-3.87",
  calendar:"M8 2v4M16 2v4M3 10h18M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2M8 14h2M14 14h2M8 18h2",
  palette:"M12 3a9 9 0 1 0 0 18h1a2 2 0 0 0 1.5-3.3 1.5 1.5 0 0 1 1-2.7H18a3 3 0 0 0 3-3 9 9 0 0 0-9-9ZM7.5 9h.01M10 6h.01M15 6h.01M17.5 9h.01",
  globe:"M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M3 12h18M12 3a18 18 0 0 1 0 18 18 18 0 0 1 0-18",
  key:"M15 7a4 4 0 1 1 0 8 4 4 0 0 1 0-8ZM12 14l-8 8H2v-4l3-3h3l2-2",
  plug:"M8 2v5M16 2v5M6 7h12v3a6 6 0 0 1-12 0ZM12 16v6",
  settings:"M10 3h4l.6 3 2 .9 2.7-1 2 3.4-2.2 2v2.4l2.2 2-2 3.4-2.7-1-2 .9-.6 3h-4l-.6-3-2-.9-2.7 1-2-3.4 2.2-2v-2.4l-2.2-2 2-3.4 2.7 1 2-.9ZM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
  folder: "M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z",
  file: "M6 3h8l4 4v14H6ZM14 3v5h4M9 12h6M9 16h6",
  search: "M16 16l5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
  plus: "M12 4v16M4 12h16", close: "M6 6l12 12M6 18 18 6",
  menu: "M4 6h16M4 12h16M4 18h16", more: "M12 5h.01M12 12h.01M12 19h.01",
  star: "m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2-5.6-3-5.6 3 1.1-6.2L3 9.6l6.2-.9Z",
  trash: "M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7",
  share: "M8 11l8-5M8 13l8 5M8 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0M22 4a3 3 0 1 1-6 0 3 3 0 0 1 6 0M22 20a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
  upload: "M12 16V3m-5 5 5-5 5 5M4 16v5h16v-5",
  send: "M12 20V4m-6 6 6-6 6 6",
  copy: "M9 8h12v13H9ZM15 5V2H3v13h3",
  download: "M12 3v13m-5-5 5 5 5-5M4 17v4h16v-4",
  grid: "M3 3h7v7H3ZM14 3h7v7h-7ZM3 14h7v7H3ZM14 14h7v7h-7Z",
  list: "M8 5h13M8 12h13M8 19h13M3 5h.01M3 12h.01M3 19h.01",
  chevron: "m9 5 7 7-7 7", back: "m14 5-7 7 7 7",
  check: "m5 12 4 4L19 6", clock: "M12 7v5l3 2M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
  info: "M12 11v6M12 7h.01M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
  terminal: "M4 6l6 6-6 6M12 19h8", pencil: "M4 20h4L20 8l-4-4L4 16ZM13 7l4 4",
  undo: "M9 14 4 9l5-5M4 9h11a5 5 0 0 1 0 10h-4", pin: "M9 3h6l-1 6 3 3H7l3-3ZM12 12v9", archive: "M3 4h18v4H3ZM5 8v12h14V8M10 12h4", panel: "M3 4h18v16H3ZM15 4v16", sidebar: "M3 4h18v16H3ZM9 4v16",
  lock: "M5 10h14v11H5ZM8 10V7a4 4 0 0 1 8 0v3", refresh: "M20 7V2m0 5h-5M4 17v5m0-5h5M20 7A9 9 0 0 0 4 8M4 17a9 9 0 0 0 16-1",
} as const;
export type IconName = keyof typeof paths;
export function Icon({ name, className = "" }: { name: IconName; className?: string }) {
  return <svg className={`icon ${className}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}
