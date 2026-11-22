export interface NoteData {kind:"note";title:string;tags:string[];favorite:boolean}
export interface TaskData {kind:"task";title:string;description:string;start:string;due:string;timezone:string;allDay:boolean;priority:number;status:"NEEDS-ACTION"|"IN-PROCESS"|"COMPLETED"|"CANCELLED";progress:number;completedAt:string;categories:string[];parentUid:string;recurrence:string;recurring?:boolean;reminderMinutes:number|null}

/** Notes are Markdown files in this Files folder unless the account chooses another. */
export const defaultNotesFolder="docs/notes";
/** A note or folder in the notes library; `parentId` is the Files folder that holds it. */
export interface NotesFolder {id:string;parentId:string;name:string;path:string;modifiedAt:number}
export interface NotesFile {id:string;parentId:string;name:string;title:string;excerpt:string;size:number;revision:number;modifiedAt:number;favorite:boolean;path:string}
export interface NotesLibrary {rootId:string;path:string;folders:NotesFolder[];notes:NotesFile[];truncated:boolean;legacy:number}
export const isNoteFile=(name:string)=>/\.(md|markdown)$/i.test(name)&&!name.startsWith(".");
