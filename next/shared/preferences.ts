export interface Preferences {
  revision:number;appearance?:"system"|"latte"|"mocha";timezone?:string;weekStart?:0|1;contactSort?:string;hiddenCalendars?:string[];
  notesSort?:string;tasksSort?:string;filesView?:"list"|"grid";filesShowHidden?:boolean;
  filesConfirmTrash?:boolean;filesDetailsTab?:"details"|"versions"|"sharing";
  notesEditorMode?:"write"|"preview";tasksView?:"all"|"inbox"|"today"|"upcoming"|"important"|"completed";
}
