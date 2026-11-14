/** Photos indexes image and video files in Files; these are its browser-facing shapes. */
export type PhotoView="library"|"favorites"|"videos"|"archive"|"album"|"trash"|"locked";
export interface PhotoExif {make?:string;model?:string;lens?:string;fNumber?:number;exposure?:number;iso?:number;focalLength?:number;orientation?:number;videoCodec?:string;audioCodec?:string}
/**
 * How a browser plays a video: the original as is, a fast remux of its H.264 stream
 * into MP4, or a full H.264 transcode (HEVC, MKV with other codecs, MPEG-4 Part 2…).
 */
export type PhotoPlayback="original"|"remux"|"transcode";
export interface PhotoAsset {
  id:string;name:string;kind:"image"|"video";mediaType:string;size:number;takenAt:number;width:number;height:number;duration:number;
  favorite:boolean;archived:boolean;thumbnail:"ready"|"pending"|"failed";revision:number;
  /** Videos only. */
  playback?:PhotoPlayback;videoCodec?:string;
  /** The motion part of a Live Photo (a video file ID). */
  live?:string;
  /** Set on the cover of a stack with two or more visible photos. */
  stack?:{id:string;count:number};
  /** Trash view only: when the file was moved to the trash. */
  trashedAt?:number;
  /** Display name of the owner when the photo is someone else's (in a shared album). */
  owner?:string;
}
export interface PhotoDetail extends PhotoAsset {checksum?:string;description:string;exif:PhotoExif;latitude:number|null;longitude:number|null;albums:{id:string;name:string}[];parentId:string;modifiedAt:number;locked:boolean;stackId:string}
/** Your access to an album: yours, or shared with you to add photos (editor) or to view. */
export type PhotoAlbumRole="owner"|"editor"|"viewer";
export interface PhotoAlbum {id:string;name:string;description:string;count:number;coverId:string;updatedAt:number;revision:number;role:PhotoAlbumRole;
  /** Set for albums shared with you. */
  owner?:{id:string;name:string};
  /** Shared with people or through a link (your albums only). */
  shared:boolean}
export interface PhotoPage {items:PhotoAsset[];cursor:string}
export interface PhotoStack {id:string;primaryId:string;items:PhotoAsset[]}
export interface PhotoDuplicateGroup {key:string;identical:boolean;items:(PhotoAsset&{path:string})[]}
export interface PhotoDuplicatePage {groups:PhotoDuplicateGroup[];cursor:string}
export interface PhotoCounts {library:number;favorites:number;videos:number;archive:number;trash:number;duplicates:number;shared:number}
export interface PhotoOverview {albums:PhotoAlbum[];sharedAlbums:PhotoAlbum[];counts:PhotoCounts;indexing:boolean;uploadFolder:string}
/** Undo information for photos moved into the Locked folder. */
export interface PhotoMoved {id:string;parentId:string}
/** Facts the runtime extracts from a blob with exiftool/ffprobe and the thumbnail. */
export interface PhotoFacts {
  takenAt:number;width:number;height:number;duration:number;exif:PhotoExif;latitude:number|null;longitude:number|null;
  /** 64-bit difference hash of the thumbnail (16 hex digits), or "". */
  phash:string;playback:PhotoPlayback|"";
  /** Apple ContentIdentifier pairing a Live Photo's still and motion parts. */
  contentId:string;
  /** Apple BurstUUID shared by a burst's frames. */
  burstId:string;
  /** SHA-1 of the file, base64 (Immich clients match backups by it). */
  sha1:string;
}
/** Raising this re-indexes every asset in the background (2: hashes, playback and pairing; 3: SHA-1 for Immich). */
export const photoIndexVersion=3;
export const photoLimits={albums:500,albumItems:50000,description:2000,albumName:200,batch:500,stackItems:200,uploadFolder:500};
export const defaultPhotoFolder="pics";
/** Motion parts of Live Photos are short; longer clips with the same name stay ordinary videos. */
export const livePhotoMaxSeconds=4;
export const rawExtensions=["dng","cr2","cr3","nef","arw","orf","rw2","raf"];
/** Media types Photos indexes. HEIC/HEIF and RAW previews come from libvips. */
export function photoKind(mediaType:string,name:string):"image"|"video"|undefined{
  const type=mediaType.toLowerCase(),ext=name.toLowerCase().split(".").pop()??"";
  if(type.startsWith("image/")&&type!=="image/svg+xml")return "image";
  if(type.startsWith("video/"))return "video";
  if(["jpg","jpeg","png","gif","webp","heic","heif","avif","tif","tiff",...rawExtensions].includes(ext))return "image";
  if(["mp4","mov","m4v","webm","mkv","3gp","avi"].includes(ext))return "video";
  return undefined;
}
/** Splits a file name into its stem and lower-case extension. */
export function nameParts(name:string){const dot=name.lastIndexOf(".");return dot>0?{stem:name.slice(0,dot),ext:name.slice(dot+1).toLowerCase()}:{stem:name,ext:""};}
/** A Files folder path like "pics" or "Media/Camera": 1–8 names, no dots-only or control characters. */
export function photoFolderParts(value:string){
  const parts=value.split("/").map(part=>part.normalize("NFC").trim()).filter(Boolean);
  if(!parts.length||parts.length>8||value.length>photoLimits.uploadFolder||parts.some(part=>part.length>255||part==="."||part===".."||/[\x00-\x1f\\]/.test(part)))return undefined;
  return parts;
}
export interface PhotoAlbumMember {userId:string;username:string;displayName:string;role:"editor"|"viewer"}
/** A shared link as its owner sees it; url is "" when this instance can no longer decrypt it. */
export interface PhotoLink {
  id:string;albumId:string;albumName:string;count:number;url:string;expiresAt:number;hasPassword:boolean;
  allowDownload:boolean;allowUpload:boolean;showMetadata:boolean;createdAt:number;revision:number;
}
export interface PhotoAlbumSharing {members:PhotoAlbumMember[];links:PhotoLink[]}
/** What a shared-link visitor sees before (needsPassword) and after any password. */
export type PhotoPublicShare={needsPassword:true}|{needsPassword:false;title:string;description:string;owner:string;count:number;album:boolean;expiresAt:number;allowDownload:boolean;allowUpload:boolean;showMetadata:boolean};
export interface PhotoPublicDetail {id:string;name:string;kind:"image"|"video";width:number;height:number;duration:number;takenAt?:number;description?:string;exif?:PhotoExif;latitude?:number|null;longitude?:number|null}
export const photoLinkLimits={links:200,members:100,passwordDays:0.5,expiryDays:3650};
/** Shared-link passwords: 4–200 characters (account passwords keep their own rules). */
export const validLinkPassword=(value:string)=>[...value].length>=4&&value.length<=200;
