export type SearchGroup="Photos"|"Files"|"Contacts"|"Calendar"|"Notes"|"Tasks"|"News"|"Applications";
/** `thumbnail` is a same-origin image URL (Photos results). */
export interface SearchResult {id:string;group:SearchGroup;title:string;detail:string;href:string;thumbnail?:string}
export interface SearchResults {items:SearchResult[];truncated:boolean}
