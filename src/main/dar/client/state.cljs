(ns dar.client.state
  (:require [clojure.string]
            [re-frame.core :as frame]))

(def default-db
  {:route :home
   :notifications {:status :loading :count 0 :items [] :error nil}
   :files nil
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
 :notifications/load
 (fn [_]
   (-> (js/fetch "/api/v1/notifications/unread-count"
                 #js {:credentials "same-origin"})
       (.then (fn [response]
                (if (.-ok response)
                  (-> (.json response)
                      (.then #(frame/dispatch
                               [:notifications/loaded
                                (js->clj % :keywordize-keys true)])))
                  (frame/dispatch [:notifications/failed "Notifications unavailable."]))))
       (.catch #(frame/dispatch [:notifications/failed (.-message %)])))))

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
 (fn [db [_ route-name]]
   (assoc db :route route-name)))

(frame/reg-event-fx
 :auth/loaded
 (fn [{:keys [db]} [_ session]]
   {:db (assoc db :session {:status :authenticated
                            :user (:user session)
                            :expiresAt (:expiresAt session)
                            :error nil})
    :dispatch [:notifications/load]}))

(frame/reg-event-db
 :auth/unauthenticated
 (fn [db _]
   (assoc db :session {:status :unauthenticated :user nil :error nil}
          :notifications {:status :idle :count 0 :items [] :error nil})))

(frame/reg-event-db
 :auth/failed
 (fn [db [_ message]]
   (assoc db :session {:status :error :user nil :error message})))

(frame/reg-event-fx
 :auth/logout-requested
 (fn [_ _]
   {:auth/logout nil}))

(frame/reg-event-fx
 :notifications/load
 (fn [_ _]
   {:notifications/load nil}))

(frame/reg-event-db
 :notifications/loaded
 (fn [db [_ result]]
   (assoc db :notifications {:status :ready
                             :count (:count result)
                             :items (:items result)
                             :error nil})))

(frame/reg-event-db
 :notifications/failed
 (fn [db [_ message]]
   (assoc db :notifications {:status :error :count 0 :items [] :error message})))

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
