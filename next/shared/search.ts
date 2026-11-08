export type SearchGroup="Files"|"Contacts"|"Calendar"|"Notes"|"Tasks"|"Applications";
export interface SearchResult {id:string;group:SearchGroup;title:string;detail:string;href:string}
export interface SearchResults {items:SearchResult[];truncated:boolean}
