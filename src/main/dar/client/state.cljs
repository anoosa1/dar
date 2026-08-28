(ns dar.client.state
  (:require [re-frame.core :as frame]))

(def default-db
  {:route :home
   :notifications 2
   :profile {:displayName "Anoosa"
             :administrator true}})

(frame/reg-event-db
 :initialize
 (fn [_ _]
   default-db))

(frame/reg-event-db
 :route/navigated
 (fn [db [_ route-name]]
   (assoc db :route route-name)))

(frame/reg-sub
 :route
 (fn [db _]
   (:route db)))

(frame/reg-sub
 :profile
 (fn [db _]
   (:profile db)))

