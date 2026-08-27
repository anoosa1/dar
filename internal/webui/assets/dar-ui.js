(function () {
  "use strict";
  let dialog;
  let toastRegion;
  let activeResolve;
  let restoreFocus;

  function node(tag, classes, text) {
    const element = document.createElement(tag);
    if (classes) element.className = classes;
    if (text !== undefined) element.textContent = text;
    return element;
  }
  function ensure() {
    if (!dialog) {
      dialog = node("dialog","w-[min(32rem,calc(100vw-2rem))] rounded-3xl bg-surface p-0 text-on-surface shadow-2xl backdrop:bg-scrim/50");
      dialog.id = "dar-ui-dialog";
      dialog.addEventListener("cancel", event => {
        event.preventDefault();
        finish(null);
      });
      dialog.addEventListener("keydown", event => {
        if (event.key === "Escape") {
          event.preventDefault();
          finish(null);
        }
      });
      document.body.append(dialog);
    }
    if (!toastRegion) {
      toastRegion = node("div","pointer-events-none fixed inset-x-4 bottom-5 z-[100] flex flex-col items-center gap-2");
      toastRegion.id = "dar-ui-toasts";
      toastRegion.setAttribute("aria-live","polite");
      toastRegion.setAttribute("aria-atomic","true");
      document.body.append(toastRegion);
    }
  }
  function finish(value) {
    if (!dialog?.open) return;
    dialog.close();
    const resolve = activeResolve;
    activeResolve = null;
    dialog.replaceChildren();
    restoreFocus?.focus();
    restoreFocus = null;
    resolve?.(value);
  }
  function shell(options) {
    ensure();
    restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const form = node("form","p-5");
    form.method = "dialog";
    const header = node("header","mb-4 flex items-start gap-3");
    const headingBox = node("div","min-w-0 flex-1");
    const heading = node("h2","text-xl font-semibold",options.title || "Confirm");
    heading.id = "dar-ui-dialog-title";
    headingBox.append(heading);
    if (options.message) headingBox.append(node("p","mt-2 text-sm text-on-surface-variant",options.message));
    const close = node("button","h-10 w-10 shrink-0 rounded-full hover:bg-surface-container-high");
    close.type = "button"; close.setAttribute("aria-label","Close dialog");
    close.append(node("i","fa-solid fa-xmark"));
    close.onclick = () => finish(null);
    header.append(headingBox,close);
    form.append(header);
    dialog.setAttribute("aria-labelledby",heading.id);
    dialog.replaceChildren(form);
    return form;
  }
  function actions(form, options, submit) {
    const footer = node("footer","mt-5 flex justify-end gap-2");
    const cancel = node("button","h-10 rounded-full px-4 hover:bg-surface-container-high",options.cancelLabel || "Cancel");
    cancel.type = "button"; cancel.onclick = () => finish(null);
    const confirm = node("button","h-10 rounded-full px-5 font-medium",options.confirmLabel || "Confirm");
    confirm.type = "submit";
    if (options.tone === "danger") confirm.classList.add("bg-error","text-on-error");
    else confirm.classList.add("bg-primary","text-on-primary");
    footer.append(cancel,confirm);
    form.append(footer);
    form.onsubmit = event => {
      event.preventDefault();
      submit(confirm);
    };
  }
  function open(run) {
    ensure();
    if (dialog.open) finish(null);
    return new Promise(resolve => {
      activeResolve = resolve;
      run();
      dialog.showModal();
    });
  }
  function confirmDialog(options) {
    options = options || {};
    return open(() => {
      const form = shell(options);
      actions(form,options,()=>finish(true));
    }).then(value => value === true);
  }
  function inputDialog(options) {
    options = options || {};
    return open(() => {
      const form = shell(options);
      const label = node("label","block text-sm font-medium",options.label || "Value");
      const input = node("input","mt-2 h-11 w-full rounded-full bg-surface-container px-4 outline-none focus:ring-2 focus:ring-primary");
      input.type = options.type || "text";
      input.value = options.value || "";
      input.placeholder = options.placeholder || "";
      if (options.required !== false) input.required = true;
      if (options.maxLength) input.maxLength = options.maxLength;
      label.append(input);
      const error = node("p","mt-2 hidden text-sm text-error");
      form.append(label,error);
      actions(form,options,button=>{
        const value=input.value.trim();
        if (input.required && !value) {
          error.textContent=options.requiredMessage || "This field is required.";
          error.classList.remove("hidden"); input.focus(); return;
        }
        button.disabled=true; finish(value);
      });
      setTimeout(()=>{input.focus();input.select()},0);
    });
  }
  function formDialog(options) {
    options = options || {};
    return open(() => {
      const form=shell(options);
      const fields=node("div","space-y-4");
      const inputs={};
      for (const field of options.fields || []) {
        const label=node("label","block text-sm font-medium",field.label || field.name);
        let input;
        if (field.type === "textarea") {
          input=node("textarea","mt-2 min-h-24 w-full rounded-2xl bg-surface-container px-4 py-3 outline-none focus:ring-2 focus:ring-primary");
        } else if (field.type === "select") {
          input=node("select","mt-2 h-11 w-full rounded-full bg-surface-container px-4 outline-none focus:ring-2 focus:ring-primary");
          for (const choice of field.options || []) {
            const option=node("option","",choice.label);
            option.value=choice.value;
            input.append(option);
          }
        } else {
          input=node("input","mt-2 h-11 w-full rounded-full bg-surface-container px-4 outline-none focus:ring-2 focus:ring-primary");
          input.type=field.type || "text";
        }
        input.value=field.value || ""; input.placeholder=field.placeholder || "";
        input.required=field.required !== false;
        if (field.maxLength) input.maxLength=field.maxLength;
        inputs[field.name]=input; label.append(input); fields.append(label);
      }
      const error=node("p","hidden text-sm text-error");
      form.append(fields,error);
      actions(form,options,button=>{
        const result={};
        for (const [name,input] of Object.entries(inputs)) {
          const value=input.value.trim();
          if (input.required && !value) {
            error.textContent="Complete every required field.";
            error.classList.remove("hidden"); input.focus(); return;
          }
          result[name]=value;
        }
        button.disabled=true; finish(result);
      });
      setTimeout(()=>Object.values(inputs)[0]?.focus(),0);
    });
  }
  function toast(message, options) {
    ensure();
    options=options || {};
    const item=node("div","pointer-events-auto max-w-xl rounded-full px-5 py-3 text-sm shadow-2xl",String(message || ""));
    if (options.tone === "error") {
      item.classList.add("bg-error","text-on-error");
      toastRegion.setAttribute("aria-live","assertive");
    } else {
      item.classList.add("bg-on-surface","text-surface");
      toastRegion.setAttribute("aria-live","polite");
    }
    toastRegion.append(item);
    setTimeout(()=>item.remove(),options.duration || 4000);
  }
  window.DarUI={confirmDialog,inputDialog,formDialog,toast};
})();

// Shared Nextcloud-style sidebar and master-detail behavior.
(function () {
  "use strict";
  const compactQuery = matchMedia("(max-width: 1023px)");
  const detailOpeners = new WeakMap();

  function element(value) {
    if (value instanceof HTMLElement) return value;
    return typeof value === "string" ? document.getElementById(value) : null;
  }

  function shellFor(target) {
    return target?.closest("[data-dar-shell]") || target?.parentElement;
  }

  function sidebarControls(sidebar) {
    const shell = shellFor(sidebar);
    const id = sidebar.id;
    return {
      shell,
      toggle: shell?.querySelector('[data-dar-sidebar-toggle][aria-controls="' + CSS.escape(id) + '"]') ||
        shell?.querySelector("[data-dar-sidebar-toggle]"),
      backdrop: shell?.querySelector("[data-dar-sidebar-backdrop]")
    };
  }

  function setSidebar(sidebar, open, remember) {
    const controls = sidebarControls(sidebar);
    if (compactQuery.matches) {
      sidebar.dataset.darSidebarOpen = String(open);
      sidebar.removeAttribute("data-dar-sidebar-collapsed");
      if (controls.backdrop) {
        controls.backdrop.hidden = !open;
        controls.backdrop.dataset.darSidebarOpen = String(open);
      }
    } else {
      sidebar.dataset.darSidebarCollapsed = String(!open);
      sidebar.removeAttribute("data-dar-sidebar-open");
      if (controls.backdrop) {
        controls.backdrop.hidden = true;
        controls.backdrop.removeAttribute("data-dar-sidebar-open");
      }
      if (remember) {
        localStorage.setItem("dar.sidebar." + sidebar.dataset.darSidebarKey + ".collapsed", String(!open));
      }
    }
    if (controls.toggle) {
      const label = sidebar.getAttribute("aria-label") || "navigation";
      controls.toggle.setAttribute("aria-expanded", String(open));
      controls.toggle.setAttribute("aria-label", (open ? "Close " : "Open ") + label);
      controls.toggle.innerHTML = open
        ? '<i class="fa-solid ' + (document.dir === "rtl" ? "fa-chevron-right" : "fa-chevron-left") + '"></i>'
        : '<i class="fa-solid fa-bars"></i>';
    }
    sidebar.setAttribute("aria-hidden", String(!open));
  }

  function initializeSidebar(sidebar, index) {
    if (!sidebar.id) sidebar.id = "dar-sidebar-" + index;
    if (!sidebar.dataset.darSidebarKey) sidebar.dataset.darSidebarKey = sidebar.id;
    const shell = shellFor(sidebar);
    if (!shell) return;
    shell.setAttribute("data-dar-shell", "");
    let header = sidebar.querySelector(":scope > [data-dar-sidebar-header]");
    if (!header) {
      header = document.createElement("header");
      header.dataset.darSidebarHeader = "";
      const title = document.createElement("span");
      title.dataset.darSidebarTitle = "";
      title.textContent = (sidebar.getAttribute("aria-label") || "Navigation").replace(/\s+navigation$/i,"");
      header.append(title);
      sidebar.insertBefore(header,sidebar.firstChild);
    }
    let toggle = shell.querySelector('[data-dar-sidebar-toggle][aria-controls="' + CSS.escape(sidebar.id) + '"]');
    if (!toggle) {
      toggle = document.createElement("button");
      toggle.type = "button";
      toggle.dataset.darSidebarToggle = "";
      toggle.className = "hover:bg-surface-container-highest";
      toggle.setAttribute("aria-controls", sidebar.id);
      shell.append(toggle);
    }
    const toggleHost = shell.querySelector("[data-dar-sidebar-toggle-host]");
    if (toggleHost) toggleHost.prepend(toggle);
    let backdrop = shell.querySelector("[data-dar-sidebar-backdrop]");
    if (!backdrop) {
      backdrop = document.createElement("div");
      backdrop.dataset.darSidebarBackdrop = "";
      backdrop.hidden = true;
      shell.append(backdrop);
    }
    toggle.onclick = () => {
      const open = toggle.getAttribute("aria-expanded") !== "true";
      setSidebar(sidebar, open, true);
      toggle.focus();
    };
    backdrop.onclick = () => {
      setSidebar(sidebar, false, false);
      toggle.focus();
    };
    const stored = localStorage.getItem("dar.sidebar." + sidebar.dataset.darSidebarKey + ".collapsed");
    setSidebar(sidebar, compactQuery.matches ? false : stored !== "true", false);
  }

  function setMastersInert(pane, inert) {
    const shell = shellFor(pane);
    shell?.querySelectorAll("[data-dar-master-pane]").forEach(master => {
      if (master === pane) return;
      master.inert = inert;
      if (inert) master.setAttribute("aria-hidden", "true");
      else master.removeAttribute("aria-hidden");
    });
  }

  function openDetail(target, opener) {
    const pane = element(target);
    if (!pane) return;
    const options = opener instanceof HTMLElement ? {returnFocus:opener} : (opener || {});
    if (options.returnFocus instanceof HTMLElement) detailOpeners.set(pane, options.returnFocus);
    pane.dataset.darDetailOpen = "true";
    pane.classList.remove("hidden");
    pane.classList.add("flex");
    if (compactQuery.matches) {
      pane.setAttribute("role", "dialog");
      pane.setAttribute("aria-modal", "true");
      setMastersInert(pane, true);
      pane.querySelector("[data-dar-detail-back]")?.focus();
    } else {
      pane.setAttribute("role", "region");
      pane.removeAttribute("aria-modal");
    }
  }

  function closeDetail(target, options) {
    const pane = element(target);
    if (!pane) return;
    pane.dataset.darDetailOpen = "false";
    pane.removeAttribute("aria-modal");
    pane.setAttribute("role", "region");
    setMastersInert(pane, false);
    if (compactQuery.matches) {
      pane.classList.add("hidden");
      pane.classList.remove("flex");
    }
    if (options?.restoreFocus !== false) detailOpeners.get(pane)?.focus();
  }

  function syncResponsiveState() {
    document.querySelectorAll("[data-dar-sidebar]").forEach(sidebar => {
      if (compactQuery.matches) {
        setSidebar(sidebar, false, false);
      } else {
        const stored = localStorage.getItem("dar.sidebar." + sidebar.dataset.darSidebarKey + ".collapsed");
        setSidebar(sidebar, stored !== "true", false);
      }
    });
    document.querySelectorAll("[data-dar-detail-pane]").forEach(pane => {
      if (compactQuery.matches) pane.style.width = "";
      if (pane.dataset.darDetailOpen === "true") openDetail(pane);
      else closeDetail(pane, {restoreFocus:false});
    });
  }

  function initialize() {
    document.querySelectorAll("[data-dar-sidebar]").forEach(initializeSidebar);
    document.querySelectorAll("[data-dar-detail-back]").forEach(button => {
      button.addEventListener("click", () => {
        const pane = button.closest("[data-dar-detail-pane]");
        pane?.dispatchEvent(new CustomEvent("dar:detail-back", {bubbles:true}));
      });
    });
    compactQuery.addEventListener("change", syncResponsiveState);
    document.addEventListener("keydown", event => {
      if (event.key !== "Escape") return;
      const openSidebar = [...document.querySelectorAll('[data-dar-sidebar][data-dar-sidebar-open="true"]')].at(-1);
      if (openSidebar) {
        event.preventDefault();
        setSidebar(openSidebar, false, false);
        sidebarControls(openSidebar).toggle?.focus();
        return;
      }
      const detail = [...document.querySelectorAll('[data-dar-detail-pane][data-dar-detail-open="true"]')].at(-1);
      if (compactQuery.matches && detail) {
        event.preventDefault();
        const back = detail.querySelector("[data-dar-detail-back]");
        if (back) back.click();
        else closeDetail(detail);
      }
    });
  }

  window.DarUI = window.DarUI || {};
  window.DarUI.shell = {openDetail,closeDetail,isCompact:() => compactQuery.matches};
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", initialize, {once:true});
  else initialize();
})();

// Transparent shell controls sit directly on the user's wallpaper. Sample the
// exact cover-cropped image beneath each marked element so light regions use a
// dark foreground and dark regions use a light foreground.
(function () {
  "use strict";
  const selector = "[data-wallpaper-contrast]";
  let image;
  let imageURL = "";
  let loadToken = 0;
  let frame = 0;

  function wallpaperURL() {
    const value = getComputedStyle(document.body).backgroundImage.trim();
    if (!value || value === "none") return "";
    const match = value.match(/^url\((?:"([^"]*)"|'([^']*)'|([^)]*))\)$/);
    if (!match) return "";
    try {
      return new URL(match[1] || match[2] || match[3].trim(), location.href).href;
    } catch {
      return "";
    }
  }

  function clear() {
    document.querySelectorAll(selector).forEach(element => {
      element.removeAttribute("data-wallpaper-tone");
    });
  }

  function linear(value) {
    value /= 255;
    return value <= 0.04045 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4);
  }

  function sample(element) {
    const rect = element.getBoundingClientRect();
    if (!image?.naturalWidth || rect.width <= 0 || rect.height <= 0) return;
    const viewportWidth = document.documentElement.clientWidth;
    const viewportHeight = document.documentElement.clientHeight;
    const scale = Math.max(viewportWidth / image.naturalWidth, viewportHeight / image.naturalHeight);
    const drawnWidth = image.naturalWidth * scale;
    const drawnHeight = image.naturalHeight * scale;
    const offsetX = (viewportWidth - drawnWidth) / 2;
    const offsetY = (viewportHeight - drawnHeight) / 2;
    const sourceX = (rect.left - offsetX) / scale;
    const sourceY = (rect.top - offsetY) / scale;
    const sourceWidth = rect.width / scale;
    const sourceHeight = rect.height / scale;
    const canvas = document.createElement("canvas");
    canvas.width = 24;
    canvas.height = 12;
    const context = canvas.getContext("2d", {willReadFrequently: true});
    if (!context) return;
    try {
      context.drawImage(image, sourceX, sourceY, sourceWidth, sourceHeight, 0, 0, canvas.width, canvas.height);
      const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
      let luminance = 0;
      let count = 0;
      for (let index = 0; index < pixels.length; index += 4) {
        const alpha = pixels[index + 3] / 255;
        if (alpha < 0.05) continue;
        luminance += (0.2126 * linear(pixels[index]) + 0.7152 * linear(pixels[index + 1]) + 0.0722 * linear(pixels[index + 2])) * alpha;
        count += alpha;
      }
      if (count) {
        const tone = luminance / count > 0.179 ? "light" : "dark";
        if (element.dataset.wallpaperTone !== tone) element.dataset.wallpaperTone = tone;
      }
    } catch {
      element.removeAttribute("data-wallpaper-tone");
    }
  }

  function apply() {
    frame = 0;
    const nextURL = wallpaperURL();
    if (!nextURL) {
      image = null;
      imageURL = "";
      clear();
      return;
    }
    if (nextURL !== imageURL) {
      imageURL = nextURL;
      image = null;
      clear();
      const token = ++loadToken;
      const nextImage = new Image();
      nextImage.decoding = "async";
      nextImage.onload = () => {
        if (token !== loadToken) return;
        image = nextImage;
        schedule();
      };
      nextImage.onerror = () => {
        if (token === loadToken) clear();
      };
      nextImage.src = nextURL;
      return;
    }
    if (image) document.querySelectorAll(selector).forEach(sample);
  }

  function schedule() {
    if (!frame) frame = requestAnimationFrame(apply);
  }

  addEventListener("resize", schedule, {passive: true});
  addEventListener("scroll", schedule, {passive: true});
  new MutationObserver(schedule).observe(document.documentElement, {
    attributes: true,
    childList: true,
    subtree: true,
    characterData: true
  });
  addEventListener("DOMContentLoaded", schedule, {once: true});
  window.DarWallpaperContrast = {refresh: schedule};
})();

// Presentation-only content color adaptation. Raw CalDAV/CardDAV/tag colors
// remain unchanged; this preserves their hue while matching the active theme's
// tone and chroma.
(function(){
  function hexRGB(value){
    const match=String(value||"").trim().match(/^#([0-9a-f]{6})$/i);
    if(!match)return null;const n=parseInt(match[1],16);return[n>>16,(n>>8)&255,n&255]
  }
  function lch(rgb){
    const linear=v=>{v/=255;return v<=.04045?v/12.92:Math.pow((v+.055)/1.055,2.4)};
    const [r,g,b]=rgb.map(linear),x=(.4124564*r+.3575761*g+.1804375*b)/.95047,y=.2126729*r+.7151522*g+.072175*b,z=(.0193339*r+.119192*g+.9503041*b)/1.08883;
    const f=v=>v>216/24389?Math.cbrt(v):(24389/27*v+16)/116,fx=f(x),fy=f(y),fz=f(z),L=116*fy-16,a=500*(fx-fy),bb=200*(fy-fz);
    return[L,Math.hypot(a,bb),(Math.atan2(bb,a)*180/Math.PI+360)%360]
  }
  function rgbFromLCH(L,C,H){
    for(let attempt=0;attempt<24;attempt++){
      const rad=H*Math.PI/180,a=C*Math.cos(rad),bb=C*Math.sin(rad),fy=(L+16)/116,fx=fy+a/500,fz=fy-bb/200;
      const inv=v=>v*v*v>216/24389?v*v*v:(116*v-16)/(24389/27),x=.95047*inv(fx),y=inv(fy),z=1.08883*inv(fz);
      let values=[3.2404542*x-1.5371385*y-.4985314*z,-.969266*x+1.8760108*y+.041556*z,.0556434*x-.2040259*y+1.0572252*z];
      if(values.every(v=>v>=0&&v<=1)){values=values.map(v=>Math.round(255*(v<=.0031308?12.92*v:1.055*Math.pow(v,1/2.4)-.055)));return"#"+values.map(v=>v.toString(16).padStart(2,"0")).join("")}
      C*=.92
    }
    return L>50?"#ffffff":"#111111"
  }
  function ratio(a,b){
    const lum=value=>{const rgb=hexRGB(value)||[0,0,0];return[.2126,.7152,.0722].reduce((sum,w,i)=>{let v=rgb[i]/255;v=v<=.04045?v/12.92:Math.pow((v+.055)/1.055,2.4);return sum+w*v},0)};
    let x=lum(a),y=lum(b);if(x<y)[x,y]=[y,x];return(x+.05)/(y+.05)
  }
  function tonalize(value){
    const source=hexRGB(value);if(!source)return{background:"rgb(var(--dar-primary-container))",foreground:"rgb(var(--dar-on-primary-container))"};
    const [,sourceC,hue]=lch(source),root=getComputedStyle(document.documentElement),primaryText=root.getPropertyValue("--dar-primary").trim().split(/\s+/).map(Number);
    const primaryC=primaryText.length===3?lch(primaryText)[1]:48,dark=root.colorScheme==="dark"||matchMedia("(prefers-color-scheme:dark)").matches;
    const chroma=Math.max(24,Math.min(sourceC,Math.max(32,primaryC))),background=rgbFromLCH(dark?35:80,chroma,hue);
    let foreground=rgbFromLCH(dark?95:10,Math.min(chroma,12),hue);if(ratio(foreground,background)<4.5)foreground=dark?"#ffffff":"#111111";
    return{background,foreground}
  }
  window.DarTheme={tonalize};
})();
