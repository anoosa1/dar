export type SearchGroup="Photos"|"Files"|"Contacts"|"Calendar"|"Notes"|"Tasks"|"News"|"Chat"|"Applications";
/** `thumbnail` is a same-origin image URL (Photos results). */
export interface SearchResult {id:string;group:SearchGroup;title:string;detail:string;href:string;thumbnail?:string}
export interface SearchResults {items:SearchResult[];truncated:boolean}
/** Files whose text the search reads: Markdown and other plain text, up to textIndexBytes. */
export const textIndexBytes=2*1024*1024;
/** At most this much of a file's text is kept for search. */
export const textIndexChars=256*1024;
const textExtensions=/\.(md|markdown|txt|text|csv|tsv|json|ya?ml|toml|ini|cfg|conf|log|xml|html?|css|js|ts|tsx|jsx|py|go|rs|sh|sql|tex|org|rst|adoc|vcf|ics)$/i;
export function isTextFile(name:string,mediaType:string){
  return mediaType.startsWith("text/")||/^application\/(json|xml|x-yaml|yaml|toml|javascript|x-sh|sql)$/.test(mediaType)||textExtensions.test(name);
}
