import type { Database } from "../database.ts";
import type { PimResource } from "../../shared/pim.ts";
import { expand } from "./calendar.ts";
import {isTask,taskOccurrences,readTask} from "./tasks.ts";

/** Durable per-resource watermarks catch up after downtime. Edits delete the
 * previous revision's inbox entries and cursor in the same transaction. */
export async function deliverReminders(db: Database) {
  const work = await db.call<{ resource: PimResource; from: number; to: number; userId: string; timezone: string }[]>("service_pim_reminder_work");
  for (const { resource, from, to, timezone } of work) {
    try {
      const occurrences = isTask(resource.content)?["COMPLETED","CANCELLED"].includes(readTask(resource.content).data.status)?[]:taskOccurrences(resource,timezone,new Date(from),new Date(to+7*86400000)).filter(o=>!["COMPLETED","CANCELLED"].includes(o.status)):expand(resource, timezone, new Date(from), new Date(to + 7 * 86400000));
      const reminders = occurrences.flatMap(occurrence => {
        if (occurrence.reminderMinutes === null) return [];
        const dueAt = ("alarmAt"in occurrence?Number(occurrence.alarmAt):Date.parse(occurrence.startAt)) - occurrence.reminderMinutes * 60000;
        return dueAt >= from && dueAt <= to ? [{ id: occurrence.recurrenceId, dueAt, title: occurrence.title }] : [];
      });
      await db.call("service_pim_commit_reminders", [resource.id, resource.revision, from, to, JSON.stringify(reminders)]);
    } catch (error) {
      // A concurrent edit is retried from its new revision on the next tick.
      // Keep a failed cursor unchanged rather than silently losing alarms.
      if (!(error instanceof Error) || error.message !== "conflict") {
        await db.call("service_pim_defer_reminder", [resource.id, resource.revision, from]);
        console.warn("A calendar reminder resource will retry.");
      }
    }
  }
}
