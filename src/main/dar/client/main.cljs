(ns dar.client.main
  (:require [clojure.string]
            [dar.client.activity :as activity]
            [dar.client.appearance :as appearance]
            [dar.client.calendar :as calendar]
            [dar.client.chat :as chat]
            [dar.client.todo :as todo]
            [dar.client.notes :as notes]
            [dar.client.prayer :as prayer]
            [dar.client.weather :as weather]
            [dar.client.news :as news]
            [dar.client.email :as email]
            [dar.client.contacts :as contacts]
            [dar.client.files :as files]
            [dar.client.navigation :as navigation]
            [dar.client.home :as home]
            [dar.client.share-visitor :as share-visitor]
            [dar.client.state]
            [dar.client.workspace :as workspace]
            [dar.client.ui :as ui]
            [dar.cordis :as cordis]
            [re-frame.core :as frame]
            [reagent.core :as r]
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
   ["/chat" {:name :chat :label "Chat"}]
   ["/settings" {:name :settings :label "Settings"}]
   ["/login" {:name :login :label "Sign in"}]
   ;; Public share visitors render outside the authenticated shell.
   ["/share/:token" {:name :share-visitor :label "Shared with you"}]])

(def app-router (router/router routes))

(defonce client-context (atom nil))
(defonce react-root (atom nil))

(defn- route-name []
  (or (some-> (router/match-by-path app-router (.-pathname js/location))
              :data
              :name)
      :home))

(defn- remember-login-route! [_event]
  (try
    (.setItem js/sessionStorage "dar-login-return"
      (js/JSON.stringify #js {:path (str (.-pathname js/location) (.-search js/location) (.-hash js/location))
                              :at (js/Date.now)}))
    (catch :default _ nil)))

(defn- restore-login-route!
  "The OIDC callback returns to Home. Consume a recent tab-local intent before
  initializing the SPA, retaining private deep links without an open redirect."
  []
  (try
    (when-let [raw (.getItem js/sessionStorage "dar-login-return")]
      (.removeItem js/sessionStorage "dar-login-return")
      (when (= "/" (.-pathname js/location))
        (let [intent (js->clj (js/JSON.parse raw) :keywordize-keys true)
              age (when (number? (:at intent)) (- (js/Date.now) (:at intent)))
              url (when (string? (:path intent)) (js/URL. (:path intent) (.-origin js/location)))
              route (when url (some-> (router/match-by-path app-router (.-pathname url)) :data :name))]
          (when (and url route (not (contains? #{:login :share-visitor} route))
                     (= (.-origin js/location) (.-origin url)) (number? age) (<= 0 age 600000))
            (.replaceState js/history nil "" (str (.-pathname url) (.-search url) (.-hash url)))))))
    (catch :default _ nil)))

(defn- navigate! [event path]
  (when-not (or (.-ctrlKey event) (.-metaKey event) (.-shiftKey event) (.-altKey event))
    (.preventDefault event)
    (.pushState js/history nil "" path)
    (frame/dispatch [:route/navigated (route-name) (.-search js/location)])))

(defn- placeholder [route]
  [:section.sanctuary-empty
   [:div
    [:span.sanctuary-empty-symbol "D"]
    [:h2 (str (:label route) " isn't available yet")]
    [:a {:href "/"} "Back to Home"]]])

(defn- focus-main! [node]
  (when node (.focus node)))

(defn- shell-content []
  (let [active @(frame/subscribe [:route])
        session @(frame/subscribe [:session])
        notifications @(frame/subscribe [:notifications])
        profile (:user session)
        route (or (some #(when (= active (get-in % [1 :name])) (second %)) routes)
                  {:name :home :label "Home"})]
    [:div.sanctuary-app
     [appearance/wallpaper]
     [:a.sanctuary-skip-link {:href "#workspace-content"} "Skip to content"]
     [:aside.sanctuary-rail
      [:a.sanctuary-brand {:href "/" :on-click #(navigate! % "/")}
       [:span "D"] [:strong "Dar"]]
      [navigation/bar :desktop active navigate!]
      [:div.sanctuary-user
       [:span.sanctuary-avatar {:title (str (:displayName profile) (when (:administrator profile) " · Administrator"))}
        (->> (clojure.string/split (or (:displayName profile) "Dar User") #"\s+")
             (take 2)
             (map #(subs % 0 1))
             (apply str)
             clojure.string/upper-case)]
       [:span [:strong (:displayName profile)]
        [:small (when (:administrator profile) "Administrator")]]
       [:button.sanctuary-signout
        {:type "button" :on-click #(frame/dispatch [:auth/logout-requested])}
        "Sign out"]]]
     [:div.sanctuary-workspace
      [:header.sanctuary-topbar
       [:div
        [:h1 (if (= active :home)
               (str "Welcome, " (or (:displayName profile) "home"))
               (:label route))]]
       [:div.sanctuary-actions
        [ui/icon-button {:on-click #(workspace/open! :search)} :search "Search"]
        [:button {:type "button" :aria-label "Notifications"
                  :on-click #(workspace/open! :notifications)}
         [ui/icon-action :notifications "Notifications"]
         (when (pos? (or (:count notifications) 0)) [:span.sanctuary-icon-badge {:aria-hidden true} (:count notifications)])]]]
      [:main.sanctuary-content {:id "workspace-content" :key active :ref focus-main! :tab-index -1}
       (cond
         (= active :home) [home/page]
         (= active :files) [files/page]
         (= active :calendar) [calendar/page]
         (= active :contacts) [contacts/page]
         (= active :activity) [activity/page]
         (= active :chat) [chat/page]
         (= active :todo) [todo/page]
         (= active :notes) [notes/page]
         (= active :prayer) [prayer/page]
         (= active :weather) [weather/page]
         (= active :news) [news/page]
         (= active :email) [email/page]
         (= active :settings) [workspace/settings]
         :else [placeholder route])]]
     [:div.sanctuary-mobile-nav [navigation/bar :mobile active navigate!]]
     [workspace/panels]]))

(def shell
  (r/create-class
   {:component-did-mount workspace/mount!
    :component-will-unmount workspace/unmount!
    :reagent-render shell-content}))

(defn- loading-screen []
  [:main.sanctuary-auth
   [:section.sanctuary-auth-card {:aria-live "polite"}
    [:span.sanctuary-empty-symbol "D"]
    [:h1 "Opening your workspace"]
    [:div.sanctuary-auth-skeleton]
    [:p "Checking your Dar session…"]]])

(defn- login-screen [error]
  [:main.sanctuary-auth
   [:section.sanctuary-auth-card
    [:span.sanctuary-empty-symbol "D"]
    [:span.sanctuary-eyebrow "Your private workspace"]
    [:h1 "Welcome to Dar"]
    [:p "Files, calendars, mail, tasks, notes, and your assistant—kept together on your own instance."]
    (when error
      [:p.sanctuary-auth-error {:role "alert"} error])
    [:a.sanctuary-login-action {:href "/api/v1/auth/login" :on-click remember-login-route!} "Continue with OpenID Connect"]]])

(defn- visitor-token []
  (when (clojure.string/starts-with? (.-pathname js/location) "/share/")
    (subs (.-pathname js/location) (count "/share/"))))

(defn- app-root []
  @(frame/subscribe [:route])
  (if-let [token (visitor-token)]
    ^{:key token} [share-visitor/page token]
    (let [{:keys [status error]} @(frame/subscribe [:session])]
      (case status
        :authenticated [shell]
        :loading [loading-screen]
        :error [login-screen error]
        [login-screen nil]))))

(def client-plugin
  (cordis/plugin
   {:name "dar-client-shell"
    :apply (fn [ctx _config]
             (cordis/effect!
              ctx
              "dar.client.popstate"
              (fn []
                (let [listener #(frame/dispatch [:route/navigated (route-name) (.-search js/location)])]
                  (.addEventListener js/window "popstate" listener)
                  (fn [] (.removeEventListener js/window "popstate" listener)))))
             nil)}))

(defn init []
  (restore-login-route!)
  (frame/dispatch-sync [:initialize])
  (frame/dispatch-sync [:route/navigated (route-name) (.-search js/location)])
  (let [ctx (cordis/context)
        root (rdom/create-root (.getElementById js/document "app"))]
    (reset! client-context ctx)
    (reset! react-root root)
    (rdom/render root [app-root])
    (cordis/mount! ctx client-plugin)))
