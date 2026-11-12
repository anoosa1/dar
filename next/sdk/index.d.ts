import type {ReactNode} from "react";
export {useState,useEffect,useMemo,useRef,useCallback,createElement,Fragment} from "react";
export {jsx,jsxs} from "react/jsx-runtime";
export type {JSX} from "react/jsx-runtime";
export {createRoot} from "react-dom/client";
export interface AppRecord<T=unknown>{key:string;revision:number;schemaVersion:number;value:T;modifiedAt:number}
export interface AppHost {
 id:string;session:{user:{id:string;username:string;displayName:string;administrator:boolean};csrfToken:string};signal:AbortSignal;onError:(error:unknown)=>void;
 data:{get:<T>(key:string)=>Promise<AppRecord<T>|null>;put:<T>(key:string,value:T,revision:number,schemaVersion?:number)=>Promise<AppRecord<T>>;delete:(key:string,revision:number)=>Promise<void>;list:(cursor?:string)=>Promise<{items:AppRecord[];cursor:string|null}>};
 regional:()=>Promise<{timezone?:string}>;
}
export function AppWorkspace(props:{host:AppHost;query:string;onQuery:(value:string)=>void;sidebar:ReactNode;children:ReactNode;details?:ReactNode;searchLabel?:string;onSettings?:()=>void}):ReactNode;
export function EditorPane(props:{title:string;onClose:()=>void;dirty?:boolean;children:ReactNode}):ReactNode;
export function Dialog(props:{title:string;onClose:()=>void;dirty?:boolean;children:ReactNode}):ReactNode;
export function Select(props:import("react").SelectHTMLAttributes<HTMLSelectElement>):ReactNode;
export function Icon(props:{name:"settings"|"close"|"back"|"chevron"|"clock"|"plus"|"check"}):ReactNode;
