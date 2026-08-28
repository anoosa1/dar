(ns dar.client.main
  (:require [dar.client.state]
            [dar.cordis :as cordis]
            [re-frame.core :as frame]
            [reagent.dom.client :as rdom]
            [reitit.frontend :as router]))

(def routes
  [["/" {:name :home :label "Home"}]
   ["/files" {:name :files :label "Files"}]
   ["/contacts" {:name :contacts :label "Contacts"}]
   ["/calendar" {:name :calendar :label "Calendar"}]
   ["/todo" {:name :todo :label "Todo"}]
   ["/notes" {:name :notes :label "Notes"}]
   ["/news" {:name :news :label "News"}]
   ["/email" {:name :email :label "Email"}]
   ["/activity" {:name :activity :label "Activity"}]
   ["/prayer" {:name :prayer :label "Prayer"}]
   ["/weather" {:name :weather :label "Weather"}]
   ["/codex" {:name :codex :label "Codex"}]
   ["/settings" {:name :settings :label "Settings"}]])

(def app-router (router/router routes))

(def primary-apps
  [{:name :home :label "Home" :path "/"}
   {:name :files :label "Files" :path "/files"}
   {:name :calendar :label "Calendar" :path "/calendar"}
   {:name :email :label "Email" :path "/email"}
   {:name :codex :label "Codex" :path "/codex"}
   {:name :settings :label "Settings" :path "/settings"}])

(defonce client-context (atom nil))
(defonce react-root (atom nil))

(defn- route-name []
  (or (some-> (router/match-by-path app-router (.-pathname js/location))
              :data
              :name)
      :home))

(defn- navigate! [event path]
  (.preventDefault event)
  (.pushState js/history nil "" path)
  (frame/dispatch [:route/navigated (route-name)]))

(defn- app-link [{:keys [name label path]} active]
  [:a.sanctuary-nav-link
   {:href path
    :aria-current (when (= name active) "page")
    :on-click #(navigate! % path)}
   label])

(defn- dashboard []
  [:div.sanctuary-dashboard
   [:section.sanctuary-metrics
    [:article.sanctuary-panel
     [:span.sanctuary-eyebrow "Weather · Toronto"]
     [:strong "21° · Clear"]
     [:span "Feels like 20°"]]
    [:article.sanctuary-panel
     [:span.sanctuary-eyebrow "Next prayer"]
     [:strong "Asr · 5:14 PM"]
     [:span "In 2 hours 18 minutes"]]
    [:article.sanctuary-panel
     [:span.sanctuary-eyebrow "Inbox"]
     [:strong "7 unread"]
     [:span "2 need a reply"]]]
   [:section.sanctuary-dashboard-grid
    [:article.sanctuary-panel
     [:div.sanctuary-panel-heading [:h2 "Today"] [:a {:href "/calendar"} "Open calendar"]]
     [:ul.sanctuary-list
      [:li [:span [:strong "Design review"] [:small "10:30–11:15 AM"]] [:span "in 24 min"]]
      [:li [:span [:strong "Dar planning"] [:small "2:00–3:00 PM"]] [:span "Team"]]
      [:li [:span [:strong "Ship plugin manifest"] [:small "Due today"]] [:span "Todo"]]]]
    [:article.sanctuary-panel
     [:div.sanctuary-panel-heading [:h2 "Recent files"] [:a {:href "/files"} "All files"]]
     [:ul.sanctuary-list
      [:li [:span [:strong "Rewrite parity ledger"] [:small "Docs · 42 KB"]] [:span "8m"]]
      [:li [:span [:strong "Budget 2026"] [:small "Finance · 318 KB"]] [:span "1h"]]
      [:li [:span [:strong "Calendar audit"] [:small "Design · 60 KB"]] [:span "Yesterday"]]]]]])

(defn- placeholder [route]
  [:section.sanctuary-empty
   [:div
    [:span.sanctuary-empty-symbol "D"]
    [:h2 (str (:label route) " is mapped")]
    [:p "Its existing behavior is preserved in the parity ledger and will be implemented as a complete vertical slice."]]])

(defn- shell []
  (let [active @(frame/subscribe [:route])
        profile @(frame/subscribe [:profile])
        route (or (some #(when (= active (get-in % [1 :name])) (second %)) routes)
                  {:name :home :label "Home"})]
    [:div.sanctuary-app
     [:aside.sanctuary-rail
      [:a.sanctuary-brand {:href "/" :on-click #(navigate! % "/")}
       [:span "D"] [:strong "Dar"]]
      [:nav {:aria-label "Applications"}
       (for [app primary-apps]
         ^{:key (:name app)} [app-link app active])]
      [:div.sanctuary-user
       [:span.sanctuary-avatar "AS"]
       [:span [:strong (:displayName profile)]
        [:small (when (:administrator profile) "Administrator")]]]]
     [:div.sanctuary-workspace
      [:header.sanctuary-topbar
       [:div [:span.sanctuary-eyebrow "Sanctuary workspace"]
        [:h1 (if (= active :home) "Good morning, Anoosa" (:label route))]]
       [:div.sanctuary-actions
        [:button {:type "button"} "Search"]
        [:button {:type "button" :aria-label "Notifications"} "2"]
        [:button.sanctuary-primary {:type "button"} "New"]]]
      [:main.sanctuary-content
       (if (= active :home)
         [dashboard]
         [placeholder route])]]
     [:nav.sanctuary-mobile-nav {:aria-label "Mobile applications"}
      (for [app (take 5 primary-apps)]
        ^{:key (:name app)} [app-link app active])]]))

(def client-plugin
  (cordis/plugin
   {:name "dar-client-shell"
    :apply (fn [ctx _config]
             (cordis/effect!
              ctx
              "dar.client.popstate"
              (fn []
                (let [listener #(frame/dispatch [:route/navigated (route-name)])]
                  (.addEventListener js/window "popstate" listener)
                  (fn [] (.removeEventListener js/window "popstate" listener))))))}))

(defn init []
  (frame/dispatch-sync [:initialize])
  (frame/dispatch-sync [:route/navigated (route-name)])
  (let [ctx (cordis/context)
        root (rdom/create-root (.getElementById js/document "app"))]
    (reset! client-context ctx)
    (reset! react-root root)
    (rdom/render root [shell])
    (cordis/mount! ctx client-plugin)))

