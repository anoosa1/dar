export function AppSwitcher({ active }: { active: "files" | "contacts" | "calendar"|"notes"|"tasks" }) {
  return <label className="app-switcher"><span className="sr-only">Application</span><select aria-label="Application" value={active} onChange={event => { location.href = event.target.value === "files" ? "/" : `/${event.target.value}`; }}><option value="files">Files</option><option value="contacts">Contacts</option><option value="calendar">Calendar</option><option value="notes">Notes</option><option value="tasks">Tasks</option></select></label>;
}
