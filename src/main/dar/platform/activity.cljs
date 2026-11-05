(ns dar.platform.activity
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :refer [randomUUID]]))

(defn- row->event [^js row]
  {:id (.-id row)
   :appId (.-app_id row)
   :eventType (.-event_type row)
   :subject (js->clj (js/JSON.parse (.-subject_json row)) :keywordize-keys true)
   :createdAt (.-created_at row)
   :actorName (.-actorName row)})

(defn- encode-cursor [event]
  (.toString (Buffer.from (js/JSON.stringify
                           #js {:createdAt (:createdAt event) :id (:id event)}))
             "base64url"))

(defn- decode-cursor [value]
  (when-not (str/blank? (or value ""))
    (try
      (let [parsed (js/JSON.parse (.toString (Buffer.from value "base64url") "utf8"))
            created-at (.-createdAt parsed)
            id (.-id parsed)]
        (when-not (and (string? created-at) (not (str/blank? created-at))
                       (string? id) (not (str/blank? id)))
          (throw (js/Error. "invalid cursor")))
        {:created-at created-at :id id})
      (catch :default _
        (throw (ex-info "The activity cursor is invalid."
                        {:type :invalid-activity-cursor}))))))

(defn record-in-transaction!
  "Idempotent event/outbox insertion for portable mutations. The caller owns
  the shared writer queue and transaction; the event ID comes from its journal."
  [^js db id actor app event-type subject recipients]
  (when (seq recipients)
    (let [created (.toISOString (js/Date.))
          result (.run (.prepare db "INSERT OR IGNORE INTO activity_events(id, actor_user_id, app_id, event_type, subject_json, created_at) VALUES (?, ?, ?, ?, ?, ?)")
                       id actor app event-type (js/JSON.stringify (clj->js subject)) created)]
      (when (pos? (.-changes result))
        (doseq [recipient (distinct recipients)]
          (.run (.prepare db "INSERT INTO activity_recipients(event_id, user_id) VALUES (?, ?)") id recipient))
        (.run (.prepare db "INSERT INTO activity_outbox(event_id, attempts, available_at) VALUES (?, 0, ?)") id created)))))

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
                  "SELECT e.id, e.app_id, e.event_type, e.subject_json, e.created_at, u.display_name AS actorName
                   FROM activity_recipients r
                   JOIN activity_events e ON e.id = r.event_id
                   LEFT JOIN users u ON u.id = e.actor_user_id
                   WHERE r.user_id = ?
                     AND (? IS NULL OR e.app_id = ?)
                     AND (? IS NULL OR json_extract(e.subject_json, '$.path') = ?)
                     AND (? IS NULL OR e.created_at < ? OR (e.created_at = ? AND e.id < ?))
                   ORDER BY e.created_at DESC, e.id DESC
                   LIMIT ?")
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
                  "SELECT 1 AS ok FROM activity_recipients WHERE event_id = ? AND user_id = ?")
        recipients (.prepare connection "SELECT user_id FROM activity_recipients WHERE event_id = ? ORDER BY user_id")]
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
                         (doseq [recipient-user-id (distinct recipient-user-ids)]
                           (.run insert-recipient id recipient-user-id))
                         (.run insert-outbox id now)
                         (.exec connection "COMMIT")
                         {:id id :appId app-id :eventType event-type
                          :subject subject :createdAt now}
                         (catch :default error
                           (.exec connection "ROLLBACK")
                           (throw error)))))))
         :page
         (fn [user-id {:keys [app-id path cursor limit]}]
           (when-not (and (string? user-id)
                          (or (nil? app-id) (and (string? app-id) (not (str/blank? app-id))))
                          (or (nil? path) (string? path)))
             (throw (ex-info "Activity page scope is invalid."
                             {:type :invalid-activity})))
           (let [limit (min 100 (max 1 (or limit 50)))
                 decoded (decode-cursor cursor)
                 created-at (:created-at decoded)
                 cursor-id (:id decoded)
                 rows (.all select-page user-id
                            app-id app-id
                            path path
                            created-at created-at created-at cursor-id
                            (inc limit))
                 events (mapv row->event rows)
                 more? (> (count events) limit)
                 items (if more? (subvec events 0 limit) events)]
             {:items items
              :nextCursor (when more? (encode-cursor (peek items)))}))
         :list
         (fn [user-id]
           (when-not (string? user-id)
             (throw (ex-info "Activity user scope is invalid."
                             {:type :invalid-activity})))
           (mapv row->event
                 (.all select-page user-id
                       nil nil nil nil nil nil nil nil 50)))
         :dueOutbox
         (fn [now]
           (mapv (fn [^js row] (assoc (row->event row) :attempts (.-attempts row))) (.all select-due now)))
         :recipients (fn [event-id] (mapv (fn [^js row] (.-user_id row)) (.all recipients event-id)))
         :get (fn [user-id event-id]
                (when-let [row (.get (.prepare connection "SELECT e.*, u.display_name AS actorName FROM activity_events e JOIN activity_recipients r ON r.event_id = e.id LEFT JOIN users u ON u.id = e.actor_user_id WHERE r.user_id = ? AND e.id = ?") user-id event-id)]
                  (row->event row)))
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
    (.get server "/api/v1/activity/events/:id"
          (fn [^js request response _]
            (if-let [event (.get store (.-id (.-user request)) (.-id (.-params request)))]
              (http/json! response event)
              (http/error! response 404 "notFound" "Activity event not found."))))
    (.get server "/api/v1/activity"
          (fn [^js request response _next]
            (try
              (let [query (.-query request)
                    raw-limit (.get query "limit")
                    parsed-limit (when-not (str/blank? (or raw-limit ""))
                                   (js/Number raw-limit))]
                (when (and (some? parsed-limit)
                           (or (.isNaN js/Number parsed-limit)
                               (not (int? parsed-limit))
                               (not (pos? parsed-limit))))
                  (throw (ex-info "The activity limit is invalid."
                                  {:type :invalid-activity})))
                (http/json! response 200
                            (.page store (.-id (.-user request))
                                   {:app-id (.get query "appId")
                                    :path (.get query "path")
                                    :cursor (.get query "cursor")
                                    :limit parsed-limit})))
              (catch :default error
                (http/error! response 400 "invalid" (ex-message error)
                             (ex-data error) (http/request-id))))))
    nil))

(defn delivery!
  "Owns one bounded delivery pass. The returned flush function allows explicit
  recovery passes; concurrent callers join the active pass instead of duplicating it."
  ([ctx store streams] (delivery! ctx store streams #(.now js/Date)))
  ([^js ctx ^js store ^js streams clock]
  (let [closed (atom false) active (atom nil)
        stamp #(.toISOString (js/Date. (clock)))
        flush! (fn []
                 (if @active @active
                 (when-not @closed
                   (let [work (reduce
                               (fn [prior event]
                                 (.then prior
                                        (fn [_]
                                          (when-not @closed
                                            (-> (js/Promise.resolve)
                                                (.then (fn [_]
                                                         (doseq [user-id (.recipients store (:id event))]
                                                           (.publish streams user-id "activity")
                                                           (.publish streams user-id (:appId event))
                                                           (.publish streams user-id "notifications"))
                                                         (.markDelivered store (:id event) (stamp))))
                                                (.catch (fn [_]
                                                          (.failDelivery store (:id event)
                                                                         (.toISOString (js/Date. (+ (clock) (min 300000 (* 1000 (js/Math.pow 2 (min 9 (:attempts event))))))))
                                                                         "Activity delivery failed; retry scheduled."))))))))
                               (js/Promise.resolve nil) (.dueOutbox store (stamp)))
                         result (-> work (.catch (fn [_] nil)) (.finally #(reset! active nil)))]
                     (reset! active result)
                     result))))]
    (cordis/effect! ctx "dar.activity.delivery"
      (fn []
        (let [timer (js/setInterval flush! 1000)]
          (flush!)
          (fn [] (reset! closed true) (js/clearInterval timer)
            (or @active (js/Promise.resolve nil))))))
    flush!)))

(def activity-plugin
  (cordis/plugin
   {:name "dar-activity"
    :inject ["database" "server" "eventStreams"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "activity" (service (.-database ctx)))
      (delivery! ctx (.-activity ctx) (.-eventStreams ctx))
      (register-api! ctx _config))}))

(defn- row->notification-page-row [row]
  row)
