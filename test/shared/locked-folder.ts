/**
 * Each account has one Locked folder in Files. It sits outside the folder tree (no
 * parent listing or WebDAV path reaches it), and its contents need a recent account
 * password confirmation on this browser session. Search, sharing, public links, the
 * file picker and other apps never see it.
 */
export const lockedFolderType="inode/x-dar-locked";
export const lockedFolderName="Locked folder";
/** How long one password confirmation keeps a browser session unlocked. */
export const lockedUnlockMinutes=10;
export interface LockedFolderState {folderId:string;unlockedUntil:number}
