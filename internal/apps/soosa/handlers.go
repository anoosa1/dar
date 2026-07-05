package soosa

import (
	"html/template"
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

const soosaDaemonPort = 8081

// handlePage serves the chat UI page.
func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}

	if err := RenderChat(w, r, user.DisplayName); err != nil {
		http.Error(w, "Internal server error", http.StatusInternalServerError)
	}
}

// handleHealth checks Soosa daemon health.
func handleHealth(w http.ResponseWriter, r *http.Request) {
	http.Error(w, "not implemented", http.StatusNotImplemented)
}

// RenderChat renders the chat UI page using the app bar from the request context.
func RenderChat(w io.Writer, r *http.Request, displayName string) error {
	return webui.Render(w, webui.PageData{
		Title:    "Soosa — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: template.HTML(userMenuHTML(displayName)),
		Content:  template.HTML(chatHTML()),
	})
}

func userMenuHTML(displayName string) string {
	initial := "U"
	if len(displayName) > 0 {
		initial = displayName[:1]
	}
	return `<div class="relative">
		<button id="user-menu-button" type="button" onclick="toggleUserMenu()"
			class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer"
			aria-label="User menu">
			<span aria-hidden="true">` + initial + `</span>
		</button>
		<div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
			<div class="px-4 py-2 text-sm font-medium text-on-surface">` + displayName + `</div>
			<a href="/home" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Home</a>
			<a href="/logout" class="block px-4 py-2 text-sm text-error no-underline hover:bg-error-container">Logout</a>
		</div>
	</div>
	<script>
	function toggleUserMenu(){let m=document.getElementById("user-menu");if(m)m.classList.toggle("hidden")}
	document.addEventListener("click",function(e){let b=document.getElementById("user-menu-button");let m=document.getElementById("user-menu");if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add("hidden")});
	</script>`
}

func chatHTML() string {
	return `
<div id="soosa-chat" class="flex flex-col h-[calc(100vh-120px)] max-w-3xl mx-auto">
	<!-- Messages area -->
	<div id="soosa-messages" class="flex-1 overflow-y-auto space-y-4 p-4">
		<div class="flex justify-start">
			<div class="bg-surface-container rounded-2xl rounded-tl-sm px-4 py-3 max-w-[80%]">
				<p class="text-sm text-on-surface">Hello! I'm Soosa, your AI assistant. How can I help you today?</p>
			</div>
		</div>
	</div>

	<!-- Input area -->
	<div class="border-t border-outline-variant p-4">
		<div class="flex gap-3">
			<input id="soosa-input" type="text" placeholder="Ask Soosa anything..."
				class="flex-1 bg-surface-container rounded-full px-5 py-3 text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant"
				onkeydown="if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();sendMessage()}">
			<button onclick="sendMessage()"
				class="w-12 h-12 rounded-full bg-primary text-white inline-flex items-center justify-center border-0 cursor-pointer hover:opacity-90 transition-opacity shrink-0">
				<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="22" y1="2" x2="11" y2="13"></line><polygon points="22 2 15 22 11 13 2 9 22 2"></polygon></svg>
			</button>
		</div>
		<p class="text-xs text-on-surface-variant mt-2 text-center">Soosa can make mistakes. Verify important information.</p>
	</div>
</div>

<script>
let messages = [
	{role:"assistant",content:"Hello! I'm Soosa, your AI assistant. How can I help you today?"}
];

function sendMessage() {
	const input = document.getElementById("soosa-input");
	const text = input.value.trim();
	if (!text) return;
	input.value = "";
	input.disabled = true;

	// Add user message
	messages.push({role:"user",content:text});
	renderMessages();

	// Add loading bubble
	const msgsDiv = document.getElementById("soosa-messages");
	const loadingDiv = document.createElement("div");
	loadingDiv.className = "flex justify-start";
	loadingDiv.id = "soosa-loading";
	loadingDiv.innerHTML = '<div class="bg-surface-container rounded-2xl rounded-tl-sm px-4 py-3"><span class="inline-flex gap-1"><span class="w-2 h-2 rounded-full bg-on-surface-variant animate-pulse"></span><span class="w-2 h-2 rounded-full bg-on-surface-variant animate-pulse" style="animation-delay:0.2s"></span><span class="w-2 h-2 rounded-full bg-on-surface-variant animate-pulse" style="animation-delay:0.4s"></span></span></div>';
	msgsDiv.appendChild(loadingDiv);
	msgsDiv.scrollTop = msgsDiv.scrollHeight;

	// Send to server via fetch + SSE (or simple POST for now)
	fetch("/api/soosa/chat", {
		method:"POST",
		headers:{"Content-Type":"application/json"},
		body:JSON.stringify({messages:messages})
	})
	.then(r => {
		if (!r.ok) return r.text().then(t => {throw new Error(t)});
		const reader = r.body.getReader();
		const decoder = new TextDecoder();
		let aiMsg = {role:"assistant",content:""};
		messages.push(aiMsg);
		document.getElementById("soosa-loading")?.remove();

		// Add AI bubble
		const aiDiv = document.createElement("div");
		aiDiv.className = "flex justify-start";
		aiDiv.id = "soosa-ai-bubble";
		aiDiv.innerHTML = '<div class="bg-surface-container rounded-2xl rounded-tl-sm px-4 py-3 max-w-[80%]"><p class="text-sm text-on-surface whitespace-pre-wrap" id="soosa-ai-text"></p></div>';
		msgsDiv.appendChild(aiDiv);

		function pump() {
			reader.read().then(({done,value}) => {
				if (done) { input.disabled = false; input.focus(); return; }
				const text = decoder.decode(value,{stream:true});
				const lines = text.split("\n");
				for (const line of lines) {
					if (line.startsWith("data: ")) {
						try {
							const d = JSON.parse(line.slice(6));
							if (d.content) {
								aiMsg.content += d.content;
								document.getElementById("soosa-ai-text").textContent = aiMsg.content;
							}
							if (d.done) { input.disabled = false; input.focus(); return; }
						} catch(e) {}
					}
				}
				msgsDiv.scrollTop = msgsDiv.scrollHeight;
				pump();
			});
		}
		pump();
	})
	.catch(err => {
		document.getElementById("soosa-loading")?.remove();
		messages.pop(); // remove placeholder
		const errDiv = document.createElement("div");
		errDiv.className = "flex justify-start";
		errDiv.innerHTML = '<div class="bg-error-container rounded-2xl rounded-tl-sm px-4 py-3 max-w-[80%]"><p class="text-sm text-error">Error: '+err.message+'</p></div>';
		msgsDiv.appendChild(errDiv);
		input.disabled = false;
	});
}

function renderMessages() {
	const div = document.getElementById("soosa-messages");
	div.innerHTML = "";
	for (const m of messages) {
		const row = document.createElement("div");
		const isUser = m.role === "user";
		row.className = "flex " + (isUser ? "justify-end" : "justify-start");
		const bubbleClass = isUser
			? "bg-primary-container rounded-2xl rounded-tr-sm px-4 py-3 max-w-[80%]"
			: "bg-surface-container rounded-2xl rounded-tl-sm px-4 py-3 max-w-[80%]";
		const textClass = isUser ? "text-sm text-on-surface" : "text-sm text-on-surface";
		row.innerHTML = '<div class="'+bubbleClass+'"><p class="'+textClass+' whitespace-pre-wrap">'+escapeHtml(m.content)+'</p></div>';
		div.appendChild(row);
	}
	div.scrollTop = div.scrollHeight;
}

function escapeHtml(s) {
	const d = document.createElement("div");
	d.textContent = s;
	return d.innerHTML;
}
</script>`
}
