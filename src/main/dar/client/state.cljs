(ns dar.client.state
  (:require [clojure.string]
            [re-frame.core :as frame]))

(def default-db
  {:route :home
   :notifications {:status :loading :count 0 :items [] :error nil}
   :files nil
   :calendar nil
   :contacts nil
   :session {:status :loading
             :user nil
             :error nil}})

(defn cookie-value
  "Reads one cookie's value; shared by auth flows and app pages that need
  the CSRF token."
  [name]
  (some (fn [part]
          (let [[key value] (clojure.string/split
                             (clojure.string/trim part) #"=" 2)]
            (when (= key name) value)))
        (clojure.string/split (.-cookie js/document) #";")))

(frame/reg-fx
 :auth/load
 (fn [_]
   (-> (js/fetch "/api/v1/session" #js {:credentials "same-origin"})
       (.then (fn [response]
                (if (.-ok response)
                  (-> (.json response)
                      (.then #(frame/dispatch
                               [:auth/loaded
                                (js->clj % :keywordize-keys true)])))
                  (frame/dispatch [:auth/unauthenticated]))))
   (.catch #(frame/dispatch [:auth/failed (.-message %)])))))

(frame/reg-fx
 :auth/logout
 (fn [_]
   (-> (js/fetch "/api/v1/auth/logout"
                 #js {:method "POST"
                      :credentials "same-origin"
                      :headers #js {"x-csrf-token" (or (cookie-value "dar-csrf") "")}})
       (.then (fn [response]
                (if (.-ok response)
                  (frame/dispatch [:auth/unauthenticated])
                  (frame/dispatch [:auth/failed "Logout failed."]))))
       (.catch #(frame/dispatch [:auth/failed (.-message %)])))))

(frame/reg-event-fx
 :initialize
 (fn [_ _]
   {:db default-db
    :auth/load nil}))

(frame/reg-event-db
 :route/navigated
 (fn [db [_ route-name query]]
   (assoc db :route route-name :routeQuery (or query ""))))

(frame/reg-sub :route-query (fn [db _] (:routeQuery db)))

(frame/reg-event-fx
 :auth/loaded
 (fn [{:keys [db]} [_ session]]
   {:db (assoc (if (= (get-in db [:session :user :id]) (get-in session [:user :id]))
                 db (assoc default-db :route (:route db) :routeQuery (:routeQuery db)))
               :session {:status :authenticated
                            :user (:user session)
                            :expiresAt (:expiresAt session)
                            :error nil})}))

(frame/reg-event-db
 :auth/unauthenticated
 (fn [db _]
  ;; Session drafts live in user-scoped sessionStorage. In-memory app data
  ;; must be dropped immediately, before component disposal events arrive.
  (assoc default-db :route (:route db) :routeQuery (:routeQuery db)
          :session {:status :unauthenticated :user nil :error nil}
          :notifications {:status :idle :count 0 :items [] :error nil})))

(frame/reg-event-db
 :auth/failed
 (fn [db [_ message]]
   (assoc default-db :route (:route db) :routeQuery (:routeQuery db)
          :session {:status :error :user nil :error message})))

(frame/reg-event-fx
 :auth/logout-requested
 (fn [_ _]
   {:auth/logout nil}))

(frame/reg-sub
 :route
 (fn [db _]
   (:route db)))

(frame/reg-sub
 :session
 (fn [db _]
   (:session db)))

(frame/reg-sub
 :notifications
 (fn [db _]
   (:notifications db)))
