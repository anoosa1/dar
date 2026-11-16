export type PimKind = "contacts" | "calendar" | "notes";
/** A subscribed calendar's sync state. The address is shown to its owner only. */
export interface PimSubscription { url: string; refreshMinutes: number; lastSyncAt: number; lastError: string }
export interface PimCollection { id: string; ownerId: string; kind: PimKind; name: string; color: string; revision: number; modifiedAt: number | string; role: "owner" | "read" | "write"; subscription?: PimSubscription }
/** How often a subscribed calendar is fetched, in minutes. */
export const calendarRefreshChoices = [15, 60, 360, 1440] as const;
export interface PimResource { id: string; collectionId: string; href: string; uid: string; content: string; projection: string; revision: number; modifiedAt: number | string }
export interface ContactValue { value: string; label: string }
export interface ContactData { displayName: string; firstName: string; lastName: string; middleName: string; prefix: string; suffix: string; organization: string; title: string; notes: string; birthday: string; emails: ContactValue[]; phones: ContactValue[]; urls: ContactValue[]; addresses: { label: string; poBox?: string; extended?: string; street: string; city: string; region: string; postalCode: string; country: string }[]; groups: string[]; favorite: boolean }
export interface EventData { title: string; start: string; end: string; timezone: string; allDay: boolean; description: string; location: string; recurrence: string; recurring?: boolean; attendees: string[]; reminderMinutes: number | null; status: string }
/** `task` marks a dated VTODO shown in Calendar at its due time (or start when it has no due). */
export interface Occurrence extends EventData { resourceId: string; collectionId: string; href: string; revision: number; recurrenceId: string; startAt: string; endAt: string; task?: boolean; priority?: number }
