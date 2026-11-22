import {Icon,type IconName} from "./icons.tsx";

export type FileType="folder"|"doc"|"sheet"|"image"|"pdf"|"code"|"archive"|"audio"|"video"|"other";
const labels:Record<FileType,string>={folder:"Folder",doc:"Document",sheet:"Spreadsheet",image:"Image",pdf:"PDF",code:"Code",archive:"Archive",audio:"Audio",video:"Video",other:"File"};
const icons:Record<FileType,IconName>={folder:"folder",doc:"file",sheet:"sheet",image:"image",pdf:"pdf",code:"code",archive:"archive2",audio:"media",video:"video",other:"file"};
const extensions:[FileType,RegExp][]=[
  ["sheet",/\.(csv|tsv|xlsx?|ods|numbers)$/i],["pdf",/\.pdf$/i],["image",/\.(png|jpe?g|gif|webp|svg|heic|avif|bmp|tiff?)$/i],
  ["archive",/\.(zip|tar|gz|tgz|bz2|xz|7z|rar|zst)$/i],["audio",/\.(mp3|m4a|ogg|oga|flac|wav|opus|aac)$/i],["video",/\.(mp4|m4v|mov|webm|mkv|avi)$/i],
  ["code",/\.(json|ya?ml|toml|xml|html?|css|[cm]?[jt]sx?|py|rb|go|rs|java|c|h|cpp|sh|nix|sql|ini|conf)$/i],["doc",/\.(md|markdown|txt|rtf|docx?|odt|pages|tex|org)$/i],
];
/** Classifies an entry by kind, media type and then extension. */
export function fileType(entry:{kind:string;name:string;mediaType:string}):FileType{
  if(entry.kind==="folder")return "folder";
  const media=entry.mediaType;
  if(/^image\//.test(media))return "image";if(/^audio\//.test(media))return "audio";if(/^video\//.test(media))return "video";if(media==="application/pdf")return "pdf";
  if(/(zip|x-tar|gzip|x-7z|x-rar|x-xz|x-bzip)/.test(media))return "archive";if(/(csv|spreadsheet|ms-excel)/.test(media))return "sheet";
  return extensions.find(([,pattern])=>pattern.test(entry.name))?.[0]??(/^text\//.test(media)?"doc":/(json|xml|javascript)/.test(media)?"code":"other");
}
export function fileTypeLabel(entry:{kind:string;name:string;mediaType:string}){const type=fileType(entry);return type==="doc"&&/\.(md|markdown)$/i.test(entry.name)?"Markdown":type==="doc"&&/\.txt$/i.test(entry.name)?"Text":labels[type];}
/** A tinted type tile; the glyph differs per type so color is never the only signal. */
export function FileTile({entry,large=false}:{entry:{kind:string;name:string;mediaType:string};large?:boolean}){
  const type=fileType(entry);
  return <span className={`file-tile file-tile-${type}${large?" is-large":""}`} aria-hidden="true"><Icon name={icons[type]}/></span>;
}
