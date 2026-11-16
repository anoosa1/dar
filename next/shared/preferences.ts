export interface Preferences {
  agentSettledThreads?:{id:string;revision:number}[];
  agentPinnedThreads?:string[];agentArchivedThreads?:string[];agentSendKey?:"enter"|"modEnter";agentNotify?:boolean;agentGrouping?:"recent"|"project";agentSnoozed?:{id:string;until:number}[];
  revision:number;calendarShowCancelled?:boolean;calendarShowTasks?:boolean;appearance?:"system"|"latte"|"mocha";timezone?:string;weekStart?:0|1;contactSort?:string;hiddenCalendars?:string[];
  notesSort?:string;tasksSort?:string;filesView?:"list"|"grid";filesShowHidden?:boolean;
  filesConfirmTrash?:boolean;filesDetailsTab?:"details"|"versions"|"sharing";
  /** Files folder Photos uploads into (year/month folders inside), default "pics". */
  photosUploadFolder?:string;notesFolder?:string;notesImported?:boolean;
  newsMarkRead?:"open"|"scroll";newsOrder?:"newest"|"oldest";newsShowRead?:boolean;newsImages?:"proxy"|"block";newsRefresh?:number;newsKeepDays?:number;
  /** Books: the Files folder of the library and the reader's defaults. */
  booksFolder?:string;booksTheme?:"dar"|"paper"|"sepia"|"night";booksFont?:"serif"|"sans"|"book";booksFontSize?:number;booksLineHeight?:"tight"|"normal"|"loose";
  booksLayout?:"pages"|"scroll";booksComicDirection?:"auto"|"ltr"|"rtl";booksSort?:"recent"|"title"|"author"|"added";
  notesEditorMode?:"write"|"preview";tasksView?:"all"|"inbox"|"today"|"upcoming"|"important"|"completed";
}
