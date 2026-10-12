import { writable } from "svelte/store";

/** The settings drawer's open tab, or null while closed. Any view can open it. */
export type DrawerTab = "categories" | "repositories" | "storage" | "guide";
export const drawer = writable<DrawerTab | null>(null);
