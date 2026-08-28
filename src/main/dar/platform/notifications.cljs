(ns dar.platform.notifications
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            ["node:crypto" :refer [randomUUID]]))

(defn- row->notification [^js row]
  {:id (.-id row)
   :appId (.-app_id row)
   :kind (.-kind row)
   :payload (js->clj (js/JSON.parse (.-payload_json row)) :keywordize-keys true)
   :readAt (.-read_at row)
   :createdAt (.-created_at row)})

(defn service
  "Creates a user-scoped notification store with stable deduplication."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        select-one
        (.prepare connection
                  "SELECT id, app_id, kind, payload_json, read_at, created_at FROM notifications WHERE user_id = ? AND id = ?")
        select-page
        (.prepare connection
                  "SELECT id, app_id, kind, payload_json, read_at, created_at FROM notifications WHERE user_id = ? ORDER BY created_at DESC, id DESC LIMIT 50")
        select-unread-page
        (.prepare connection
                  "SELECT id, app_id, kind, payload_json, read_at, created_at FROM notifications WHERE user_id = ? AND read_at IS NULL ORDER BY created_at DESC, id DESC LIMIT 50")
        select-dedupe
        (.prepare connection
                  "SELECT id, app_id, kind, payload_json, read_at, created_at FROM notifications WHERE user_id = ? AND dedupe_key = ?")
        insert
        (.prepare connection
                  "INSERT INTO notifications(id, user_id, dedupe_key, app_id, kind, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        mark-read
        (.prepare connection
                  "UPDATE notifications SET read_at = ? WHERE user_id = ? AND id = ? AND read_at IS NULL")
        mark-all-read
        (.prepare connection
                  "UPDATE notifications SET read_at = ? WHERE user_id = ? AND read_at IS NULL")
        delete-one
        (.prepare connection
                  "DELETE FROM notifications WHERE user_id = ? AND id = ?")
        count-unread
        (.prepare connection
                  "SELECT count(*) AS count FROM notifications WHERE user_id = ? AND read_at IS NULL")]
    #js {:create
         (fn [user-id app-id kind payload dedupe-key]
           (when-not (and (string? user-id) (not (str/blank? app-id))
                          (not (str/blank? kind)) (some? payload))
             (throw (ex-info "Notification scope or payload is invalid."
                             {:type :invalid-notification})))
           (.write writer
                   (fn [_]
                     (let [now (.toISOString (js/Date.))]
                       (if (and (string? dedupe-key) (not (str/blank? dedupe-key)))
                         (if-let [^js existing (.get select-dedupe user-id dedupe-key)]
                           (row->notification existing)
                           (let [id (randomUUID)]
                             (.run insert id user-id dedupe-key app-id kind
                                   (js/JSON.stringify (clj->js payload)) now)
                             {:id id :appId app-id :kind kind
                              :payload payload :readAt nil :createdAt now}))
                         (let [id (randomUUID)]
                           (.run insert id user-id nil app-id kind
                                 (js/JSON.stringify (clj->js payload)) now)
                           {:id id :appId app-id :kind kind
                            :payload payload :readAt nil :createdAt now}))))))
         :list
         (fn [user-id unread-only?]
           (when-not (string? user-id)
             (throw (ex-info "Notification user scope is invalid."
                             {:type :invalid-notification})))
           (mapv row->notification
                 (if unread-only?
                   (.all select-unread-page user-id)
                   (.all select-page user-id))))
         :markRead
         (fn [user-id id]
           (when-not (and (string? user-id) (string? id))
             (throw (ex-info "Notification user scope is invalid."
                             {:type :invalid-notification})))
           (.write writer
                   (fn [_]
                     (let [changes (.-changes (.run mark-read (.toISOString (js/Date.)) user-id id))]
                       (if (pos? changes)
                         changes
                         (if-let [^js existing (.get select-one user-id id)]
                           (row->notification existing)
                           (throw (ex-info "The notification does not exist."
                                           {:type :notification-not-found}))))))))
         :markAllRead
         (fn [user-id]
           (when-not (string? user-id)
             (throw (ex-info "Notification user scope is invalid."
                             {:type :invalid-notification})))
           (.write writer
                   (fn [_]
                     (.-changes (.run mark-all-read (.toISOString (js/Date.)) user-id)))))
         :delete
         (fn [user-id id]
           (when-not (and (string? user-id) (string? id))
             (throw (ex-info "Notification user scope is invalid."
                             {:type :invalid-notification})))
           (.write writer
                   (fn [_]
                     (let [changes (.-changes (.run delete-one user-id id))]
                       (if (pos? changes)
                         changes
                         (throw (ex-info "The notification does not exist."
                                         {:type :notification-not-found})))))))
         :unreadCount
         (fn [user-id]
           (when-not (string? user-id)
             (throw (ex-info "Notification user scope is invalid."
                             {:type :invalid-notification})))
           (.-count (.get count-unread user-id)))}))

(defn- send-error! [response error]
  (let [data (ex-data error)
        request-id (.-requestId response)]
    (case (:type data)
      :notification-not-found
      (http/error! response 404 "notFound" (ex-message error) data request-id)
      (http/error! response 400 "invalid" (ex-message error) data request-id))))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js store (.-notifications ctx)]
    (.get server "/api/v1/notifications"
          (fn [^js request response _next]
            (try
              (http/json! response
                          200
                          {:items (.list store
                                         (.-id (.-user request))
                                         (= "true" (.get (.-query request) "unreadOnly")))})
              (catch :default error
                (send-error! response error)))))
    (.get server "/api/v1/notifications/unread-count"
          (fn [^js request response _next]
            (http/json! response
                        200
                        {:count (.unreadCount store (.-id (.-user request)))})))
    (.post server "/api/v1/notifications/:id/read"
           (fn [^js request response _next]
             (-> (.markRead store (.-id (.-user request)) (.-id (.-params request)))
                 (.then (fn [_]
                          (http/json! response 200 {:ok true})))
                 (.catch (fn [error]
                           (send-error! response error))))))
    (.post server "/api/v1/notifications/read-all"
           (fn [^js request response _next]
             (-> (.markAllRead store (.-id (.-user request)))
                 (.then (fn [changes]
                          (http/json! response 200 {:changed changes})))
                 (.catch (fn [error]
                           (send-error! response error))))))
    (.delete server "/api/v1/notifications/:id"
             (fn [^js request response _next]
               (-> (.delete store (.-id (.-user request)) (.-id (.-params request)))
                   (.then (fn [_]
                            (http/json! response 200 {:ok true})))
                   (.catch (fn [error]
                             (send-error! response error))))))
    nil))

(def notifications-plugin
  (cordis/plugin
   {:name "dar-notifications"
    :inject ["database" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "notifications" (service (.-database ctx)))
      (register-api! ctx _config))}))
