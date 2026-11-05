(ns dar.platform.preferences
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]))

(defn- valid-namespace? [value]
  (and (string? value)
       (re-matches #"[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)*" value)))

(defn- valid-key? [value]
  (and (string? value)
       (not (str/blank? value))
       (not (str/includes? value "\u0000"))
       (<= (count value) 255)))

(defn- write-preference! [writer select-one upsert update-if-match
                          user-id namespace key encoded expected-revision]
  (.write writer
          (fn [_]
            (let [now (.toISOString (js/Date.))]
              (if (nil? expected-revision)
                (if-let [^js existing (.get select-one user-id namespace key)]
                  (throw (ex-info "An If-Match header is required."
                                  {:type :preference-conflict
                                   :revision (.-revision existing)}))
                  (let [^js inserted (.get upsert user-id namespace key encoded now)]
                    {:revision (.-revision inserted)
                     :updatedAt (.-updatedAt inserted)}))
                (or (when-let [^js row (.get update-if-match encoded now
                                         user-id namespace key expected-revision)]
                      {:revision (.-revision row)
                       :updatedAt (.-updatedAt row)})
                    (if-let [^js existing (.get select-one user-id namespace key)]
                      (throw (ex-info "The preference changed."
                                      {:type :preference-conflict
                                       :revision (.-revision existing)}))
                      (throw (ex-info "The preference does not exist."
                                      {:type :preference-not-found})))))))))

(defn service
  "Creates a user-scoped revisioned preference store backed by SQLite."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        select-all
        (.prepare connection
                  "SELECT key, value_json AS value, revision, updated_at AS updatedAt FROM preferences WHERE user_id = ? AND namespace = ? ORDER BY key")
        select-one
        (.prepare connection
                  "SELECT key, value_json AS value, revision, updated_at AS updatedAt FROM preferences WHERE user_id = ? AND namespace = ? AND key = ?")
        upsert
        (.prepare connection
                  "INSERT INTO preferences(user_id, namespace, key, value_json, revision, updated_at) VALUES (?, ?, ?, ?, 1, ?) ON CONFLICT(user_id, namespace, key) DO UPDATE SET value_json = excluded.value_json, revision = preferences.revision + 1, updated_at = excluded.updated_at RETURNING revision, updated_at AS updatedAt")
        update-if-match
        (.prepare connection
                  "UPDATE preferences SET value_json = ?, revision = revision + 1, updated_at = ? WHERE user_id = ? AND namespace = ? AND key = ? AND revision = ? RETURNING revision, updated_at AS updatedAt")
        delete-if-match
        (.prepare connection
                  "DELETE FROM preferences WHERE user_id = ? AND namespace = ? AND key = ? AND revision = ?")
        row->preference
        (fn [^js row value]
          {:key (.-key row)
           :value (if (some? value)
                    value
                    (js->clj (js/JSON.parse (.-value row)) :keywordize-keys true))
           :revision (.-revision row)
           :updatedAt (.-updatedAt row)})]
    #js {:list
         (fn [user-id namespace]
           (when-not (and (string? user-id) (valid-namespace? namespace))
             (throw (ex-info "Preference scope is invalid."
                             {:type :invalid-preference})))
           (mapv #(row->preference % nil) (.all select-all user-id namespace)))
         :get
         (fn [user-id namespace key]
           (when-not (and (string? user-id) (valid-namespace? namespace) (valid-key? key))
             (throw (ex-info "Preference scope is invalid."
                             {:type :invalid-preference})))
           (when-let [row (.get select-one user-id namespace key)]
             (row->preference row nil)))
         :set
         (fn [user-id namespace key value expected-revision]
           (when-not (and (string? user-id) (valid-namespace? namespace) (valid-key? key)
                          (some? value))
             (throw (ex-info "Preference key or value is invalid."
                             {:type :invalid-preference})))
           (when (and (some? expected-revision)
                      (not (and (int? expected-revision) (pos? expected-revision))))
             (throw (ex-info "A valid If-Match revision is required."
                             {:type :invalid-precondition})))
           (let [encoded (js/JSON.stringify (clj->js value))]
             (.then
              (write-preference! writer select-one upsert update-if-match
                                 user-id namespace key encoded expected-revision)
              (fn [revision]
                {:key key :value value :revision (:revision revision)
                 :updatedAt (:updatedAt revision)}))))
         :delete
         (fn [user-id namespace key expected-revision]
           (when-not (and (string? user-id) (valid-namespace? namespace) (valid-key? key))
             (throw (ex-info "Preference scope is invalid."
                             {:type :invalid-preference})))
           (when-not (and (int? expected-revision) (pos? expected-revision))
             (throw (ex-info "A valid If-Match revision is required."
                             {:type :invalid-precondition})))
           (.write writer
                   (fn [_]
                     (let [changes (.-changes (.run delete-if-match
                                                    user-id namespace key expected-revision))]
                       (if (pos? changes)
                         changes
                         (if-let [^js existing (.get select-one user-id namespace key)]
                           (throw (ex-info "The preference changed."
                                           {:type :preference-conflict
                                            :revision (.-revision existing)}))
                           (throw (ex-info "The preference does not exist."
                                           {:type :preference-not-found}))))))))}))

(defn- preference-etag [revision]
  (str "\"" revision "\""))

(defn- if-match-revision [^js request]
  (let [header (.get (.-headers request) "if-match")]
    (when-not (str/blank? header)
      (let [parsed (js/Number (str/replace (str/trim header) #"\"" ""))]
        (when-not (and (int? parsed) (pos? parsed))
          (throw (ex-info "The If-Match header is invalid."
                          {:type :invalid-precondition})))
        parsed))))

(defn- send-error! [response error]
  (let [data (ex-data error)
        request-id (.-requestId response)]
    (case (:type data)
      :preference-conflict
      (http/error! response 409 "conflict" (ex-message error) data request-id)
      :preference-not-found
      (http/error! response 404 "notFound" (ex-message error) data request-id)
      (http/error! response 400 "invalid" (ex-message error) data request-id))))

(defn- register-api! [^js ctx _config]
  (let [^js server (.-server ctx)
        ^js store (.-preferences ctx)]
    (.get server "/api/v1/preferences/:namespace"
          (fn [^js request response _next]
            (try
              (http/json! response
                          200
                          {:items (.list store (.-id (.-user request))
                                                    (.-namespace (.-params request)))})
              (catch :default error
                (send-error! response error)))))
    (.put server "/api/v1/preferences/:namespace/:key"
          (fn [^js request response _next]
            (-> (http/read-json! request)
                (.then (fn [body]
                         (.set store (.-id (.-user request))
                                          (.-namespace (.-params request))
                                          (.-key (.-params request))
                                          (js->clj body :keywordize-keys true)
                                          (if-match-revision request))))
                (.then (fn [result]
                         (.set (.-headers response) "etag" (preference-etag (:revision result)))
                         (http/json! response 200 (clj->js result))))
                (.catch (fn [error]
                          (send-error! response error))))))
    (.delete server "/api/v1/preferences/:namespace/:key"
             (fn [^js request response _next]
               (-> (js/Promise.resolve)
                   (.then (fn []
                            (.delete store (.-id (.-user request))
                                                (.-namespace (.-params request))
                                                (.-key (.-params request))
                                                (if-match-revision request))))
                   (.then (fn [_]
                            (http/json! response 200 {:ok true})))
                   (.catch (fn [error]
                             (send-error! response error))))))
    nil))

(def preferences-plugin
  (cordis/plugin
   {:name "dar-preferences"
    :inject ["database" "server"]
    :apply
    (fn [^js ctx _config]
      (cordis/provide! ctx "preferences" (service (.-database ctx)))
      (register-api! ctx _config))}))
