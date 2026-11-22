/** A Files identity. Never a host path or a physical blob location. */
export interface DarFileSelection {id:string;name:string;kind:"file"|"folder"}
export interface DarFilePickerPage {
  folder:(DarFileSelection&{parentId:string})|null;
  ancestors:{id:string;name:string}[];
  items:DarFileSelection[];
  nextCursor:string|null;
}
