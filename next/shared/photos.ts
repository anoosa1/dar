/** Photos indexes image and video files in Files; these are its browser-facing shapes. */
export type PhotoView="library"|"favorites"|"videos"|"archive"|"album";
export interface PhotoExif {make?:string;model?:string;lens?:string;fNumber?:number;exposure?:number;iso?:number;focalLength?:number;orientation?:number}
export interface PhotoAsset {
  id:string;name:string;kind:"image"|"video";mediaType:string;size:number;takenAt:number;width:number;height:number;duration:number;
  favorite:boolean;archived:boolean;thumbnail:"ready"|"pending"|"failed";revision:number;
}
export interface PhotoDetail extends PhotoAsset {description:string;exif:PhotoExif;latitude:number|null;longitude:number|null;albums:{id:string;name:string}[];parentId:string;modifiedAt:number}
export interface PhotoAlbum {id:string;name:string;description:string;count:number;coverId:string;updatedAt:number;revision:number}
export interface PhotoPage {items:PhotoAsset[];cursor:string}
export interface PhotoOverview {albums:PhotoAlbum[];counts:{library:number;favorites:number;videos:number;archive:number};indexing:boolean}
/** Facts the runtime extracts from a blob with exiftool/ffprobe. */
export interface PhotoFacts {takenAt:number;width:number;height:number;duration:number;exif:PhotoExif;latitude:number|null;longitude:number|null}
export const photoLimits={albums:500,albumItems:50000,description:2000,albumName:200,batch:500};
/** Media types Photos indexes. HEIC/HEIF and RAW previews come from libvips. */
export function photoKind(mediaType:string,name:string):"image"|"video"|undefined{
  const type=mediaType.toLowerCase(),ext=name.toLowerCase().split(".").pop()??"";
  if(type.startsWith("image/")&&type!=="image/svg+xml")return "image";
  if(type.startsWith("video/"))return "video";
  if(["jpg","jpeg","png","gif","webp","heic","heif","avif","tif","tiff","dng","cr2","cr3","nef","arw","orf","rw2","raf"].includes(ext))return "image";
  if(["mp4","mov","m4v","webm","mkv","3gp","avi"].includes(ext))return "video";
  return undefined;
}
