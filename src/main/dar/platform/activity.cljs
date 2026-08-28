(ns dar.platform.activity
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            ["node:crypto" :refer [randomUUID]]))

(defn- row->event [^js row]
  {:id (.-id row)
   :appId (.-app_id row)
   :eventType (.-event_type row)
   :subject (js->clj (js/JSON.parse (.-subject_json row)) :keywordize-keys true)
   :createdAt (.-created_at row)
   :actorName (.-actorName row)})

(defn service
  "Creates a user-scoped activity store with a durable delivery outbox."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        insert-event
        (.prepare connection
                  "INSERT INTO activity_events(id, actor_user_id, app_id, event_type, subject_json, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        insert-recipient
        (.prepare connection
                  "INSERT INTO activity_recipients(event_id, user_id) VALUES (?, ?)")
        insert-outbox
        (.prepare connection
                  "INSERT INTO activity_outbox(event_id, attempts, available_at) VALUES (?, 0, ?)")
        select-page
        (.prepare connection
                  "SELECT e.id, e.app_id, e.event_type, e.subject_json, e.created_at, u.display_name AS actorName FROM activity_recipients r JOIN activity_events e ON e.id = r.event_id LEFT JOIN users u ON u.id = e.actor_user_id WHERE r.user_id = ? ORDER BY e.created_at DESC, e.id DESC LIMIT 50")
        select-due
        (.prepare connection
                  "SELECT e.id, e.app_id, e.event_type, e.subject_json, e.created_at, o.attempts FROM activity_outbox o JOIN activity_events e ON e.id = o.event_id WHERE o.available_at <= ? ORDER BY o.available_at, e.id LIMIT 50")
        mark-delivered
        (.prepare connection
                  "DELETE FROM activity_outbox WHERE event_id = ? AND available_at <= ?")
        fail-delivery
        (.prepare connection
                  "UPDATE activity_outbox SET attempts = attempts + 1, available_at = ?, last_error = ? WHERE event_id = ?")
        exists-recipient
        (.prepare connection
                  "SELECT 1 AS ok FROM activity_recipients WHERE event_id = ? AND user_id = ?")]
    #js {:record
         (fn [actor-user-id app-id event-type subject recipient-user-ids]
           (when-not (and (string? actor-user-id) (not (str/blank? app-id))
                          (not (str/blank? event-type)) (some? subject)
                          (sequential? recipient-user-ids)
                          (every? string? recipient-user-ids)
                          (pos? (count recipient-user-ids)))
             (throw (ex-info "Activity event scope is invalid."
                             {:type :invalid-activity})))
           (.write writer
                   (fn [_]
                     (let [id (randomUUID)
                           now (.toISOString (js/Date.))]
                       (.exec connection "BEGIN IMMEDIATE")
                       (try
                         (.run insert-event id actor-user-id app-id event-type
                               (js/JSON.stringify (clj->js subject)) now)
                         (doseq [recipient-user-id recipient-user-ids]
                           (.run insert-recipient id recipient-user-id))
                         (.run insert-outbox id now)
                         (.exec connection "COMMIT")
                         {:id id :appId app-id :eventType event-type
                          :subject subject :createdAt now}
                         (catch :default error
                           (.exec connection "ROLLBACK")
                           (throw error)))))))
         :list
         (fn [user-id]
           (when-not (string? user-id)
             (throw (ex-info "Activity user scope is invalid."
                             {:type :invalid-activity})))
           (mapv row->event (.all select-page user-id)))
         :dueOutbox
         (fn [now]
           (mapv row->event (.all select-due now)))
         :markDelivered
         (fn [event-id now]
           (.write writer
                   (fn [_]
                     (pos? (.-changes (.run mark-delivered event-id now))))))
         :failDelivery
         (fn [event-id available-at error-message]
           (.write writer
                   (fn [_]
                     (.-changes (.run fail-delivery available-at error-message event-id)))))
         :canSee
         (fn [user-id event-id]
           (some? (.get exists-recipient event-id user-id)))}))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js store (.-activity ctx)]
    (.get server "/api/v1/activity"
          (fn [^js request response _next]
            (http/json! response
                        200
                        {:items (.list store (.-id (.-user request)))})))
    nil))

(def activity-plugin
  (cordis/plugin
   {:name "dar-activity"
    :inject ["database" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "activity" (service (.-database ctx)))
      (register-api! ctx _config))}))

(defn- row->notification-page-row [row]
  row)
