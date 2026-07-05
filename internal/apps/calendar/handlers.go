package calendar

import (
	"html/template"
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
	return webui.Render(w, webui.PageData{
		Title:    "Calendar — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "Month", Icon: "fa-solid fa-calendar-days", Href: "/calendar", Active: true},
			{Label: "Week", Icon: "fa-solid fa-calendar-week", Href: "/calendar?view=week"},
		}, "/settings"),
		Content: template.HTML(calendarHTML()),
	})
}

func calendarHTML() string {
	return `
<div id="calendar-app" class="max-w-4xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<button onclick="prevMonth()" class="w-10 h-10 rounded-full bg-surface-container-high border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center justify-center">
			<i class="fa-solid fa-chevron-left"></i></button>
		<h2 id="month-title" class="text-lg font-semibold flex-1 text-center"></h2>
		<button onclick="nextMonth()" class="w-10 h-10 rounded-full bg-surface-container-high border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center justify-center">
			<i class="fa-solid fa-chevron-right"></i></button>
		<button onclick="showCreate()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-plus"></i> Event</button>
	</div>
	<div class="grid grid-cols-7 text-center text-xs font-bold text-on-surface-variant mb-2">
		<div>Sun</div><div>Mon</div><div>Tue</div><div>Wed</div><div>Thu</div><div>Fri</div><div>Sat</div>
	</div>
	<div id="month-grid" class="grid grid-cols-7 border-t border-l border-outline-variant rounded-xl overflow-hidden"></div>
	<div id="day-events" class="mt-4 hidden">
		<h3 id="day-title" class="text-sm font-semibold mb-2"></h3>
		<div id="day-event-list"></div>
	</div>

	<!-- Event modal -->
	<div id="event-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)closeModal()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-md shadow-2">
			<h2 id="modal-title" class="text-lg font-semibold mb-4">New Event</h2>
			<input id="ev-title" placeholder="Event title" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<div class="flex gap-2 mb-3">
				<input id="ev-start-date" type="date" class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
				<input id="ev-start-time" type="time" class="w-32 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			</div>
			<div class="flex gap-2 mb-3">
				<input id="ev-end-date" type="date" class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
				<input id="ev-end-time" type="time" class="w-32 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			</div>
			<textarea id="ev-desc" placeholder="Description" rows="2" class="w-full px-4 py-2 mb-3 rounded-xl bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 resize-none"></textarea>
			<div class="flex gap-2 justify-end mt-4">
				<button onclick="closeModal()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
				<button onclick="saveEvent()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Save</button>
			</div>
		</div>
	</div>
</div>

<script>
let currentYear, currentMonth, events = [], currentEdit = null;
const apiBase = "/api/calendar";

function dt(d) { return d.getFullYear() + "-" + String(d.getMonth()+1).padStart(2,"0") + "-" + String(d.getDate()).padStart(2,"0"); }

async function api(method, path, body) {
	let opts = {method};
	if (body) { opts.headers = {"Content-Type":"application/json"}; opts.body = JSON.stringify(body); }
	let r = await fetch(apiBase + path, opts);
	return r.json();
}

function init() {
	let now = new Date();
	currentYear = now.getFullYear();
	currentMonth = now.getMonth();
	renderCalendar();
}

function prevMonth() { currentMonth--; if (currentMonth < 0) { currentMonth = 11; currentYear--; } renderCalendar(); }
function nextMonth() { currentMonth++; if (currentMonth > 11) { currentMonth = 0; currentYear++; } renderCalendar(); }

function renderCalendar() {
	let months = ["January","February","March","April","May","June","July","August","September","October","November","December"];
	document.getElementById("month-title").textContent = months[currentMonth] + " " + currentYear;
	let firstDay = new Date(currentYear, currentMonth, 1).getDay();
	let daysInMonth = new Date(currentYear, currentMonth + 1, 0).getDate();
	let today = new Date();
	let todayStr = dt(today);

	let rangeStart = new Date(currentYear, currentMonth, 1);
	rangeStart.setDate(rangeStart.getDate() - 7);
	let rangeEnd = new Date(currentYear, currentMonth + 1, 7);
	let startTs = Math.floor(rangeStart.getTime() / 1000);
	let endTs = Math.floor(rangeEnd.getTime() / 1000);

	api("GET", "/list?start=" + startTs + "&end=" + endTs).then(d => {
		events = d.events || [];
		let grid = document.getElementById("month-grid");
		let html = "";
		for (let i = 0; i < firstDay; i++) html += '<div class="border-r border-b border-outline-variant bg-surface-container p-1 min-h-[80px]"></div>';
		for (let d = 1; d <= daysInMonth; d++) {
			let dateStr = currentYear + "-" + String(currentMonth+1).padStart(2,"0") + "-" + String(d).padStart(2,"0");
			let isToday = dateStr === todayStr;
			let dayEvents = events.filter(e => {
				let es = new Date(e.start * 1000);
				return es.getFullYear() === currentYear && es.getMonth() === currentMonth && es.getDate() === d;
			});
			let dots = dayEvents.map(e => '<div class="w-2 h-2 rounded-full mt-0.5" style="background:' + (e.color || "#6750a4") + '" title="' + esc(e.title) + '"></div>').join("");
			html += '<div class="border-r border-b border-outline-variant p-1 min-h-[80px] cursor-pointer hover:bg-surface-container-high transition-colors" onclick="showDay(\'' + dateStr + '\')">';
			html += '<div class="text-xs font-medium ' + (isToday ? 'bg-primary text-white rounded-full w-6 h-6 inline-flex items-center justify-center' : 'text-on-surface') + '">' + d + '</div>';
			html += '<div class="flex flex-wrap gap-0.5 mt-1">' + dots + '</div>';
			html += '</div>';
		}
		grid.innerHTML = html;
	});
}

function showDay(dateStr) {
	let dayEvents = events.filter(e => {
		let es = new Date(e.start * 1000);
		return dt(es) === dateStr;
	});
	document.getElementById("day-events").classList.remove("hidden");
	document.getElementById("day-title").textContent = dateStr;
	let list = document.getElementById("day-event-list");
	if (dayEvents.length === 0) {
		list.innerHTML = '<p class="text-sm text-on-surface-variant">No events</p>';
		return;
	}
	list.innerHTML = dayEvents.map(e => {
		let start = new Date(e.start * 1000);
		let timeStr = start.toLocaleTimeString([], {hour:"2-digit",minute:"2-digit"});
		return '<div class="flex items-center gap-2 py-1 px-2 rounded-lg hover:bg-surface-container-high cursor-pointer" onclick="editEvent(\'' + e.id + '\')">' +
			'<div class="w-2 h-2 rounded-full" style="background:' + (e.color || "#6750a4") + '"></div>' +
			'<span class="text-xs text-on-surface-variant w-14">' + timeStr + '</span>' +
			'<span class="text-sm text-on-surface">' + esc(e.title) + '</span>' +
			'<button class="ml-auto w-6 h-6 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error" onclick="event.stopPropagation();deleteEvent(\'' + e.id + '\')"><i class="fa-solid fa-xmark text-xs"></i></button>' +
			'</div>';
	}).join("");
}

function showCreate() {
	currentEdit = null;
	document.getElementById("modal-title").textContent = "New Event";
	["ev-title","ev-desc"].forEach(id => document.getElementById(id).value = "");
	document.getElementById("event-modal").classList.remove("hidden");
}

function editEvent(id) {
	let e = events.find(ev => ev.id === id);
	if (!e) return;
	currentEdit = e.id;
	document.getElementById("modal-title").textContent = "Edit Event";
	document.getElementById("ev-title").value = e.title || "";
	document.getElementById("ev-desc").value = e.description || "";
	let s = new Date(e.start * 1000);
	let en = new Date(e.end * 1000);
	document.getElementById("ev-start-date").value = dt(s);
	document.getElementById("ev-start-time").value = s.toTimeString().slice(0,5);
	document.getElementById("ev-end-date").value = dt(en);
	document.getElementById("ev-end-time").value = en.toTimeString().slice(0,5);
	document.getElementById("event-modal").classList.remove("hidden");
}

function closeModal() { document.getElementById("event-modal").classList.add("hidden"); }

function saveEvent() {
	let sd = document.getElementById("ev-start-date").value;
	let st = document.getElementById("ev-start-time").value || "00:00";
	let ed = document.getElementById("ev-end-date").value || sd;
	let et = document.getElementById("ev-end-time").value || "23:59";
	let start = Math.floor(new Date(sd + "T" + st).getTime() / 1000);
	let end = Math.floor(new Date(ed + "T" + et).getTime() / 1000);
	let data = {
		id: currentEdit || "",
		title: document.getElementById("ev-title").value,
		description: document.getElementById("ev-desc").value,
		start: start, end: end, color: "#6750a4"
	};
	let path = currentEdit ? "/update" : "/create";
	api("POST", path, data).then(() => { closeModal(); renderCalendar(); });
}

function deleteEvent(id) {
	if (!confirm("Delete event?")) return;
	api("POST", "/delete?id=" + id).then(() => renderCalendar());
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

init();
</script>`
}
