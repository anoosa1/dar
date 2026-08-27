package calendar

import (
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	if err := renderPage(w, r, user.DisplayName); err != nil {
		http.Error(w, "Internal server error", 500)
	}
}

func renderPage(w io.Writer, r *http.Request, displayName string) error {
	content, _ := webui.ExternalizeLastScript(calendarHTML(), "/calendar/assets/app.js")
	return webui.RenderRequest(w, r, webui.PageData{
		Title:      "Calendar — Dar",
		CurrentApp: "calendar",
		AppBar:     server.AppBarHTML(r),
		UserMenu:   webui.UserMenuHTML(displayName),
		Content:    content,
	})
}

func handleCalendarAsset(w http.ResponseWriter, r *http.Request) {
	_, script := webui.ExternalizeLastScript(calendarHTML(), "/calendar/assets/app.js")
	webui.ServeAppJavaScript(w, r, "app.js", script)
}

func calendarHTML() string {
	return `
<div id="calendar-app" data-dar-shell class="grid h-full min-h-0 grid-cols-[minmax(0,1fr)_auto] grid-rows-[auto_minmax(0,1fr)] overflow-hidden bg-surface lg:grid-cols-[280px_minmax(0,1fr)_auto]">
	<!-- Top toolbar (Nextcloud-style) -->
	<div data-dar-pane-header class="col-start-1 row-start-1 flex items-center gap-3 px-4 py-3 border-b border-outline-variant bg-transparent shrink-0 lg:col-start-2 lg:col-span-2">
		<button type="button" onclick="goToday()" class="h-9 px-4 text-sm font-medium rounded-full border border-outline-variant hover:bg-surface-container-high">Today</button>
		<div class="flex items-center gap-1">
			<button type="button" onclick="prevPeriod()" aria-label="Previous period" class="w-9 h-9 rounded-full hover:bg-surface-container-high inline-flex items-center justify-center"><i class="fa-solid fa-chevron-left"></i></button>
			<button type="button" onclick="nextPeriod()" aria-label="Next period" class="w-9 h-9 rounded-full hover:bg-surface-container-high inline-flex items-center justify-center"><i class="fa-solid fa-chevron-right"></i></button>
		</div>
		<div data-dar-sidebar-toggle-host class="flex min-h-10 min-w-0 flex-1 items-center gap-2">
			<h1 id="period-title" class="min-w-0 truncate text-lg font-semibold"></h1>
		</div>
		<div class="hidden gap-1 bg-surface-container rounded-3xl p-1 lg:flex">
			<button onclick="switchView('day')" id="view-day" class="view-btn px-4 py-1 text-sm font-medium rounded-3xl">Day</button>
			<button onclick="switchView('week')" id="view-week" class="view-btn px-4 py-1 text-sm font-medium rounded-3xl">Week</button>
			<button onclick="switchView('month')" id="view-month" class="view-btn px-4 py-1 text-sm font-medium rounded-3xl">Month</button>
			<button onclick="switchView('year')" id="view-year" class="view-btn px-4 py-1 text-sm font-medium rounded-3xl">Year</button>
			<button onclick="switchView('list')" id="view-list" class="view-btn px-4 py-1 text-sm font-medium rounded-3xl">List</button>
		</div>
		<input id="event-filter" type="search" aria-label="Filter events" placeholder="Filter events…" oninput="onFilterChange()" class="hidden xl:block w-40 h-9 px-4 text-sm rounded-full bg-surface-container border-0 outline-none focus:ring-2 focus:ring-primary">
		<button onclick="showCreate()" class="h-9 px-5 rounded-full bg-primary text-on-primary text-sm font-medium inline-flex items-center gap-2 hover:opacity-90">
			<i class="fa-solid fa-plus"></i><span>Event</span>
		</button>
	</div>

	<div class="contents">
		<!-- Left sidebar: mini-calendar + calendars list (Nextcloud parity) -->
		<aside id="calendar-sidebar" data-dar-sidebar data-dar-sidebar-key="calendar" aria-label="Calendar navigation" class="col-start-1 row-start-1 row-span-2 flex shrink-0 border-r border-outline-variant bg-transparent flex-col">
			<div id="mini-calendar" class="p-3 border-b border-outline-variant"></div>
			<div class="p-3 flex-1 overflow-y-auto" id="calendars-list">
				<!-- Populated by JS: color checkboxes, visibility, New calendar -->
				<div class="text-xs uppercase tracking-widest text-on-surface-variant px-3 py-2">Calendars</div>
			</div>
			<div class="p-3 border-t border-outline-variant shrink-0">
				<button onclick="newCalendar()" class="w-full h-9 rounded-full text-sm border border-outline-variant hover:bg-surface-container-high flex items-center justify-center gap-2">
					<i class="fa-solid fa-plus text-xs"></i> New calendar
				</button>
				<button onclick="newSubscription()" class="w-full h-9 mt-2 rounded-full text-sm text-primary hover:bg-primary-container flex items-center justify-center gap-2">
					<i class="fa-solid fa-link text-xs"></i> Subscribe from link
				</button>
				<button type="button" onclick="openSettings()" class="w-full h-9 mt-2 rounded-full text-sm text-on-surface-variant hover:bg-surface-container-high flex items-center justify-center gap-2">
					<i class="fa-solid fa-gear text-xs"></i> Settings
				</button>
			</div>
		</aside>

		<!-- Main view area -->
		<div data-dar-master-pane class="col-start-1 row-start-2 flex-1 flex flex-col min-h-0 bg-transparent overflow-hidden lg:col-start-2" id="main-view">
			<div id="month-view" class="flex-1 flex flex-col hidden">
				<div id="month-weekday-header" class="grid grid-cols-7 text-center text-xs font-bold text-on-surface-variant py-2 border-b border-outline-variant">
					<div class="text-error">Sun</div><div>Mon</div><div>Tue</div><div>Wed</div><div>Thu</div><div>Fri</div><div>Sat</div>
				</div>
				<div id="month-grid" class="flex-1 grid grid-cols-7 border border-outline-variant overflow-hidden"></div>
			</div>
			<div id="week-view" class="flex-1 hidden flex-col min-h-0 overflow-auto"></div>
			<div id="day-view" class="flex-1 hidden flex-col min-h-0 overflow-auto"></div>
			<div id="list-view" class="flex-1 overflow-y-auto p-4 hidden"></div>
			<div id="year-view" class="flex-1 hidden overflow-auto p-4"></div>
		</div>

		<!-- Right detail pane (event details/editor) -->
		<section id="detail-pane" data-dar-detail-pane class="hidden col-start-2 row-start-2 lg:col-start-3 lg:flex relative w-80 min-w-72 shrink-0 border-l border-outline-variant bg-transparent flex-col overflow-hidden">
			<div id="detail-pane-resizer" role="separator" aria-orientation="vertical" aria-label="Resize event details" tabindex="0" class="absolute inset-y-0 left-0 z-20 w-2 cursor-col-resize touch-none hover:bg-primary/20 focus:bg-primary/30 focus:outline-none"></div>
			<div id="detail-empty" class="flex-1 flex flex-col items-center justify-center text-center p-8 text-on-surface-variant">
				<i class="fa-solid fa-calendar text-6xl opacity-20 mb-6"></i>
				<p class="text-lg">No event selected</p>
				<p class="text-sm mt-2">Click an event or create new</p>
			</div>
			<!-- Event detail content populated by JS -->
			<div id="event-detail" class="hidden min-h-0 flex-1 p-6"></div>
		</section>
	</div>

	<!-- Quick create/edit popover (Nextcloud-style simple editor) -->
	<div id="event-popover" class="hidden fixed z-50 w-[22rem] bg-surface rounded-2xl shadow-2xl border border-outline-variant flex flex-col max-h-[90vh]">
		<div class="flex items-center justify-between px-4 py-3 border-b border-outline-variant">
			<h2 id="pop-title-label" class="text-base font-semibold">New event</h2>
			<button type="button" onclick="closePopover()" class="w-8 h-8 rounded-full hover:bg-surface-container-high inline-flex items-center justify-center text-on-surface-variant"><i class="fa-solid fa-xmark"></i></button>
		</div>
		<div class="p-4 space-y-3 overflow-y-auto">
			<input id="pop-title" placeholder="Event title" class="w-full h-11 px-4 text-base rounded-2xl bg-surface-container text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary">
			<label class="flex items-center gap-2 text-sm text-on-surface-variant cursor-pointer">
				<input id="pop-all-day" type="checkbox" class="w-4 h-4 accent-primary"> All day
			</label>
			<div class="grid grid-cols-2 gap-2">
				<div>
					<label class="block text-[10px] uppercase tracking-widest text-on-surface-variant mb-1">From</label>
					<input id="pop-start-date" type="date" class="w-full h-10 px-2 text-sm rounded-xl bg-surface-container border-0">
					<input id="pop-start-time" type="time" class="w-full h-10 px-2 mt-1 text-sm rounded-xl bg-surface-container border-0">
				</div>
				<div>
					<label class="block text-[10px] uppercase tracking-widest text-on-surface-variant mb-1">To</label>
					<input id="pop-end-date" type="date" class="w-full h-10 px-2 text-sm rounded-xl bg-surface-container border-0">
					<input id="pop-end-time" type="time" class="w-full h-10 px-2 mt-1 text-sm rounded-xl bg-surface-container border-0">
				</div>
			</div>
			<div>
				<label class="block text-[10px] uppercase tracking-widest text-on-surface-variant mb-1">Calendar</label>
				<select id="pop-calendar" class="w-full h-10 px-3 text-sm rounded-xl bg-surface-container border-0 text-on-surface"></select>
			</div>
		</div>
		<div class="flex items-center gap-2 p-3 border-t border-outline-variant">
			<button type="button" onclick="moreDetailsFromPopover()" class="h-10 px-3 text-sm text-primary font-medium rounded-xl hover:bg-primary-container">More details</button>
			<div class="flex-1"></div>
			<button type="button" onclick="closePopover()" class="h-10 px-3 text-sm text-on-surface-variant font-medium rounded-xl hover:bg-surface-container-high">Cancel</button>
			<button type="button" onclick="saveFromPopover()" class="h-10 px-4 text-sm bg-primary text-on-primary font-medium rounded-xl">Save</button>
		</div>
	</div>


	<!-- Full event modal -->
	<div id="event-modal" class="hidden fixed inset-0 bg-scrim/60 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeModal()">
		<div class="bg-surface rounded-3xl w-full max-w-lg shadow-2xl max-h-[90vh] flex flex-col">
			<div class="flex items-center justify-between px-6 py-4 border-b border-outline-variant">
				<h2 id="modal-title" class="text-xl font-semibold">New event</h2>
				<button type="button" onclick="closeModal()" class="text-on-surface-variant hover:text-on-surface"><i class="fa-solid fa-xmark text-2xl"></i></button>
			</div>
			<div class="flex-1 p-6 space-y-4 overflow-y-auto">
				<input id="ev-title" placeholder="Event title" class="w-full h-12 px-5 text-lg rounded-2xl bg-surface-container text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary">
				<div class="flex items-center gap-3">
					<input id="ev-all-day" type="checkbox" class="w-5 h-5 accent-primary"> <span class="text-sm text-on-surface-variant">All day</span>
				</div>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-1">Starts</label>
						<input id="ev-start-date" type="date" class="w-full h-11 px-4 rounded-2xl bg-surface-container border-0">
						<input id="ev-start-time" type="time" class="w-full h-11 px-4 mt-2 rounded-2xl bg-surface-container border-0">
					</div>
					<div>
						<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-1">Ends</label>
						<input id="ev-end-date" type="date" class="w-full h-11 px-4 rounded-2xl bg-surface-container border-0">
						<input id="ev-end-time" type="time" class="w-full h-11 px-4 mt-2 rounded-2xl bg-surface-container border-0">
					</div>
				</div>
				<div>
					<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-2">Calendar</label>
					<select id="ev-calendar" class="w-full h-11 px-4 rounded-2xl bg-surface-container border-0 text-on-surface"></select>
				</div>
				<input id="ev-location" placeholder="Add a location" class="w-full h-11 px-5 rounded-2xl bg-surface-container text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary">
				<textarea id="ev-desc" placeholder="Description or notes..." rows="3" class="w-full px-5 py-3 rounded-3xl bg-surface-container border-0 resize-y focus:ring-2 focus:ring-primary"></textarea>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-1">Status</label>
						<select id="ev-status" class="w-full h-11 px-4 rounded-2xl bg-surface-container border-0 text-on-surface">
							<option value="CONFIRMED">Confirmed</option>
							<option value="TENTATIVE">Tentative</option>
							<option value="CANCELLED">Cancelled</option>
						</select>
					</div>
					<div>
						<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-1">Show as</label>
						<select id="ev-busy" class="w-full h-11 px-4 rounded-2xl bg-surface-container border-0 text-on-surface">
							<option value="1">Busy</option>
							<option value="0">Free</option>
						</select>
					</div>
				</div>
				<div>
					<label class="block text-xs uppercase tracking-widest text-on-surface-variant mb-1">Categories</label>
					<input id="ev-categories" placeholder="work, personal (comma-separated)" class="w-full h-11 px-5 rounded-2xl bg-surface-container text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary">
				</div>
				<div class="pt-2 border-t border-outline-variant text-sm flex items-center justify-between gap-3">
					<span class="text-on-surface-variant shrink-0">Repeats</span>
					<select id="ev-repeat" class="bg-surface-container rounded-xl px-3 py-2 border-0 text-primary font-medium">
						<option value="">Does not repeat</option>
						<option value="FREQ=DAILY">Daily</option>
						<option value="FREQ=WEEKLY">Weekly</option>
						<option value="FREQ=MONTHLY">Monthly</option>
						<option value="FREQ=YEARLY">Yearly</option>
					</select>
				</div>
				<div class="text-sm flex items-center justify-between gap-3">
					<span class="text-on-surface-variant shrink-0">Reminder</span>
					<select id="ev-reminder" class="bg-surface-container rounded-xl px-3 py-2 border-0 text-primary font-medium">
						<option value="">None</option>
						<option value="0">At time of event</option>
						<option value="5">5 minutes before</option>
						<option value="10">10 minutes before</option>
						<option value="15">15 minutes before</option>
						<option value="30">30 minutes before</option>
						<option value="60">1 hour before</option>
						<option value="120">2 hours before</option>
						<option value="1440">1 day before</option>
						<option value="10080">1 week before</option>
					</select>
				</div>
			</div>
			<div class="flex gap-3 p-4 border-t border-outline-variant bg-surface rounded-b-3xl">
				<button type="button" id="modal-delete-btn" onclick="deleteCurrentEvent()" class="flex-1 h-11 text-error font-medium hover:bg-error-container rounded-2xl hidden">Delete</button>
				<button type="button" onclick="closeModal()" class="flex-1 h-11 text-on-surface-variant font-medium hover:bg-surface-container-high rounded-2xl">Cancel</button>
				<button type="button" onclick="saveEvent()" class="flex-1 h-11 bg-primary text-on-primary font-medium rounded-2xl">Save</button>
			</div>
		</div>
	</div>

	<!-- Recurrence scope chooser (this / future / all) -->
	<div id="scope-modal" class="hidden fixed inset-0 bg-scrim/60 z-[60] flex items-center justify-center p-4" onclick="if(event.target===this)closeScopeModal()">
		<div class="bg-surface rounded-3xl w-full max-w-sm shadow-2xl p-6 space-y-4">
			<h3 id="scope-title" class="text-lg font-semibold">Recurring event</h3>
			<p id="scope-desc" class="text-sm text-on-surface-variant">This event repeats. Which events do you want to change?</p>
			<div class="flex flex-col gap-2">
				<button type="button" onclick="resolveScope('this')" class="h-11 rounded-2xl border border-outline-variant hover:bg-surface-container-high font-medium text-sm">This event only</button>
				<button type="button" onclick="resolveScope('future')" class="h-11 rounded-2xl border border-outline-variant hover:bg-surface-container-high font-medium text-sm">This and future events</button>
				<button type="button" onclick="resolveScope('all')" class="h-11 rounded-2xl bg-primary text-on-primary font-medium text-sm">All events</button>
			</div>
			<button type="button" onclick="closeScopeModal()" class="w-full h-10 text-sm text-on-surface-variant">Cancel</button>
		</div>
	</div>

	<!-- Month overflow popover -->
	<div id="month-overflow-popover" role="dialog" aria-modal="false" aria-labelledby="month-overflow-title" class="hidden fixed z-50 w-72 max-w-[calc(100vw-2rem)] overflow-hidden rounded-2xl border border-outline-variant bg-surface shadow-2xl">
		<div class="flex items-center justify-between border-b border-outline-variant px-4 py-3">
			<h2 id="month-overflow-title" class="text-sm font-semibold"></h2>
			<button type="button" onclick="closeMonthOverflow()" aria-label="Close event list" class="inline-flex h-8 w-8 items-center justify-center rounded-full text-on-surface-variant hover:bg-surface-container-high"><i class="fa-solid fa-xmark"></i></button>
		</div>
		<div id="month-overflow-events" class="max-h-80 overflow-y-auto p-2"></div>
	</div>
	</div>

	<div id="calendar-menu-modal" class="hidden fixed inset-0 z-[65] bg-scrim/40 items-center justify-center p-4" role="dialog" aria-modal="true" aria-labelledby="calendar-menu-title" onclick="if(event.target===this)closeCalendarMenu()">
		<div class="w-full max-w-md rounded-3xl bg-surface shadow-2xl overflow-hidden">
			<header class="flex items-center gap-3 px-5 py-4 border-b border-outline-variant"><h2 id="calendar-menu-title" class="flex-1 text-lg font-semibold">Calendar</h2><button type="button" onclick="closeCalendarMenu()" class="w-9 h-9 rounded-full hover:bg-surface-container-high" aria-label="Close calendar menu"><i class="fa-solid fa-xmark"></i></button></header>
			<div class="p-5 space-y-4">
				<label class="block text-sm">Name<input id="calendar-menu-name" maxlength="120" class="mt-1 w-full h-11 px-4 rounded-full bg-surface-container border-0 outline-none focus:ring-2 focus:ring-primary"></label>
				<label class="block text-sm">Color<input id="calendar-menu-color" type="color" class="mt-1 block h-11 w-full rounded-xl bg-surface-container border-0 px-2"></label>
				<section id="calendar-menu-subscription" class="hidden rounded-2xl bg-surface-container-low p-4 space-y-2"><div class="text-sm font-medium">Subscribed calendar</div><p id="calendar-menu-subscription-url" class="break-all font-mono text-xs text-on-surface-variant"></p><p id="calendar-menu-sync-state" class="text-xs text-on-surface-variant" aria-live="polite"></p><button id="calendar-menu-refresh" type="button" class="h-10 px-4 rounded-full bg-secondary-container text-sm font-medium">Refresh now</button></section>
				<div id="calendar-menu-delete-confirm" class="hidden rounded-2xl bg-error-container p-4 text-sm"><p>Remove this calendar and all of its events?</p><div class="mt-3 flex justify-end gap-2"><button type="button" onclick="cancelCalendarRemove()" class="h-9 px-3 rounded-full">Cancel</button><button type="button" onclick="removeCalendar()" class="h-9 px-4 rounded-full bg-error text-on-error">Remove</button></div></div>
			</div>
			<footer class="flex items-center gap-2 border-t border-outline-variant p-4"><button id="calendar-menu-delete" type="button" class="h-10 px-4 rounded-full text-error hover:bg-error-container">Remove calendar</button><div class="flex-1"></div><button type="button" onclick="closeCalendarMenu()" class="h-10 px-4 rounded-full">Cancel</button><button id="calendar-menu-save" type="button" class="h-10 px-5 rounded-full bg-primary text-on-primary font-medium">Save</button></footer>
		</div>
	</div>

	<div id="calendar-settings-modal" class="hidden fixed inset-0 z-[70] bg-scrim/40 items-center justify-center p-4" role="dialog" aria-modal="true" aria-labelledby="calendar-settings-title" onclick="if(event.target===this)closeSettings()">
		<div class="bg-surface rounded-3xl shadow-2xl w-full max-w-4xl max-h-[90vh] flex flex-col overflow-hidden">
			<header class="flex items-center gap-3 px-5 py-4 border-b border-outline-variant">
				<h2 id="calendar-settings-title" class="text-xl font-semibold flex-1">Calendar settings</h2>
				<span id="calendar-settings-status" class="text-xs text-on-surface-variant" aria-live="polite"></span>
				<button type="button" onclick="closeSettings()" class="w-9 h-9 rounded-full hover:bg-surface-container-high" aria-label="Close Calendar settings"><i class="fa-solid fa-xmark"></i></button>
			</header>
			<div class="flex flex-1 min-h-0">
				<nav class="hidden sm:flex w-48 shrink-0 p-3 border-r border-outline-variant flex-col gap-1" aria-label="Calendar settings sections">
					<button type="button" data-calendar-settings-tab="general" class="calendar-settings-tab h-10 px-4 rounded-full text-left text-sm bg-primary-container text-primary">General</button>
					<button type="button" data-calendar-settings-tab="appearance" class="calendar-settings-tab h-10 px-4 rounded-full text-left text-sm hover:bg-surface-container-high">Appearance</button>
					<button type="button" data-calendar-settings-tab="editing" class="calendar-settings-tab h-10 px-4 rounded-full text-left text-sm hover:bg-surface-container-high">Editing</button>
					<button type="button" data-calendar-settings-tab="legend" class="calendar-settings-tab h-10 px-4 rounded-full text-left text-sm hover:bg-surface-container-high">Event legend</button>
					<button type="button" data-calendar-settings-tab="shortcuts" class="calendar-settings-tab h-10 px-4 rounded-full text-left text-sm hover:bg-surface-container-high">Keyboard shortcuts</button>
				</nav>
				<main class="flex-1 overflow-y-auto p-5 sm:p-7">
					<div class="sm:hidden mb-5"><label class="text-sm">Section<select id="calendar-settings-mobile-tab" class="mt-1 w-full h-10 px-3 rounded-xl bg-surface-container border-0"><option value="general">General</option><option value="appearance">Appearance</option><option value="editing">Editing</option><option value="legend">Event legend</option><option value="shortcuts">Keyboard shortcuts</option></select></label></div>
					<section data-calendar-settings-section="general" class="calendar-settings-section space-y-5">
						<h3 class="text-lg font-semibold">General</h3>
						<label class="block text-sm">Timezone<input id="calendar-setting-timezone" list="calendar-timezones" placeholder="automatic or America/Toronto" class="mt-1 w-full h-11 px-4 rounded-full bg-surface-container border-0 outline-none focus:ring-2 focus:ring-primary"></label>
						<datalist id="calendar-timezones"><option value="automatic"><option value="UTC"><option value="America/Toronto"><option value="America/New_York"><option value="America/Chicago"><option value="America/Denver"><option value="America/Los_Angeles"><option value="Europe/London"><option value="Europe/Paris"><option value="Asia/Tokyo"><option value="Australia/Sydney"></datalist>
						<div class="flex flex-wrap gap-2"><button type="button" onclick="chooseCalendarImport()" class="h-10 px-4 rounded-full bg-primary-container text-primary text-sm font-medium"><i class="fa-solid fa-file-import mr-2"></i>Import calendar</button><input id="calendar-import-input" type="file" accept=".ics,text/calendar" class="hidden"></div>
						<div class="rounded-2xl bg-surface-container-low p-4 space-y-3"><h4 class="font-semibold">CalDAV</h4><p class="text-sm text-on-surface-variant">Access Dar calendars from other apps and devices.</p><div class="flex gap-2"><input id="calendar-caldav-url" readonly class="min-w-0 flex-1 h-10 px-3 rounded-full bg-surface font-mono text-xs border-0"><button type="button" onclick="copyCalendarCalDAV()" class="h-10 px-4 rounded-full bg-surface-container-high text-sm">Copy</button></div></div>
					</section>
					<section data-calendar-settings-section="appearance" class="calendar-settings-section hidden space-y-4"><h3 class="text-lg font-semibold">Appearance</h3>
						<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low p-4 text-sm"><span>Birthday calendar</span><input id="calendar-setting-birthdays" type="checkbox" class="w-5 h-5 accent-primary"></label>
						<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low p-4 text-sm"><span>Weekends</span><input id="calendar-setting-weekends" type="checkbox" class="w-5 h-5 accent-primary"></label>
						<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low p-4 text-sm"><span>Week numbers</span><input id="calendar-setting-week-numbers" type="checkbox" class="w-5 h-5 accent-primary"></label>
						<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low p-4 text-sm"><span>Limit events shown in Month view</span><input id="calendar-setting-month-limit" type="checkbox" class="w-5 h-5 accent-primary"></label>
						<label class="block text-sm">Density in Day and Week View<select id="calendar-setting-slot-minutes" class="mt-1 w-full h-11 px-3 rounded-xl bg-surface-container border-0"><option value="5">5 minutes</option><option value="10">10 minutes</option><option value="15">15 minutes</option><option value="20">20 minutes</option><option value="30">30 minutes</option><option value="60">An hour</option></select></label>
					</section>
					<section data-calendar-settings-section="editing" class="calendar-settings-section hidden space-y-4"><h3 class="text-lg font-semibold">Editing</h3>
						<label class="block text-sm">Default reminder for part-day events<select id="calendar-setting-timed-reminder" class="mt-1 w-full h-11 px-3 rounded-xl bg-surface-container border-0"></select></label>
						<label class="block text-sm">Default reminder for full-day events<select id="calendar-setting-all-day-reminder" class="mt-1 w-full h-11 px-3 rounded-xl bg-surface-container border-0"></select></label>
						<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low p-4 text-sm"><span><span class="block">Simple event editor</span><span class="block text-xs text-on-surface-variant mt-1">“More details” opens the detailed editor.</span></span><input id="calendar-setting-simple-editor" type="checkbox" class="w-5 h-5 accent-primary"></label>
					</section>
					<section data-calendar-settings-section="legend" class="calendar-settings-section hidden space-y-4"><h3 class="text-lg font-semibold">Event legend</h3><p class="text-sm text-on-surface-variant">Calendar events convey additional information at a glance.</p><div class="space-y-3 text-sm"><p><i class="fa-solid fa-circle mr-2 text-primary"></i><strong>Busy</strong> — your time is marked as occupied.</p><p><i class="fa-regular fa-circle mr-2 text-primary"></i><strong>Free</strong> — your time remains available.</p><p><i class="fa-solid fa-rotate mr-2"></i><strong>Repeating</strong> — the event follows a recurrence rule.</p><p><i class="fa-solid fa-bell mr-2"></i><strong>Reminder</strong> — a notification is attached to the event.</p></div></section>
					<section data-calendar-settings-section="shortcuts" class="calendar-settings-section hidden space-y-4"><h3 class="text-lg font-semibold">Keyboard shortcuts</h3><dl class="grid grid-cols-[auto_1fr] gap-x-5 gap-y-3 text-sm"><dt class="font-mono">P / K</dt><dd>Previous period</dd><dt class="font-mono">N / J</dt><dd>Next period</dd><dt class="font-mono">T</dt><dd>Today</dd><dt class="font-mono">C</dt><dd>Create event</dd><dt class="font-mono">1–5 / D W M Y L</dt><dd>Change view</dd><dt class="font-mono">H</dt><dd>Show settings and shortcuts</dd><dt class="font-mono">Escape</dt><dd>Close the active editor or dialog</dd></dl></section>
				</main>
			</div>
			<footer class="flex justify-end gap-2 border-t border-outline-variant p-4"><button type="button" onclick="closeSettings()" class="h-10 rounded-full px-5 hover:bg-surface-container-high">Cancel</button><button id="calendar-settings-save" type="button" class="h-10 rounded-full bg-primary px-5 text-on-primary">Save</button></footer>
		</div>
	</div>
<script>
let currentView = 'month';
let currentDate = new Date();
let events = [];
let eventsByDay = new Map();
let calendars = [];
let currentEdit = null; // event id (may be master@recurrence)
let currentDetailId = null;
let currentEditMeta = null; // {master_id, recurrence_id, rrule} when editing instance
let eventFilter = '';
let timeFormat24h = false;
let dragState = null; // {id, mode:'move'|'resize', startY, origStart, origEnd, colEl}
let suppressClick = false; // true after a drag so click does not open detail
let scopeResolver = null; // Promise resolve for scope modal
let calendarMenuID = '';
let detailResizeActive = false;
let calendarSettings = {timezone:'automatic',birthday_calendar:true,show_weekends:true,show_week_numbers:false,limit_month_events:true,slot_minutes:30,default_timed_reminder:null,default_all_day_reminder:null,simple_editor:true};
const apiBase = '/api/calendar';
const HOUR_H = 48; // px per hour in week/day grid
function activeCalendarTimezone() { return calendarSettings.timezone && calendarSettings.timezone !== 'automatic' ? calendarSettings.timezone : undefined; }
function formatTime(value) { return new Intl.DateTimeFormat([], {hour:'numeric', minute:'2-digit', hour12: timeFormat24h ? false : undefined, timeZone:activeCalendarTimezone()}).format(new Date(value)); }
function zonedParts(value) {
	const parts = new Intl.DateTimeFormat('en-CA',{timeZone:activeCalendarTimezone(),year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit',hourCycle:'h23'}).formatToParts(new Date(value));
	const out={}; parts.forEach(p=>{if(p.type!=='literal')out[p.type]=Number(p.value)}); return out;
}
function eventDate(seconds) { const p=zonedParts(seconds*1000); return new Date(p.year,p.month-1,p.day,p.hour,p.minute,p.second||0); }
function calendarNow() { const p=zonedParts(Date.now()); return new Date(p.year,p.month-1,p.day,p.hour,p.minute,p.second||0); }
function timezoneOffsetAt(epochMs, zone) {
	const parts = new Intl.DateTimeFormat('en-CA',{timeZone:zone,year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit',hourCycle:'h23'}).formatToParts(new Date(epochMs));
	const p={}; parts.forEach(x=>{if(x.type!=='literal')p[x.type]=Number(x.value)});
	return Date.UTC(p.year,p.month-1,p.day,p.hour,p.minute,p.second)-epochMs;
}
function wallTimeToEpoch(dateStr,timeStr) {
	const local = dateStr+'T'+timeStr+':00';
	if (!activeCalendarTimezone()) return Math.floor(new Date(local).getTime()/1000);
	const bits=(dateStr+'-'+timeStr.replace(':','-')).split('-').map(Number); const guess=Date.UTC(bits[0],bits[1]-1,bits[2],bits[3],bits[4],0); const zone=activeCalendarTimezone(); const candidates=[];
	[guess-86400000,guess,guess+86400000].forEach(sample=>{const candidate=guess-timezoneOffsetAt(sample,zone); const p=zonedParts(candidate); if(p.year===bits[0]&&p.month===bits[1]&&p.day===bits[2]&&p.hour===bits[3]&&p.minute===bits[4])candidates.push(candidate)});
	if(!candidates.length) throw new Error('That local time does not exist in '+zone+' because of a daylight-saving transition.');
	return Math.floor(Math.min(...candidates)/1000);
}
function isoWeekNumber(date) { const d=new Date(Date.UTC(date.getFullYear(),date.getMonth(),date.getDate())); const day=d.getUTCDay()||7; d.setUTCDate(d.getUTCDate()+4-day); const y=new Date(Date.UTC(d.getUTCFullYear(),0,1)); return Math.ceil((((d-y)/86400000)+1)/7); }

async function loadCalendarSettings() {
	const r=await fetch('/api/settings/calendar'); if(!r.ok)throw new Error('Could not load Calendar settings'); calendarSettings=Object.assign(calendarSettings,await r.json()); populateCalendarSettings();
}
function populateCalendarSettings() {
	document.getElementById('calendar-setting-timezone').value=calendarSettings.timezone||'automatic';
	document.getElementById('calendar-setting-birthdays').checked=!!calendarSettings.birthday_calendar; document.getElementById('calendar-setting-weekends').checked=!!calendarSettings.show_weekends; document.getElementById('calendar-setting-week-numbers').checked=!!calendarSettings.show_week_numbers; document.getElementById('calendar-setting-month-limit').checked=!!calendarSettings.limit_month_events; document.getElementById('calendar-setting-slot-minutes').value=String(calendarSettings.slot_minutes||30); document.getElementById('calendar-setting-simple-editor').checked=!!calendarSettings.simple_editor;
	document.getElementById('calendar-setting-timed-reminder').value=calendarSettings.default_timed_reminder==null?'':String(calendarSettings.default_timed_reminder); document.getElementById('calendar-setting-all-day-reminder').value=calendarSettings.default_all_day_reminder==null?'':String(calendarSettings.default_all_day_reminder);
}
function readCalendarSettingsForm() { const reminder=id=>{const v=document.getElementById(id).value;return v===''?null:Number(v)}; return {timezone:document.getElementById('calendar-setting-timezone').value.trim()||'automatic',birthday_calendar:document.getElementById('calendar-setting-birthdays').checked,show_weekends:document.getElementById('calendar-setting-weekends').checked,show_week_numbers:document.getElementById('calendar-setting-week-numbers').checked,limit_month_events:document.getElementById('calendar-setting-month-limit').checked,slot_minutes:Number(document.getElementById('calendar-setting-slot-minutes').value),default_timed_reminder:reminder('calendar-setting-timed-reminder'),default_all_day_reminder:reminder('calendar-setting-all-day-reminder'),simple_editor:document.getElementById('calendar-setting-simple-editor').checked}; }
async function saveCalendarSettings() { const status=document.getElementById('calendar-settings-status'),button=document.getElementById('calendar-settings-save'); status.className='text-xs text-on-surface-variant'; status.textContent='Saving…'; button.disabled=true; try { const r=await fetch('/api/settings/calendar',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(readCalendarSettingsForm())}); const data=await r.json().catch(()=>({})); if(!r.ok)throw new Error(data.error||'Could not save settings'); calendarSettings=data; currentDate=calendarNow(); await loadData(); closeSettings(); DarUI.toast('Calendar settings saved'); } catch(e) { status.textContent=e.message||String(e); status.className='text-xs text-error'; } finally { button.disabled=false; } }
function showCalendarSettingsSection(name) { document.querySelectorAll('.calendar-settings-section').forEach(s=>s.classList.toggle('hidden',s.dataset.calendarSettingsSection!==name)); document.querySelectorAll('.calendar-settings-tab').forEach(b=>{const active=b.dataset.calendarSettingsTab===name;b.classList.toggle('bg-primary-container',active);b.classList.toggle('text-primary',active)}); document.getElementById('calendar-settings-mobile-tab').value=name; }
function openSettings(section) { populateCalendarSettings(); showCalendarSettingsSection(section||'general'); const modal=document.getElementById('calendar-settings-modal'); modal.classList.remove('hidden'); modal.classList.add('flex'); }
function closeSettings() { const modal=document.getElementById('calendar-settings-modal'); modal.classList.add('hidden'); modal.classList.remove('flex'); }
function openCalendarMenu(id) { const cal=calendars.find(c=>c.id===id); if(!cal)return; calendarMenuID=id; document.getElementById('calendar-menu-title').textContent=cal.name||'Calendar'; document.getElementById('calendar-menu-name').value=cal.name||''; document.getElementById('calendar-menu-color').value=cal.color||'#f5c2e7'; const isSubscription=cal.kind==='subscription'; document.getElementById('calendar-menu-subscription').classList.toggle('hidden',!isSubscription); document.getElementById('calendar-menu-subscription-url').textContent=cal.subscription_url||''; document.getElementById('calendar-menu-sync-state').textContent=cal.last_sync_error?('Last refresh failed: '+cal.last_sync_error):(cal.last_synced_at?'Last refreshed '+new Date(cal.last_synced_at*1000).toLocaleString():'Not refreshed yet'); document.getElementById('calendar-menu-delete-confirm').classList.add('hidden'); document.getElementById('calendar-menu-delete').classList.toggle('hidden',cal.id==='default'||cal.id==='birthdays'); const modal=document.getElementById('calendar-menu-modal'); modal.classList.remove('hidden'); modal.classList.add('flex'); }
function closeCalendarMenu(){const modal=document.getElementById('calendar-menu-modal');modal.classList.add('hidden');modal.classList.remove('flex');calendarMenuID=''}
function confirmCalendarRemove(){document.getElementById('calendar-menu-delete-confirm').classList.remove('hidden')}
function cancelCalendarRemove(){document.getElementById('calendar-menu-delete-confirm').classList.add('hidden')}
async function saveCalendarMenu(){const cal=calendars.find(c=>c.id===calendarMenuID);if(!cal)return;try{await api('POST','/calendars/update',{id:cal.id,name:document.getElementById('calendar-menu-name').value.trim()||cal.name,color:document.getElementById('calendar-menu-color').value,visible:cal.visible!==false,order:cal.order||0});closeCalendarMenu();await loadData()}catch(e){document.getElementById('calendar-menu-sync-state').textContent=e.message||String(e)}}
async function refreshCalendarSubscription(){const cal=calendars.find(c=>c.id===calendarMenuID);if(!cal)return;const state=document.getElementById('calendar-menu-sync-state');state.textContent='Refreshing…';try{await api('POST','/subscriptions/refresh?id='+encodeURIComponent(cal.id));await loadData();const updated=calendars.find(c=>c.id===cal.id);state.textContent=updated&&updated.last_synced_at?'Refreshed just now':'Refreshed'}catch(e){state.textContent=e.message||String(e)}}
async function removeCalendar(){const cal=calendars.find(c=>c.id===calendarMenuID);if(!cal)return;try{if(cal.kind==='subscription')await api('DELETE','/subscriptions?id='+encodeURIComponent(cal.id));else await api('POST','/calendars/delete?id='+encodeURIComponent(cal.id));closeCalendarMenu();closeDetail();await loadData()}catch(e){document.getElementById('calendar-menu-sync-state').textContent=e.message||String(e)}}
function chooseCalendarImport(){document.getElementById('calendar-import-input').click()}
async function copyCalendarCalDAV(){await navigator.clipboard.writeText(document.getElementById('calendar-caldav-url').value);document.getElementById('calendar-settings-status').textContent='CalDAV URL copied'}

function findEventById(id) {
	return events.find(ev => ev.id === id);
}
function isRecurringEvent(e) {
	return !!(e && e.rrule);
}
function repeatLabel(rrule) {
	if (!rrule) return '';
	if (rrule.includes('DAILY')) return 'Daily';
	if (rrule.includes('WEEKLY')) return 'Weekly';
	if (rrule.includes('MONTHLY')) return 'Monthly';
	if (rrule.includes('YEARLY')) return 'Yearly';
	return rrule;
}
function reminderLabel(mins) {
	if (mins === 0) return 'At time of event';
	if (mins === 5) return '5 minutes before';
	if (mins === 10) return '10 minutes before';
	if (mins === 15) return '15 minutes before';
	if (mins === 30) return '30 minutes before';
	if (mins === 60) return '1 hour before';
	if (mins === 120) return '2 hours before';
	if (mins === 1440) return '1 day before';
	if (mins === 10080) return '1 week before';
	return mins + ' minutes before';
}
function askScope(action) {
	return new Promise(resolve => {
		scopeResolver = resolve;
		document.getElementById('scope-title').textContent = action === 'delete' ? 'Delete recurring event' : 'Edit recurring event';
		document.getElementById('scope-desc').textContent = action === 'delete'
			? 'This event repeats. Which events do you want to delete?'
			: 'This event repeats. Which events do you want to update?';
		document.getElementById('scope-modal').classList.remove('hidden');
	});
}
function resolveScope(scope) {
	document.getElementById('scope-modal').classList.add('hidden');
	const r = scopeResolver;
	scopeResolver = null;
	if (r) r(scope);
}
function closeScopeModal() {
	document.getElementById('scope-modal').classList.add('hidden');
	const r = scopeResolver;
	scopeResolver = null;
	if (r) r(null);
}

function dt(d) {
	return d.getFullYear() + '-' + String(d.getMonth()+1).padStart(2,'0') + '-' + String(d.getDate()).padStart(2,'0');
}
function startOfDay(d) {
	return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}
function pad2(n) { return String(n).padStart(2,'0'); }

async function api(method, path, body) {
	const opts = { method };
	if (body !== undefined) {
		opts.headers = { 'Content-Type': 'application/json' };
		opts.body = JSON.stringify(body);
	}
	const r = await fetch(apiBase + path, opts);
	if (!r.ok) throw new Error(await r.text());
	const text = await r.text();
	if (!text) return {};
	try { return JSON.parse(text); } catch { return {}; }
}

function visibleCalIds() {
	return new Set(calendars.filter(c => c.visible !== false && (calendarSettings.birthday_calendar || c.id !== 'birthdays')).map(c => c.id));
}

function rawEventColor(event) {
	const calendar = calendars.find(c => c.id === (event.calendar || 'default'));
	return (calendar && calendar.color) || event.color || '#f5c2e7';
}
function eventColor(event) { return DarTheme.tonalize(rawEventColor(event)).background; }
function colorLuminance(value) {
	const match = String(value||'').trim().match(/^#([0-9a-f]{6})$/i);
	if (!match) return null;
	const channels = [0,2,4].map(offset => parseInt(match[1].slice(offset,offset+2),16)/255).map(v => v<=0.04045?v/12.92:Math.pow((v+0.055)/1.055,2.4));
	return 0.2126*channels[0]+0.7152*channels[1]+0.0722*channels[2];
}
function eventTextColor(event) {
	return DarTheme.tonalize(rawEventColor(event)).foreground;
}
function visibleEvents() {
	const vis = visibleCalIds();
	let list = events.filter(e => vis.has(e.calendar || 'default'));
	if (eventFilter) {
		const q = eventFilter.toLowerCase();
		list = list.filter(e => (e.title||'').toLowerCase().includes(q) || (e.description||'').toLowerCase().includes(q) || (e.location||'').toLowerCase().includes(q));
	}
	return list;
}
function indexEventsByDay(range) {
	eventsByDay = new Map();
	const rangeStart = range.start * 1000;
	const rangeEnd = range.end * 1000;
	events.forEach(e => {
		const start = Math.max(e.start * 1000, rangeStart);
		const eventEnd = e.end && e.end > e.start ? e.end : e.start + 60;
		const end = Math.min(eventEnd * 1000, rangeEnd);
		if (end <= start) return;
		const day = eventDate(start/1000);
		day.setHours(0, 0, 0, 0);
		const lastDay = eventDate((end-1)/1000);
		lastDay.setHours(0, 0, 0, 0);
		while (day <= lastDay) {
			const key = dt(day);
			const dayEvents = eventsByDay.get(key) || [];
			dayEvents.push(e);
			eventsByDay.set(key, dayEvents);
			day.setDate(day.getDate() + 1);
		}
	});
}
function onFilterChange() {
	eventFilter = document.getElementById('event-filter').value || '';
	renderCurrentView();
}

async function loadData() {
	try {
		const calRes = await api('GET', '/calendars');
		const prevVis = {};
		calendars.forEach(c => { prevVis[c.id] = c.visible; });
		calendars = (calRes.calendars || []).map(c => ({
			...c,
			visible: prevVis[c.id] !== undefined ? prevVis[c.id] : (c.visible !== false),
		}));
		if (!calendars.length) calendars = [{id:'default', name:'Personal', color:'#f5c2e7', visible:true}];
	} catch (e) {
		console.warn('calendars', e);
		if (!calendars.length) calendars = [{id:'default', name:'Personal', color:'#f5c2e7', visible:true}];
	}
	renderCalendars();
	renderMiniCalendar();

	const range = getCurrentRange();
	const res = await api('GET', '/list?start=' + range.start + '&end=' + range.end);
	events = res.events || [];
	indexEventsByDay(range);
	renderCurrentView();
}

function getCurrentRange() {
	const year = currentDate.getFullYear();
	const month = currentDate.getMonth();
	let start, end;
	if (currentView === 'month') {
		const first = new Date(year, month, 1);
		const gridStart = new Date(first);
		gridStart.setDate(1 - first.getDay());
		start = wallTimeToEpoch(dt(gridStart),'00:00');
		const gridEnd=new Date(gridStart);gridEnd.setDate(gridEnd.getDate()+42);end=wallTimeToEpoch(dt(gridEnd),'00:00');
	} else if (currentView === 'week') {
		const d = startOfDay(currentDate);
		d.setDate(d.getDate() - d.getDay());
		start = wallTimeToEpoch(dt(d),'00:00'); const e=new Date(d);e.setDate(e.getDate()+7);end=wallTimeToEpoch(dt(e),'00:00');
	} else if (currentView === 'day') {
		const d = startOfDay(currentDate);
		start = wallTimeToEpoch(dt(d),'00:00'); const e=new Date(d);e.setDate(e.getDate()+1);end=wallTimeToEpoch(dt(e),'00:00');
	} else if (currentView === 'year') {
		start = wallTimeToEpoch(year+'-01-01','00:00');
		end = wallTimeToEpoch((year+1)+'-01-01','00:00');
	} else {
		start = wallTimeToEpoch(dt(new Date(year,month,1)),'00:00');
		end = wallTimeToEpoch(dt(new Date(year,month+1,1)),'00:00');
	}
	return {start, end};
}

function renderCalendars() {
	let html = '<div class="text-xs uppercase tracking-widest text-on-surface-variant px-3 py-2 mb-2">My calendars</div>';
	calendars.filter(cal => calendarSettings.birthday_calendar || cal.id !== 'birthdays').forEach(cal => {
		const checked = cal.visible !== false ? 'checked' : '';
		html += '<div class="flex items-center gap-2 px-3 py-2 hover:bg-surface-container-high rounded-2xl group">' +
			'<input type="checkbox" aria-label="Show '+esc(cal.name)+' calendar" ' + checked + ' class="accent-primary w-4 h-4" onclick="event.stopPropagation();toggleCalendarVisibility(\'' + cal.id + '\')">' +
			'<div class="w-3 h-3 rounded-full flex-shrink-0" style="background:' + esc(DarTheme.tonalize(cal.color||'#f5c2e7').background) + '"></div>' +
			'<span class="flex-1 text-sm truncate">' + esc(cal.name) + '</span>' +
			(cal.read_only ? '<span class="text-xs text-on-surface-variant" title="Read-only calendar"><i class="fa-solid fa-lock"></i></span>' : '') +
			(cal.last_sync_error ? '<span class="text-xs text-error" title="Subscription refresh failed"><i class="fa-solid fa-triangle-exclamation"></i></span>' : '') +
			(cal.id === 'birthdays' ? '' : '<button type="button" onclick="event.stopPropagation();openCalendarMenu(\'' + cal.id + '\')" class="opacity-0 group-hover:opacity-100 focus:opacity-100 text-on-surface-variant px-1" aria-label="Manage '+esc(cal.name)+'"><i class="fa-solid fa-ellipsis-vertical"></i></button>') +
			'</div>';
	});
	document.getElementById('calendars-list').innerHTML = html;
}

async function toggleCalendarVisibility(id) {
	const cal = calendars.find(c => c.id === id);
	if (!cal) return;
	cal.visible = !cal.visible;
	renderCalendars();
	renderCurrentView();
	if (cal.read_only) return;
	try {
		await api('POST', '/calendars/update', {id: cal.id, name: cal.name, color: cal.color, visible: cal.visible, order: cal.order||0});
	} catch (e) { console.warn(e); }
}

function renderMiniCalendar() {
	const el = document.getElementById('mini-calendar');
	if (!el) return;
	const y = currentDate.getFullYear(), m = currentDate.getMonth();
	const first = new Date(y, m, 1).getDay();
	const dim = new Date(y, m+1, 0).getDate();
	const todayStr = dt(calendarNow());
	const title = currentDate.toLocaleDateString(undefined, {month:'long', year:'numeric'});
	let html = '<div class="flex items-center justify-between mb-2 px-1">' +
		'<button type="button" aria-label="Previous month" class="w-7 h-7 rounded-full hover:bg-surface-container-high" onclick="miniNav(-1)"><i class="fa-solid fa-chevron-left text-xs"></i></button>' +
		'<span class="text-sm font-medium">' + esc(title) + '</span>' +
		'<button type="button" aria-label="Next month" class="w-7 h-7 rounded-full hover:bg-surface-container-high" onclick="miniNav(1)"><i class="fa-solid fa-chevron-right text-xs"></i></button></div>';
	html += '<div class="grid grid-cols-7 gap-0.5 text-center text-[10px] text-on-surface-variant mb-1">' +
		'<div>S</div><div>M</div><div>T</div><div>W</div><div>T</div><div>F</div><div>S</div></div>';
	html += '<div class="grid grid-cols-7 gap-0.5 text-center text-xs">';
	for (let i = 0; i < first; i++) html += '<div class="h-7"></div>';
	for (let d = 1; d <= dim; d++) {
		const ds = y + '-' + pad2(m+1) + '-' + pad2(d);
		const isToday = ds === todayStr;
		const isSel = ds === dt(currentDate);
		let cls = 'h-7 rounded-full flex items-center justify-center cursor-pointer hover:bg-surface-container-high';
		if (isToday) cls += ' bg-primary text-on-primary font-semibold';
		else if (isSel) cls += ' ring-1 ring-primary';
		html += '<button type="button" class="' + cls + '" onclick="jumpToDay(\'' + ds + '\')">' + d + '</button>';
	}
	html += '</div>';
	el.innerHTML = html;
}
function miniNav(delta) {
	currentDate = new Date(currentDate.getFullYear(), currentDate.getMonth() + delta, 1);
	loadData();
}
function jumpToDay(dateStr) {
	currentDate = new Date(dateStr + 'T12:00:00');
	if (currentView === 'year' || currentView === 'month') switchView('day');
	else loadData();
}

function updateSidebarHighlight() {
	document.querySelectorAll('#sidebar nav a').forEach(a => {
		const href = a.getAttribute('href') || '';
		const m = href.match(/[?&]view=(\w+)/);
		const isActive = m && m[1] === currentView;
		a.classList.toggle('bg-primary-container', !!isActive);
		a.classList.toggle('text-primary', !!isActive);
		a.classList.toggle('font-medium', !!isActive);
	});
}

function renderCurrentView() {
	closeMonthOverflow();
	document.querySelectorAll('#main-view > div').forEach(v => v.classList.add('hidden'));
	const pane = document.getElementById(currentView + '-view');
	if (pane) pane.classList.remove('hidden');
	document.querySelectorAll('.view-btn').forEach(b => b.classList.remove('bg-primary-container', 'text-primary'));
	const vb = document.getElementById('view-' + currentView);
	if (vb) vb.classList.add('bg-primary-container', 'text-primary');
	updateSidebarHighlight();
	document.getElementById('period-title').textContent = formatPeriodTitle();
	if (currentView === 'month') renderMonth();
	else if (currentView === 'week') renderTimeGrid(7);
	else if (currentView === 'day') renderTimeGrid(1);
	else if (currentView === 'list') renderList();
	else if (currentView === 'year') renderYear();
	renderMiniCalendar();
}

function formatPeriodTitle() {
	if (currentView === 'day') {
		return currentDate.toLocaleDateString(undefined, {weekday:'long', month:'long', day:'numeric', year:'numeric'});
	}
	if (currentView === 'week') {
		const d = startOfDay(currentDate);
		d.setDate(d.getDate() - d.getDay());
		const e = new Date(d); e.setDate(e.getDate() + 6);
		return d.toLocaleDateString(undefined, {month:'short', day:'numeric'}) + ' – ' + e.toLocaleDateString(undefined, {month:'short', day:'numeric', year:'numeric'});
	}
	if (currentView === 'year') return String(currentDate.getFullYear());
	return currentDate.toLocaleDateString(undefined, {month:'long', year:'numeric'});
}

function eventsOnDay(dateStr) {
	const visible = visibleCalIds();
	const q = eventFilter.toLowerCase();
	return (eventsByDay.get(dateStr) || []).filter(e => {
		if (!visible.has(e.calendar || 'default')) return false;
		return !q || (e.title||'').toLowerCase().includes(q) || (e.description||'').toLowerCase().includes(q) || (e.location||'').toLowerCase().includes(q);
	});
}

function closeMonthOverflow() {
	const pop = document.getElementById('month-overflow-popover');
	if (!pop || pop.classList.contains('hidden')) return;
	pop.classList.add('hidden');
	document.querySelectorAll('[data-month-overflow-trigger="true"]').forEach(button => button.setAttribute('aria-expanded', 'false'));
}

function showMonthOverflow(dateStr, trigger) {
	const pop = document.getElementById('month-overflow-popover');
	const dayEvents = eventsOnDay(dateStr);
	if (!pop || !dayEvents.length) return;
	closeMonthOverflow();
	document.getElementById('month-overflow-title').textContent = new Date(dateStr + 'T12:00:00').toLocaleDateString(undefined, {weekday:'long', month:'long', day:'numeric'});
	document.getElementById('month-overflow-events').innerHTML = dayEvents.map(e => {
		const time = e.all_day ? 'All day' : formatTime(e.start * 1000);
		return '<button type="button" data-event-id="' + esc(e.id) + '" onclick="openMonthOverflowEvent(this.dataset.eventId)" class="flex w-full items-center gap-2 rounded-xl px-2 py-2 text-left hover:bg-surface-container-high">' +
			'<span class="h-3 w-3 shrink-0 rounded-full" style="background:' + esc(eventColor(e)) + '"></span>' +
			'<span class="min-w-0 flex-1 truncate text-sm">' + esc(e.title || 'Untitled') + '</span>' +
			'<span class="shrink-0 text-xs text-on-surface-variant">' + esc(time) + '</span></button>';
	}).join('');
	pop.classList.remove('hidden');
	const rect = trigger.getBoundingClientRect();
	const margin = 16;
	const left = Math.min(Math.max(margin, rect.left), window.innerWidth - pop.offsetWidth - margin);
	const top = Math.min(rect.bottom + 6, window.innerHeight - pop.offsetHeight - margin);
	pop.style.left = left + 'px';
	pop.style.top = Math.max(margin, top) + 'px';
	trigger.setAttribute('aria-expanded', 'true');
}

function openMonthOverflowEvent(id) {
	closeMonthOverflow();
	showEventDetail(id);
}

function renderMonth() {
	const year=currentDate.getFullYear(), month=currentDate.getMonth(), showWeekends=calendarSettings.show_weekends;
	const todayStr = dt(calendarNow()); const maxChips = calendarSettings.limit_month_events ? 4 : Number.MAX_SAFE_INTEGER;
	const weekdayNames=showWeekends?['Sun','Mon','Tue','Wed','Thu','Fri','Sat']:['Mon','Tue','Wed','Thu','Fri'];
	const cols=weekdayNames.length+(calendarSettings.show_week_numbers?1:0); const header=document.getElementById('month-weekday-header'); const grid=document.getElementById('month-grid');
	header.className='grid grid-cols-'+cols+' text-center text-xs font-bold text-on-surface-variant py-2 border-b border-outline-variant'; header.innerHTML=(calendarSettings.show_week_numbers?'<div class="text-primary">Wk</div>':'')+weekdayNames.map(n=>'<div>'+n+'</div>').join(''); grid.className='flex-1 grid grid-cols-'+cols+' border border-outline-variant overflow-auto';
	const first=new Date(year,month,1); const start=new Date(first); if(showWeekends)start.setDate(start.getDate()-start.getDay());else start.setDate(start.getDate()-((start.getDay()+6)%7)); const last=new Date(year,month+1,0); const rows=[]; let cursor=new Date(start); while(cursor<=last||cursor.getDay()!==(showWeekends?0:1)){rows.push(new Date(cursor));cursor.setDate(cursor.getDate()+7);if(rows.length>6)break}
	let html=''; rows.forEach(weekStart=>{ if(calendarSettings.show_week_numbers)html+='<div class="border-r border-b border-outline-variant bg-surface-container-low p-2 text-center text-xs text-on-surface-variant">'+isoWeekNumber(weekStart)+'</div>'; const offsets=showWeekends?[0,1,2,3,4,5,6]:[0,1,2,3,4]; offsets.forEach(offset=>{const day=new Date(weekStart);day.setDate(day.getDate()+offset);const dateStr=dt(day);const inMonth=day.getMonth()===month;
		const isToday = dateStr === todayStr;
		const dayEvents = eventsOnDay(dateStr);
		const shown = dayEvents.slice(0, maxChips);
		const extra = dayEvents.length - shown.length;
		let chips = shown.map(e => {
			const t = e.all_day ? '' : formatTime(e.start*1000) + ' ';
			return '<div onclick="event.stopPropagation();showEventDetail(\'' + e.id + '\')" class="text-[11px] leading-tight px-1 py-0.5 rounded truncate mb-0.5" style="background:' + esc(eventColor(e)) + ';color:' + esc(eventTextColor(e)) + '" title="' + esc(e.title) + '">' + esc(t + (e.title||'')) + '</div>';
		}).join('');
		if (extra > 0) chips += '<button type="button" data-month-overflow-trigger="true" aria-expanded="false" onclick="event.stopPropagation();showMonthOverflow(\'' + dateStr + '\', this)" class="px-1 text-left text-[10px] font-medium text-primary hover:underline">+' + extra + ' more</button>';
		const dayNumCls = isToday ? 'inline-flex w-7 h-7 items-center justify-center rounded-full bg-primary text-on-primary font-semibold' : (inMonth?'':'text-on-surface-variant');
		html += '<div onclick="showCreateAt(\'' + dateStr + '\', false, undefined, undefined, event.clientX, event.clientY)" class="border-r border-b border-outline-variant p-1 min-h-[100px] cursor-pointer hover:bg-surface-container-high/50 flex flex-col '+(inMonth?'':'bg-surface-container/40')+'">' +
			'<div class="text-right text-sm pr-1 mb-0.5"><span class="' + dayNumCls + '">' + day.getDate() + '</span></div>' +
			'<div class="flex-1 overflow-hidden">' + chips + '</div></div>';
	});}); grid.innerHTML = html;
}

function renderTimeGrid(numDays) {
	const container = document.getElementById(numDays === 1 ? 'day-view' : 'week-view');
	const base = startOfDay(currentDate);
	if (numDays === 7) base.setDate(base.getDate() - (calendarSettings.show_weekends?base.getDay():((base.getDay()+6)%7)));
	const todayStr = dt(calendarNow());
	const days = [];
	const offsets=numDays===7&&!calendarSettings.show_weekends?[0,1,2,3,4]:Array.from({length:numDays},(_,i)=>i);
	offsets.forEach(i => {
		const d = new Date(base); d.setDate(base.getDate() + i);
		days.push(d);
	});
	const dayCount=days.length; let header = '<div class="grid sticky top-0 z-10 bg-surface border-b border-outline-variant" style="grid-template-columns: 56px repeat(' + dayCount + ', 1fr)">';
	header += '<div></div>';
	days.forEach(d => {
		const ds = dt(d);
		const isToday = ds === todayStr;
		header += '<div class="text-center py-2 ' + (isToday ? 'bg-primary-container/40' : '') + '">' +
			'<div class="text-xs text-on-surface-variant">' + d.toLocaleDateString(undefined,{weekday:'short'}) + '</div>' +
			'<div class="text-lg font-semibold ' + (isToday ? 'text-primary' : '') + '">' + d.getDate() + '</div></div>';
	});
	header += '</div>';

	// All-day row
	let allDay = '<div class="grid border-b border-outline-variant" style="grid-template-columns: 56px repeat(' + dayCount + ', 1fr)">';
	allDay += '<div class="text-[10px] text-on-surface-variant p-1 text-right pr-2">all-day</div>';
	days.forEach(d => {
		const ds = dt(d);
		const ads = eventsOnDay(ds).filter(e => e.all_day);
		allDay += '<div class="min-h-[28px] border-l border-outline-variant p-0.5" onclick="showCreateAt(\'' + ds + '\', true, undefined, undefined, event.clientX, event.clientY)">';
		ads.forEach(e => {
			allDay += '<div onclick="event.stopPropagation();showEventDetail(\'' + e.id + '\')" class="text-[11px] px-1 py-0.5 rounded truncate mb-0.5" style="background:' + esc(eventColor(e)) + ';color:' + esc(eventTextColor(e)) + '">' + esc(e.title) + '</div>';
		});
		allDay += '</div>';
	});
	allDay += '</div>';

	let body = '<div class="grid relative" style="grid-template-columns: 56px repeat(' + dayCount + ', 1fr); height:' + (24*HOUR_H) + 'px">';
	// hour labels
	body += '<div class="relative">';
	for (let h = 0; h < 24; h++) {
		body += '<div class="absolute right-1 text-[10px] text-on-surface-variant" style="top:' + (h*HOUR_H) + 'px">' + (h===0?'': (h<12?h:h===12?12:h-12) + (h<12?'am':'pm')) + '</div>';
	}
	body += '</div>';
	days.forEach(d => {
		const ds = dt(d);
		const isToday = ds === todayStr;
		body += '<div class="relative border-l border-outline-variant ' + (isToday ? 'bg-primary-container/10' : '') + '" onclick="onGridClick(event, \'' + ds + '\')">';
		for (let mins = 0; mins < 24*60; mins += calendarSettings.slot_minutes) {
			body += '<div class="absolute left-0 right-0 border-t border-outline-variant/50" style="top:' + ((mins/60)*HOUR_H) + 'px"></div>';
		}
		eventsOnDay(ds).filter(e => !e.all_day).forEach(e => {
			const s = eventDate(e.start);
			const en = eventDate(e.end || e.start + 3600);
			const top = (s.getHours() + s.getMinutes()/60) * HOUR_H;
			const height = Math.max(20, ((en - s) / 3600000) * HOUR_H);
			const eid = esc(e.id);
			body += '<div data-event-id="' + eid + '" data-day="' + ds + '" onmousedown="onEventPointerDown(event,\'' + eid + '\',\'move\')" onclick="event.stopPropagation();if(!suppressClick)showEventDetail(\'' + eid + '\')" class="absolute left-0.5 right-0.5 rounded px-1 py-0.5 text-[11px] overflow-hidden cursor-grab z-[1] select-none" style="top:' + top + 'px;height:' + height + 'px;background:' + esc(eventColor(e)) + ';color:' + esc(eventTextColor(e)) + '">' +
				'<div class="font-medium truncate pointer-events-none">' + esc(e.title) + '</div>' +
				'<div class="opacity-80 truncate pointer-events-none">' + formatTime(s) + '</div>' +
				'<div onmousedown="onEventPointerDown(event,\'' + eid + '\',\'resize\')" class="absolute bottom-0 left-0 right-0 h-2 cursor-ns-resize" title="Resize"></div></div>';
		});
		// now line
		if (isToday) {
			const now = calendarNow();
			const top = (now.getHours() + now.getMinutes()/60) * HOUR_H;
			body += '<div class="absolute left-0 right-0 border-t-2 border-error z-[2]" style="top:' + top + 'px"><div class="w-2 h-2 rounded-full bg-error -mt-1 -ml-1"></div></div>';
		}
		body += '</div>';
	});
	body += '</div>';
	container.innerHTML = '<div class="flex flex-col min-h-0 h-full overflow-auto">' + header + allDay + body + '</div>';
}

function onGridClick(ev, dateStr) {
	const col = ev.currentTarget;
	const rect = col.getBoundingClientRect();
	const relY = ev.clientY - rect.top;
	const total=Math.max(0,Math.min(24*60-calendarSettings.slot_minutes,Math.floor((relY/HOUR_H)*60/calendarSettings.slot_minutes)*calendarSettings.slot_minutes)); const hour=Math.floor(total/60); const mins=total%60;
	showCreateAt(dateStr, false, hour, mins, ev.clientX, ev.clientY);
}

function renderList() {
	const list = visibleEvents().slice().sort((a,b) => a.start - b.start);
	let html = list.length ? '<div class="space-y-1">' : '<div class="p-12 text-center text-on-surface-variant">No events in range</div>';
	let lastDay = '';
	list.forEach(e => {
		const s = eventDate(e.start);
		const day = s.toLocaleDateString(undefined, {weekday:'long', month:'long', day:'numeric'});
		if (day !== lastDay) {
			html += '<div class="px-4 pt-4 pb-1 text-xs font-bold uppercase tracking-widest text-on-surface-variant">' + esc(day) + '</div>';
			lastDay = day;
		}
		const time = e.all_day ? 'All day' : formatTime(s);
		html += '<div onclick="showEventDetail(\'' + e.id + '\')" class="flex gap-4 p-3 hover:bg-surface-container-high cursor-pointer group rounded-2xl mx-2">' +
			'<div style="background:' + esc(eventColor(e)) + '" class="w-1 self-stretch rounded"></div>' +
			'<div class="w-20 text-sm text-on-surface-variant shrink-0">' + esc(time) + '</div>' +
			'<div class="flex-1 min-w-0"><div class="font-medium">' + esc(e.title) + '</div>' +
			(e.location ? '<div class="text-xs text-on-surface-variant"><i class="fa-solid fa-location-dot mr-1"></i>' + esc(e.location) + '</div>' : '') +
			'</div>' +
			'<button type="button" onclick="event.stopPropagation();deleteEvent(\'' + e.id + '\')" class="opacity-0 group-hover:opacity-100 px-3 text-error"><i class="fa-solid fa-trash-can"></i></button></div>';
	});
	document.getElementById('list-view').innerHTML = html;
}

function renderYear() {
	const y = currentDate.getFullYear();
	const todayStr = dt(calendarNow());
	let html = '<div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-6">';
	for (let m = 0; m < 12; m++) {
		const first = new Date(y, m, 1).getDay();
		const dim = new Date(y, m+1, 0).getDate();
		html += '<div><div class="text-sm font-semibold mb-2">' + new Date(y,m,1).toLocaleDateString(undefined,{month:'long'}) + '</div>';
		html += '<div class="grid grid-cols-7 gap-0.5 text-[10px] text-center text-on-surface-variant mb-1"><div>S</div><div>M</div><div>T</div><div>W</div><div>T</div><div>F</div><div>S</div></div>';
		html += '<div class="grid grid-cols-7 gap-0.5 text-center text-xs">';
		for (let i = 0; i < first; i++) html += '<div class="h-6"></div>';
		for (let d = 1; d <= dim; d++) {
			const ds = y + '-' + pad2(m+1) + '-' + pad2(d);
			const has = eventsOnDay(ds).length > 0;
			const isToday = ds === todayStr;
			let cls = 'h-6 rounded-full flex items-center justify-center cursor-pointer hover:bg-surface-container-high';
			if (isToday) cls += ' bg-primary text-on-primary';
			else if (has) cls += ' font-semibold text-primary';
			html += '<button type="button" class="' + cls + '" onclick="jumpToDay(\'' + ds + '\')">' + d + '</button>';
		}
		html += '</div></div>';
	}
	html += '</div>';
	document.getElementById('year-view').innerHTML = html;
}

function showEventDetail(id) {
	currentDetailId = id;
	const e = findEventById(id) || visibleEvents().find(ev => ev.id === id);
	if (!e) return;
	if (location.hash !== '#event-' + encodeURIComponent(id)) {
		history.pushState(Object.assign({}, history.state, {darDetail:'calendar',darDetailID:id}), '', '#event-' + encodeURIComponent(id));
	}
	const when = e.all_day
		? eventDate(e.start).toLocaleDateString(undefined, {weekday:'short', month:'short', day:'numeric'}) + ' (all day)'
		: eventDate(e.start).toLocaleDateString(undefined, {weekday:'short', month:'short', day:'numeric'}) + ', ' + formatTime(e.start*1000) + ' – ' + formatTime(e.end*1000);
	const cal = calendars.find(c => c.id === (e.calendar||'default'));
	const statusLabel = ({CONFIRMED:'Confirmed',TENTATIVE:'Tentative',CANCELLED:'Cancelled'})[e.status||'CONFIRMED'] || (e.status||'Confirmed');
	const busyLabel = e.busy === false ? 'Free' : 'Busy';
	const cats = (e.categories || []).filter(Boolean);
	const rem = (e.reminders && e.reminders.length) ? e.reminders[0] : null;
	const eid = esc(id);
	let html = '<div class="flex h-full min-w-0 flex-col"><div data-dar-pane-header class="flex min-w-0 items-center gap-3 mb-6">' +
		'<button type="button" data-dar-detail-back onclick="closeDetail()" class="h-10 w-10 shrink-0 items-center justify-center rounded-full hover:bg-surface-container-high lg:hidden" aria-label="Back to calendar"><i class="fa-solid fa-arrow-left"></i></button>' +
		'<div class="w-5 h-5 rounded-xl flex-shrink-0" style="background:' + esc(eventColor(e)) + '"></div>' +
		'<h3 class="min-w-0 flex-1 break-words text-2xl font-semibold">' + esc(e.title) + '</h3></div>' +
		'<div class="min-h-0 min-w-0 flex-1 space-y-4 overflow-y-auto pr-1 text-sm">' +
		'<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">When</div><div class="min-w-0 flex-1 break-words">' + esc(when) + '</div></div>' +
		(e.rrule ? '<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Repeats</div><div class="min-w-0 flex-1 break-words">' + esc(repeatLabel(e.rrule)) + '</div></div>' : '') +
		(rem !== null && rem !== undefined ? '<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Reminder</div><div class="min-w-0 flex-1 break-words">' + esc(reminderLabel(rem)) + '</div></div>' : '') +
		(e.location ? '<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Where</div><div class="min-w-0 flex-1 break-words">' + esc(e.location) + '</div></div>' : '') +
		(cal ? '<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Calendar</div><div class="min-w-0 flex-1 break-words">' + esc(cal.name) + '</div></div>' : '') +
		'<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Status</div><div class="min-w-0 flex-1 break-words">' + esc(statusLabel) + '</div></div>' +
		'<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Show as</div><div class="min-w-0 flex-1 break-words">' + esc(busyLabel) + '</div></div>' +
		(cats.length ? '<div class="flex min-w-0 gap-4"><div class="w-20 text-on-surface-variant shrink-0">Tags</div><div class="min-w-0 flex-1 flex flex-wrap gap-1">' +
			cats.map(c => '<span class="px-2 py-0.5 rounded-full bg-surface-container-high text-xs">' + esc(c) + '</span>').join('') +
			'</div></div>' : '') +
		(e.description ? '<div class="flex gap-4 min-w-0"><div class="w-20 text-on-surface-variant shrink-0">Notes</div><div class="min-w-0 flex-1 max-h-56 overflow-y-auto overflow-x-hidden whitespace-pre-wrap break-words rounded-2xl bg-surface-container-low p-3">' + noteText(e.description) + '</div></div>' : '') +
		'</div>' +
		'<div class="mt-4 shrink-0 border-t border-outline-variant pt-4 flex flex-col gap-2">' +
		'<div class="flex gap-2">' +
		'<button type="button" onclick="editEvent(\'' + eid + '\')" class="flex-1 h-11 rounded-2xl border border-primary text-primary font-medium">Edit</button>' +
		'<button type="button" onclick="duplicateEvent(\'' + eid + '\')" class="flex-1 h-11 rounded-2xl border border-outline-variant font-medium hover:bg-surface-container-high">Duplicate</button></div>' +
		'<button type="button" onclick="deleteEvent(\'' + eid + '\')" class="w-full h-11 rounded-2xl bg-error-container text-error font-medium">Delete</button></div></div>';
	document.getElementById('event-detail').innerHTML = html;
	document.getElementById('event-detail').classList.remove('hidden');
	document.getElementById('detail-empty').classList.add('hidden');
	DarUI.shell.openDetail(document.getElementById('detail-pane'), document.activeElement);
}

function duplicateEvent(id) {
	const e = findEventById(id);
	if (!e) return;
	const data = {
		title: (e.title || 'Untitled') + ' (copy)',
		description: e.description || '',
		location: e.location || '',
		start: e.start,
		end: e.end || e.start + 3600,
		all_day: !!e.all_day,
		color: eventColor(e),
		calendar: e.calendar || 'default',
		rrule: e.rrule || '',
		busy: e.busy !== false,
		status: e.status || 'CONFIRMED',
		categories: e.categories || [],
		reminders: e.reminders || [],
	};
	api('POST', '/create', data).then((created) => {
		loadData().then(() => {
			if (created && created.id) {
				// After create, list may expand — show master or first matching title
				const match = events.find(x => x.title === created.title) || created;
				showEventDetail(match.id);
			}
		});
	}).catch(err => DarUI.toast('Duplicate failed: ' + err.message, {tone: 'error'}));
}

function closeDetail(fromHistory) {
	if (!fromHistory && DarUI.shell.isCompact()) {
		if (history.state?.darDetail === 'calendar') {
			history.back();
			return;
		}
		history.replaceState(history.state, '', location.pathname + location.search);
	}
	document.getElementById('event-detail').classList.add('hidden');
	document.getElementById('detail-empty').classList.remove('hidden');
	currentDetailId = null;
	DarUI.shell.closeDetail(document.getElementById('detail-pane'));
}

function switchView(view) {
	currentView = view;
	const url = new URL(window.location);
	url.searchParams.set('view', view);
	window.history.replaceState({}, '', url);
	loadData();
}

function goToday() {
	currentDate = calendarNow();
	loadData();
}
function prevPeriod() {
	if (currentView === 'year') currentDate.setFullYear(currentDate.getFullYear() - 1);
	else if (currentView === 'month' || currentView === 'list') currentDate.setMonth(currentDate.getMonth() - 1);
	else currentDate.setDate(currentDate.getDate() - (currentView === 'week' ? 7 : 1));
	loadData();
}
function nextPeriod() {
	if (currentView === 'year') currentDate.setFullYear(currentDate.getFullYear() + 1);
	else if (currentView === 'month' || currentView === 'list') currentDate.setMonth(currentDate.getMonth() + 1);
	else currentDate.setDate(currentDate.getDate() + (currentView === 'week' ? 7 : 1));
	loadData();
}

function defaultTimes() {
	const now = calendarNow();
	return { date: dt(now), startTime: '09:00', endTime: '10:00' };
}

function fillTimes(prefix, dateStr, allDay, hour, mins) {
	document.getElementById(prefix + '-start-date').value = dateStr;
	document.getElementById(prefix + '-end-date').value = dateStr;
	document.getElementById(prefix + '-all-day').checked = !!allDay;
	if (allDay) {
		document.getElementById(prefix + '-start-time').value = '00:00';
		document.getElementById(prefix + '-end-time').value = '00:00';
	} else {
		const h = hour !== undefined ? hour : 9;
		const m = mins !== undefined ? mins : 0;
		const st = pad2(h) + ':' + pad2(m);
		const eh = m === 30 ? h + 1 : h;
		const em = m === 30 ? 0 : 30;
		document.getElementById(prefix + '-start-time').value = st;
		document.getElementById(prefix + '-end-time').value = pad2(eh) + ':' + pad2(em);
	}
	toggleAllDayFor(prefix);
}

function populateCalSelect(selectId, selected) {
	if (!selected) selected = (calendars.find(c => c.visible !== false) || calendars[0] || {id:'default'}).id;
	const select = document.getElementById(selectId);
	select.innerHTML = calendars.map(c => '<option value="' + esc(c.id) + '"' + (c.id === selected ? ' selected' : '') + '>' + esc(c.name) + '</option>').join('');
}

function positionPopover(clientX, clientY) {
	const pop = document.getElementById('event-popover');
	const pad = 12;
	const w = 352;
	const h = Math.min(pop.offsetHeight || 360, window.innerHeight - 24);
	let left = (clientX != null ? clientX : window.innerWidth / 2 - w / 2);
	let top = (clientY != null ? clientY : window.innerHeight / 2 - h / 2);
	if (left + w + pad > window.innerWidth) left = window.innerWidth - w - pad;
	if (left < pad) left = pad;
	if (top + h + pad > window.innerHeight) top = window.innerHeight - h - pad;
	if (top < pad) top = pad;
	pop.style.left = left + 'px';
	pop.style.top = top + 'px';
}

function openPopover(opts) {
	// opts: {x,y, dateStr, allDay, hour, mins, editId}
	closeModal();
	currentEdit = opts.editId || null;
	document.getElementById('pop-title-label').textContent = currentEdit ? 'Edit event' : 'New event';
	document.getElementById('pop-title').value = '';
	const d = defaultTimes();
	const dateStr = opts.dateStr || d.date;
	if (currentEdit) {
		const e = events.find(ev => ev.id === currentEdit);
		if (e) {
			document.getElementById('pop-title').value = e.title || '';
			const s = eventDate(e.start);
			const en = eventDate(e.end || e.start + 3600);
			document.getElementById('pop-start-date').value = dt(s);
			document.getElementById('pop-end-date').value = dt(en);
			document.getElementById('pop-all-day').checked = !!e.all_day;
			document.getElementById('pop-start-time').value = pad2(s.getHours()) + ':' + pad2(s.getMinutes());
			document.getElementById('pop-end-time').value = pad2(en.getHours()) + ':' + pad2(en.getMinutes());
			toggleAllDayFor('pop');
			populateCalSelect('pop-calendar', e.calendar || 'default');
		}
	} else {
		fillTimes('pop', dateStr, opts.allDay, opts.hour, opts.mins);
		if (!opts.allDay && opts.hour === undefined) {
			document.getElementById('pop-start-time').value = d.startTime;
			document.getElementById('pop-end-time').value = d.endTime;
		}
		populateCalSelect('pop-calendar');
	}
	const pop = document.getElementById('event-popover');
	pop.classList.remove('hidden');
	positionPopover(opts.x, opts.y);
	document.getElementById('pop-title').focus();
}

function closePopover() {
	document.getElementById('event-popover').classList.add('hidden');
}

function readFormTimes(prefix) {
	const sd = document.getElementById(prefix + '-start-date').value;
	const ed = document.getElementById(prefix + '-end-date').value || sd;
	const allDay = document.getElementById(prefix + '-all-day').checked;
	let start, end;
	if (allDay) {
		start = wallTimeToEpoch(sd, '00:00'); const next=new Date(ed+'T12:00:00');next.setDate(next.getDate()+1);end=wallTimeToEpoch(dt(next),'00:00');
	} else {
		const st = document.getElementById(prefix + '-start-time').value || '09:00';
		const et = document.getElementById(prefix + '-end-time').value || '10:00';
		start = wallTimeToEpoch(sd,st);
		end = wallTimeToEpoch(ed,et);
	}
	return { start, end, allDay, calId: document.getElementById(prefix + '-calendar').value || 'default' };
}

function parseCategories(s) {
	return (s || '').split(',').map(x => x.trim()).filter(Boolean);
}

function saveFromPopover() {
	let t; try { t=readFormTimes('pop'); } catch(e) { DarUI.toast(e.message, {tone: 'error'}); return; }
	const cal = calendars.find(c => c.id === t.calId);
	const existing = currentEdit ? events.find(ev => ev.id === currentEdit) : null;
	const defaults=t.allDay?calendarSettings.default_all_day_reminder:calendarSettings.default_timed_reminder; const data = {
		title: document.getElementById('pop-title').value.trim() || 'Untitled event',
		description: (existing && existing.description) || '',
		location: (existing && existing.location) || '',
		start: t.start,
		end: t.end,
		all_day: t.allDay,
		color: (cal && cal.color) || (existing && existing.color) || '#f5c2e7',
		calendar: t.calId,
		rrule: (existing && existing.rrule) || '',
		busy: existing ? existing.busy !== false : true,
		status: (existing && existing.status) || 'CONFIRMED',
		categories: (existing && existing.categories) || [],
		timezone: activeCalendarTimezone() || Intl.DateTimeFormat().resolvedOptions().timeZone,
		reminders: existing ? (existing.reminders||[]) : (defaults==null?[]:[defaults]),
	};
	if (currentEdit) data.id = currentEdit;
	const path = currentEdit ? '/update' : '/create';
	api('POST', path, data).then(() => {
		closePopover();
		closeDetail();
		loadData();
	}).catch(err => DarUI.toast('Failed to save: ' + err.message, {tone: 'error'}));
}

function moreDetailsFromPopover() {
	const title = document.getElementById('pop-title').value;
	let t; try { t=readFormTimes('pop'); } catch(e) { DarUI.toast(e.message, {tone: 'error'}); return; }
	const calId = t.calId;
	const existing = currentEdit ? events.find(ev => ev.id === currentEdit) : null;
	closePopover();
	openFullEditor({
		editId: currentEdit,
		title,
		start: t.start,
		end: t.end,
		allDay: t.allDay,
		calId,
		description: (existing && existing.description) || '',
		location: (existing && existing.location) || '',
		status: (existing && existing.status) || 'CONFIRMED',
		busy: existing ? existing.busy !== false : true,
		categories: (existing && existing.categories) || [],
		rrule: (existing && existing.rrule) || '',
	});
}

function openFullEditor(opts) {
	opts = opts || {};
	closePopover();
	currentEdit = opts.editId || null;
	currentEditMeta = opts.editMeta || null;
	document.getElementById('modal-title').textContent = currentEdit ? 'Edit event' : 'New event';
	document.getElementById('ev-title').value = opts.title || '';
	document.getElementById('ev-desc').value = opts.description || '';
	document.getElementById('ev-location').value = opts.location || '';
	document.getElementById('ev-status').value = opts.status || 'CONFIRMED';
	document.getElementById('ev-busy').value = (opts.busy === false) ? '0' : '1';
	document.getElementById('ev-categories').value = (opts.categories || []).join(', ');
	document.getElementById('ev-repeat').value = opts.rrule || '';
	const defaultReminder=opts.allDay?calendarSettings.default_all_day_reminder:calendarSettings.default_timed_reminder; const rem = (opts.reminders && opts.reminders.length) ? String(opts.reminders[0]) : (currentEdit||defaultReminder==null?'':String(defaultReminder));
	document.getElementById('ev-reminder').value = rem;
	document.getElementById('ev-all-day').checked = !!opts.allDay;
	if (opts.start) {
		const s = eventDate(opts.start);
		const en = eventDate(opts.end || opts.start + 3600);
		document.getElementById('ev-start-date').value = dt(s);
		document.getElementById('ev-end-date').value = dt(en);
		document.getElementById('ev-start-time').value = pad2(s.getHours()) + ':' + pad2(s.getMinutes());
		document.getElementById('ev-end-time').value = pad2(en.getHours()) + ':' + pad2(en.getMinutes());
	} else {
		const d = defaultTimes();
		document.getElementById('ev-start-date').value = d.date;
		document.getElementById('ev-end-date').value = d.date;
		document.getElementById('ev-start-time').value = d.startTime;
		document.getElementById('ev-end-time').value = d.endTime;
	}
	toggleAllDayFor('ev');
	populateCalSelect('ev-calendar', opts.calId);
	document.getElementById('modal-delete-btn').classList.toggle('hidden', !currentEdit);
	document.getElementById('event-modal').classList.remove('hidden');
	document.getElementById('ev-title').focus();
}

function showCreateAt(dateStr, allDay, hour, mins, clientX, clientY) {
	if(calendarSettings.simple_editor)openPopover({ x: clientX, y: clientY, dateStr, allDay, hour, mins });else {const st=wallTimeToEpoch(dateStr,pad2(hour===undefined?9:hour)+':'+pad2(mins||0));openFullEditor({allDay:!!allDay,start:st,end:st+3600});}
}
function showCreate() {
	// Toolbar New Event → full editor (richer create); grid clicks use popover
	const d = defaultTimes();
	openFullEditor({
		title: '',
		allDay: false,
		start: wallTimeToEpoch(d.date,d.startTime),
		end: wallTimeToEpoch(d.date,d.endTime),
	});
}

function editEvent(id) {
	const e = findEventById(id);
	if (!e) return;
	openFullEditor({
		editId: id,
		editMeta: {
			master_id: e.master_id || (String(id).includes('@') ? String(id).split('@')[0] : id),
			recurrence_id: e.recurrence_id || 0,
			rrule: e.rrule || '',
		},
		title: e.title || '',
		description: e.description || '',
		location: e.location || '',
		start: e.start,
		end: e.end || e.start + 3600,
		allDay: !!e.all_day,
		calId: e.calendar || 'default',
		status: e.status || 'CONFIRMED',
		busy: e.busy !== false,
		categories: e.categories || [],
		rrule: e.rrule || '',
		reminders: e.reminders || [],
	});
}

function toggleAllDayFor(prefix) {
	const ad = document.getElementById(prefix + '-all-day').checked;
	document.getElementById(prefix + '-start-time').classList.toggle('hidden', ad);
	document.getElementById(prefix + '-end-time').classList.toggle('hidden', ad);
}
function toggleAllDay() { toggleAllDayFor('ev'); }

function populateCalendarSelect(selected) {
	populateCalSelect('ev-calendar', selected);
}

function readReminders() {
	const v = document.getElementById('ev-reminder').value;
	if (v === '' || v === null) return [];
	return [parseInt(v, 10)];
}

async function saveEvent() {
	let t; try { t=readFormTimes('ev'); } catch(e) { DarUI.toast(e.message, {tone: 'error'}); return; }
	const cal = calendars.find(c => c.id === t.calId);
	const data = {
		title: document.getElementById('ev-title').value.trim() || 'Untitled event',
		description: document.getElementById('ev-desc').value.trim(),
		location: document.getElementById('ev-location').value.trim(),
		start: t.start,
		end: t.end,
		all_day: t.allDay,
		color: (cal && cal.color) || '#f5c2e7',
		calendar: t.calId,
		rrule: document.getElementById('ev-repeat').value || '',
		busy: document.getElementById('ev-busy').value !== '0',
		status: document.getElementById('ev-status').value || 'CONFIRMED',
		categories: parseCategories(document.getElementById('ev-categories').value),
		reminders: readReminders(),
		timezone: activeCalendarTimezone() || Intl.DateTimeFormat().resolvedOptions().timeZone,
	};
	if (currentEdit) {
		data.id = currentEdit;
		const meta = currentEditMeta || {};
		const wasRecurring = !!(meta.rrule);
		if (wasRecurring) {
			const scope = await askScope('edit');
			if (!scope) return;
			data.scope = scope;
			if (meta.master_id) data.master_id = meta.master_id;
			if (meta.recurrence_id) data.recurrence_id = meta.recurrence_id;
		} else {
			data.scope = 'all';
		}
	}
	const path = currentEdit ? '/update' : '/create';
	api('POST', path, data).then(() => {
		closeModal();
		closeDetail();
		loadData();
	}).catch(err => DarUI.toast('Failed to save: ' + err.message, {tone: 'error'}));
}

function deleteCurrentEvent() {
	if (currentEdit) deleteEvent(currentEdit);
}
async function deleteEvent(id) {
	const e = findEventById(id);
	let scope = 'all';
	let recId = 0;
	if (e && isRecurringEvent(e)) {
		const s = await askScope('delete');
		if (!s) return;
		scope = s;
		recId = e.recurrence_id || 0;
	} else {
		if (!await DarUI.confirmDialog({title: 'Delete event?', message: 'This event will be permanently removed.', confirmLabel: 'Delete', tone: 'danger'})) return;
	}
	let path = '/delete?id=' + encodeURIComponent(id) + '&scope=' + encodeURIComponent(scope);
	if (recId) path += '&recurrence_id=' + recId;
	api('POST', path).then(() => {
		closeModal();
		closePopover();
		closeDetail();
		loadData();
	}).catch(err => DarUI.toast('Delete failed: ' + err.message, {tone: 'error'}));
}
function closeModal() {
	document.getElementById('event-modal').classList.add('hidden');
	currentEdit = null;
	currentEditMeta = null;
}

// --- Drag move / resize on week & day timed events ---
function snapMinutes(mins) {
	return Math.round(mins / calendarSettings.slot_minutes) * calendarSettings.slot_minutes;
}
function onEventPointerDown(ev, id, mode) {
	ev.stopPropagation();
	ev.preventDefault();
	const e = events.find(x => x.id === id);
	if (!e || e.all_day) return;
	const el = ev.currentTarget.closest('[data-event-id]') || ev.currentTarget;
	dragState = {
		id, mode,
		startY: ev.clientY,
		origStart: e.start,
		origEnd: e.end || e.start + 3600,
		el,
		moved: false,
		day: el.getAttribute('data-day'),
	};
	document.addEventListener('mousemove', onEventPointerMove);
	document.addEventListener('mouseup', onEventPointerUp);
}
function onEventPointerMove(ev) {
	if (!dragState) return;
	const dy = ev.clientY - dragState.startY;
	if (Math.abs(dy) > 3) dragState.moved = true;
	const deltaMin = snapMinutes((dy / HOUR_H) * 60);
	const deltaSec = deltaMin * 60;
	let top, height;
	const origDate=eventDate(dragState.origStart); const origTop = ((origDate.getHours() + origDate.getMinutes() / 60) * HOUR_H);
	const origH = Math.max(20, ((dragState.origEnd - dragState.origStart) / 3600) * HOUR_H);
	if (dragState.mode === 'move') {
		top = Math.max(0, Math.min(24 * HOUR_H - 20, origTop + (deltaMin / 60) * HOUR_H));
		height = origH;
	} else {
		top = origTop;
		height = Math.max(HOUR_H / 4, origH + (deltaMin / 60) * HOUR_H);
	}
	if (dragState.el) {
		dragState.el.style.top = top + 'px';
		dragState.el.style.height = height + 'px';
		dragState.el.style.opacity = '0.85';
	}
	dragState.deltaSec = dragState.mode === 'move' ? deltaSec : 0;
	dragState.resizeDeltaSec = dragState.mode === 'resize' ? deltaSec : 0;
}
function onEventPointerUp(ev) {
	document.removeEventListener('mousemove', onEventPointerMove);
	document.removeEventListener('mouseup', onEventPointerUp);
	const st = dragState;
	dragState = null;
	if (!st) return;
	if (st.moved) {
		suppressClick = true;
		setTimeout(() => { suppressClick = false; }, 0);
	}
	if (!st.moved) return;
	const e = events.find(x => x.id === st.id);
	if (!e) return;
	let start = st.origStart;
	let end = st.origEnd;
	if (st.mode === 'move') {
		start = st.origStart + (st.deltaSec || 0);
		end = st.origEnd + (st.deltaSec || 0);
	} else {
		end = Math.max(start + calendarSettings.slot_minutes * 60, st.origEnd + (st.resizeDeltaSec || 0));
	}
	// Clamp within day roughly
	const dayText=st.day||dt(eventDate(start)); const dayStart=wallTimeToEpoch(dayText,'00:00'); const nextDay=new Date(dayText+'T12:00:00');nextDay.setDate(nextDay.getDate()+1);const dayEnd=wallTimeToEpoch(dt(nextDay),'00:00');
	if (start < dayStart) { end += dayStart - start; start = dayStart; }
	if (end > dayEnd) end = dayEnd;
	if (end <= start) end = start + calendarSettings.slot_minutes * 60;
	const data = {
		id: e.id,
		title: e.title,
		description: e.description || '',
		location: e.location || '',
		start, end,
		all_day: false,
		color: eventColor(e),
		calendar: e.calendar || 'default',
		rrule: e.rrule || '',
		busy: e.busy !== false,
		status: e.status || 'CONFIRMED',
		categories: e.categories || [],
		reminders: e.reminders || [],
	};
	// Dragging a single occurrence of a series edits "this" only.
	if (e.rrule) {
		data.scope = 'this';
		data.master_id = e.master_id || masterIDFromClient(e.id);
		data.recurrence_id = e.recurrence_id || st.origStart;
	}
	api('POST', '/update', data).then(loadData).catch(err => {
		DarUI.toast('Move failed: ' + err.message, {tone: 'error'});
		loadData();
	});
}
function masterIDFromClient(id) {
	const s = String(id);
	const i = s.lastIndexOf('@');
	if (i > 0 && /^\d+$/.test(s.slice(i+1))) return s.slice(0, i);
	return s;
}

async function newCalendar() {
	const name = await DarUI.inputDialog({title: 'New calendar', label: 'Calendar name', placeholder: 'Work', confirmLabel: 'Create'});
	if (!name) return;
	const colors = ['#89b4fa','#a6e3a1','#fab387','#f5c2e7','#cba6f7','#89dceb','#f9e2af'];
	const color = colors[calendars.length % colors.length];
	api('POST', '/calendars', {name, color}).then(loadData).catch(e => DarUI.toast(e.message, {tone: 'error'}));
}
async function newSubscription() {
	const values = await DarUI.formDialog({
		title: 'New calendar subscription',
		confirmLabel: 'Subscribe',
		fields: [
			{name: 'url', label: 'Subscription URL', type: 'url', placeholder: 'https://example.com/calendar.ics'},
			{name: 'name', label: 'Calendar name', value: 'Subscribed calendar'},
		],
	});
	if (!values) return;
	api('POST', '/subscriptions', values)
		.then(loadData).catch(e => DarUI.toast('Could not subscribe: ' + e.message, {tone: 'error'}));
}

function esc(s) {
	const d = document.createElement('div');
	d.textContent = s == null ? '' : String(s);
	return d.innerHTML;
}

function noteText(s) {
	return esc(String(s || '').replace(/\\n/g, '\n'));
}

document.getElementById('ev-all-day').addEventListener('change', () => toggleAllDayFor('ev'));
document.getElementById('pop-all-day').addEventListener('change', () => toggleAllDayFor('pop'));
const reminderOptions='<option value="">No reminder</option><option value="0">At time of event</option><option value="5">5 minutes before</option><option value="10">10 minutes before</option><option value="15">15 minutes before</option><option value="30">30 minutes before</option><option value="60">1 hour before</option><option value="120">2 hours before</option><option value="1440">1 day before</option><option value="10080">1 week before</option>';
document.getElementById('calendar-setting-timed-reminder').innerHTML=reminderOptions; document.getElementById('calendar-setting-all-day-reminder').innerHTML=reminderOptions;
document.querySelectorAll('.calendar-settings-tab').forEach(b=>b.addEventListener('click',()=>showCalendarSettingsSection(b.dataset.calendarSettingsTab)));
document.getElementById('calendar-settings-mobile-tab').addEventListener('change',e=>showCalendarSettingsSection(e.target.value));
document.getElementById('calendar-settings-save').addEventListener('click',saveCalendarSettings);
document.getElementById('calendar-caldav-url').value=location.origin+'/caldav/';
document.getElementById('calendar-import-input').addEventListener('change',async e=>{const file=e.target.files&&e.target.files[0];if(!file)return;const form=new FormData();form.append('file',file);const status=document.getElementById('calendar-settings-status');status.textContent='Importing…';try{const r=await fetch(apiBase+'/import',{method:'POST',body:form});if(!r.ok)throw new Error(await r.text());const data=await r.json();status.textContent='Imported '+data.imported+' event'+(data.imported===1?'':'s');await loadData()}catch(err){status.textContent=err.message||String(err);status.className='text-xs text-error'}finally{e.target.value=''}});
document.getElementById('calendar-menu-save').addEventListener('click',saveCalendarMenu);
document.getElementById('calendar-menu-delete').addEventListener('click',confirmCalendarRemove);
document.getElementById('calendar-menu-refresh').addEventListener('click',refreshCalendarSubscription);
const detailPane=document.getElementById('detail-pane');
const detailPaneResizer=document.getElementById('detail-pane-resizer');
function resizeDetailPane(clientX){const minWidth=288;const maxWidth=Math.min(720,Math.max(minWidth,Math.floor(window.innerWidth*0.7)));const width=Math.max(minWidth,Math.min(maxWidth,window.innerWidth-clientX));detailPane.style.width=width+'px';}
detailPaneResizer.addEventListener('pointerdown',e=>{if(e.button!==0)return;e.preventDefault();detailResizeActive=true;detailPaneResizer.setPointerCapture(e.pointerId);detailPane.classList.add('select-none');resizeDetailPane(e.clientX)});
detailPaneResizer.addEventListener('pointermove',e=>{if(detailResizeActive)resizeDetailPane(e.clientX)});
function stopDetailResize(){if(!detailResizeActive)return;detailResizeActive=false;detailPane.classList.remove('select-none');}
detailPaneResizer.addEventListener('pointerup',stopDetailResize);detailPaneResizer.addEventListener('pointercancel',stopDetailResize);
detailPaneResizer.addEventListener('keydown',e=>{if(e.key!=='ArrowLeft'&&e.key!=='ArrowRight')return;e.preventDefault();const current=detailPane.getBoundingClientRect().width;const delta=e.key==='ArrowLeft'?24:-24;const minWidth=288;const maxWidth=Math.min(720,Math.max(minWidth,Math.floor(window.innerWidth*0.7)));detailPane.style.width=Math.max(minWidth,Math.min(maxWidth,current+delta))+'px';});
window.addEventListener('popstate',()=>{if(location.hash.startsWith('#event-')){const id=decodeURIComponent(location.hash.slice(7));if(findEventById(id))showEventDetail(id)}else if(currentDetailId)closeDetail(true)});

document.addEventListener('mousedown', e => {
	const pop = document.getElementById('event-popover');
	if (!pop.classList.contains('hidden') && !pop.contains(e.target)) closePopover();
	const overflow = document.getElementById('month-overflow-popover');
	if (!overflow.classList.contains('hidden') && !overflow.contains(e.target) && !e.target.closest('[data-month-overflow-trigger="true"]')) closeMonthOverflow();
});

document.addEventListener('keydown', e => {
	if (['INPUT','TEXTAREA','SELECT'].includes(document.activeElement?.tagName)) {
		if (e.key === 'Escape') { closeCalendarMenu(); closeSettings(); closeModal(); closePopover(); closeMonthOverflow(); }
		return;
	}
	const k = e.key.toLowerCase();
	if (k === 'c') { e.preventDefault(); showCreate(); }
	if (k === 'h') { e.preventDefault(); openSettings('shortcuts'); }
	if (k === 't') { e.preventDefault(); goToday(); }
	if (k === 'd' || e.key === '1') { e.preventDefault(); switchView('day'); }
	if (k === 'w' || e.key === '2') { e.preventDefault(); switchView('week'); }
	if (k === 'm' || e.key === '3') { e.preventDefault(); switchView('month'); }
	if (k === 'y' || e.key === '4') { e.preventDefault(); switchView('year'); }
	if (k === 'l' || e.key === '5') { e.preventDefault(); switchView('list'); }
	if (e.key === 'ArrowLeft' || k === 'k' || k === 'p') { e.preventDefault(); prevPeriod(); }
	if (e.key === 'ArrowRight' || k === 'j' || k === 'n') { e.preventDefault(); nextPeriod(); }
	if (e.key === 'Escape') {
		if (!document.getElementById('calendar-menu-modal').classList.contains('hidden')) closeCalendarMenu();
		else if (!document.getElementById('calendar-settings-modal').classList.contains('hidden')) closeSettings();
		else if (!document.getElementById('scope-modal').classList.contains('hidden')) closeScopeModal();
		else if (!document.getElementById('event-modal').classList.contains('hidden')) closeModal();
		else if (!document.getElementById('event-popover').classList.contains('hidden')) closePopover();
		else if (!document.getElementById('month-overflow-popover').classList.contains('hidden')) closeMonthOverflow();
		else if (currentDetailId) closeDetail();
	}
});

async function init() {
	const params = new URLSearchParams(location.search);
	const v = params.get('view');
	if (v && ['month','week','day','list','year'].includes(v)) currentView = v;
	// Client-side view switch for sidebar links (no full reload; highlight stays in sync)
	document.querySelectorAll('#sidebar nav a').forEach(a => {
		const href = a.getAttribute('href') || '';
		const m = href.match(/[?&]view=(\w+)/);
		if (!m) return;
		a.addEventListener('click', e => {
			e.preventDefault();
			switchView(m[1]);
		});
	});
	try { const [timeValue]=await Promise.all([fetch('/api/settings/time-format').then(r=>r.ok?r.json():null),loadCalendarSettings()]);timeFormat24h=!!(timeValue&&timeValue.time_format_24h); } catch(e) { console.warn(e); }
	const deepDate=(params.get('date')||'').match(/^(\d{4})-(\d{2})-(\d{2})$/);
	currentDate=deepDate?new Date(Number(deepDate[1]),Number(deepDate[2])-1,Number(deepDate[3])):calendarNow(); await loadData();
	if (location.hash.startsWith('#event-')) {
		const eventID = decodeURIComponent(location.hash.slice(7));
		if (findEventById(eventID)) showEventDetail(eventID);
	}
}
init().catch(console.error);
</script>`
}
