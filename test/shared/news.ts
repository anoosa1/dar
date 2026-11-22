/** News (RSS/Atom/JSON Feed) shared types. IDs are integers so Nextcloud News clients can sync. */
export interface NewsFolder {id:number;name:string;opened:boolean}
export interface NewsFeed {id:number;url:string;title:string;link:string;folderId:number;added:number;ordering:0|1|2;pinned:boolean;fullText:boolean;paused:boolean;
  unreadCount:number;lastUpdated:number;nextUpdate:number;updateErrorCount:number;lastUpdateError:string}
export interface NewsItem {id:number;feedId:number;guidHash:string;url:string;title:string;author:string;pubDate:number;excerpt:string;
  enclosureMime:string;enclosureLink:string;mediaThumbnail:string;unread:boolean;starred:boolean;lastModified:number;position:number}
export interface NewsItemDetail extends NewsItem {guid:string;body:string;updatedDate:number;mediaDescription:string;rtl:boolean;fingerprint:string;contentHash:string}
export interface NewsOverview {folders:NewsFolder[];feeds:NewsFeed[];starredCount:number;unreadCount:number;newestItemId:number}
export type NewsScope={type:"all"}|{type:"unread"}|{type:"starred"}|{type:"feed";id:number}|{type:"folder";id:number};
export interface NewsPage {items:NewsItem[];nextOffset:number|null}
/** A parsed entry as stored by the fetcher. Bodies are sanitized HTML with absolute URLs. */
export interface NewsEntry {guid:string;guidHash:string;url:string;title:string;author:string;pubDate:number;updatedDate:number;body:string;
  enclosureMime:string;enclosureLink:string;mediaThumbnail:string;mediaDescription:string;rtl:boolean;fingerprint:string;contentHash:string;excerpt:string}
export interface NewsDiscovered {url:string;title:string;kind:"RSS"|"Atom"|"JSON Feed";items:{title:string;pubDate:number}[]}

export const newsLimits={folders:100,feeds:500,itemsPerFeed:1000,itemsPerAccount:60000,page:100,title:1000,body:200*1024,url:2048} as const;
export const newsRefreshChoices=[15,30,60,180,720,1440] as const;
export const newsKeepChoices=[7,30,90,365] as const;
export interface NewsPreferences {newsMarkRead:"open"|"scroll";newsOrder:"newest"|"oldest";newsShowRead:boolean;newsImages:"proxy"|"block";newsRefresh:typeof newsRefreshChoices[number];newsKeepDays:typeof newsKeepChoices[number]}
export const newsDefaults:NewsPreferences={newsMarkRead:"open",newsOrder:"newest",newsShowRead:false,newsImages:"proxy",newsRefresh:30,newsKeepDays:30};
/** Reverse-ordered fixed-width keys make newest-first range scans cheap. */
export const newsKey=(id:number)=>String(4294967295-id).padStart(10,"0");
