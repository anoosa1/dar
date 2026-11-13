export type SearchGroup="Files"|"Contacts"|"Calendar"|"Notes"|"Tasks"|"News"|"Applications";
export interface SearchResult {id:string;group:SearchGroup;title:string;detail:string;href:string}
export interface SearchResults {items:SearchResult[];truncated:boolean}
