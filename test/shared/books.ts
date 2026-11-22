/** Books: EPUB, PDF and comic archives kept as files in a Files folder (default docs/books). */
export const defaultBooksFolder="docs/books";
export type BookFormat="epub"|"pdf"|"cbz"|"cbr"|"cb7"|"cbt";
const extensions:Record<string,BookFormat>={epub:"epub",pdf:"pdf",cbz:"cbz",cbr:"cbr",cb7:"cb7",cbt:"cbt"};
/** The book format from a file name; hidden files and other types are not books. */
export function bookFormat(name:string):BookFormat|undefined{
  if(name.startsWith("."))return undefined;
  const ext=/\.([A-Za-z0-9]{1,8})$/.exec(name)?.[1]?.toLowerCase();
  return ext?extensions[ext]:undefined;
}
export const isComic=(format:BookFormat)=>format!=="epub"&&format!=="pdf";
export const bookFormatLabel=(format:BookFormat)=>format.toUpperCase();
/** File types other readers use that Dar recognizes but cannot open. */
export const unsupportedBook=(name:string)=>/\.(mobi|azw3?|kfx|prc|lit|fb2|djvu)$/i.test(name);

export type BookStatus=""|"want"|"reading"|"finished";
/**
 * Where a reader stopped. EPUB: spine item and fraction through it; PDF and comics:
 * page (from 0) and fraction down that page.
 */
export interface BookPosition {spine?:number;page?:number;fraction:number}
export interface BookState {fileId:string;status:BookStatus;position:BookPosition|null;progress:number;device:string;updatedAt:number;revision:number}
export interface BookMark {id:string;fileId:string;position:BookPosition;label:string;createdAt:number}

export interface BookMeta {
  title:string;authors:string[];series:string;seriesIndex:number|null;language:string;publisher:string;published:string;
  description:string;subjects:string[];pages:number;cover:boolean;rightToLeft:boolean;error:""|"drm"|"damaged"|"unsupported";
}
export interface BookItem extends BookMeta {
  id:string;parentId:string;name:string;format:BookFormat;size:number;revision:number;modifiedAt:number;
  /** Top-level folder under the Books folder ("" for books at the top). */
  library:string;path:string;state:BookState|null;
}
export interface BookLibraryFolder {id:string;name:string;count:number}
export interface BookLibrary {rootId:string;path:string;libraries:BookLibraryFolder[];books:BookItem[];truncated:boolean;indexing:number}
export interface BookTocEntry {label:string;href:string;depth:number;spine:number}
export interface EpubManifest {kind:"epub";spine:{href:string;size:number}[];toc:BookTocEntry[];rightToLeft:boolean;language:string}
export interface ComicManifest {kind:"comic";pages:number;rightToLeft:boolean}
export type BookManifest=EpubManifest|ComicManifest|{kind:"pdf"};
export interface BookDetail {state:BookState|null;marks:BookMark[];manifest:BookManifest}

/** Validated stored position; anything else reads as no position. */
export function bookPosition(value:unknown):BookPosition|null{
  if(!value||typeof value!=="object"||Array.isArray(value))return null;
  const v=value as Record<string,unknown>,keys=Object.keys(v);
  if(keys.some(key=>!["spine","page","fraction"].includes(key)))return null;
  const index=(n:unknown)=>n===undefined||(typeof n==="number"&&Number.isInteger(n)&&n>=0&&n<1_000_000);
  if(!index(v.spine)||!index(v.page)||(v.spine===undefined)===(v.page===undefined))return null;
  if(typeof v.fraction!=="number"||!(v.fraction>=0&&v.fraction<=1))return null;
  return v.spine!==undefined?{spine:v.spine as number,fraction:v.fraction}:{page:v.page as number,fraction:v.fraction};
}
export const samePosition=(a:BookPosition|null,b:BookPosition|null)=>!!a&&!!b&&a.spine===b.spine&&a.page===b.page&&Math.abs(a.fraction-b.fraction)<0.02;
