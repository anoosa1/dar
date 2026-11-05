(ns dar.platform.search
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.server.http :as http]))

(defn- escape-like [value]
  (-> value
      (str/replace "\\" "\\\\")
      (str/replace "%" "\\%")
      (str/replace "_" "\\_")))

(defn- row->document [^js row]
  {:provider (.-provider row)
   :id (.-document_id row)
   :title (.-title row)
   :body (.-body row)
   :url (.-url row)
   :generation (.-generation row)
   :updatedAt (.-updated_at row)})

(defn index-in-transaction!
  "For an app already inside the shared writer queue. Keep permission changes
  and their search projection in the same synchronous transaction."
  [^js connection user-id provider id title body url]
  (when-not (and (every? #(and (string? %) (not (str/blank? %))) [user-id provider id])
                 (every? string? [title body url]))
    (throw (ex-info "Search document scope is invalid." {:type :invalid-search-document})))
  (.run (.prepare connection "INSERT INTO search_documents(user_id, provider, document_id, title, body, url, generation, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?) ON CONFLICT(user_id, provider, document_id) DO UPDATE SET title = excluded.title, body = excluded.body, url = excluded.url, generation = search_documents.generation + 1, updated_at = excluded.updated_at")
        user-id provider id title body url (.toISOString (js/Date.))))

(defn delete-in-transaction!
  "The caller must own the shared writer queue, as above."
  [^js connection user-id provider id]
  (.run (.prepare connection "DELETE FROM search_documents WHERE user_id = ? AND provider = ? AND document_id = ?") user-id provider id))

(defn service
  "Creates a user-scoped incremental search index backed by SQLite."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        upsert
        (.prepare connection
                  "INSERT INTO search_documents(user_id, provider, document_id, title, body, url, generation, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?) ON CONFLICT(user_id, provider, document_id) DO UPDATE SET title = excluded.title, body = excluded.body, url = excluded.url, generation = search_documents.generation + 1, updated_at = excluded.updated_at RETURNING generation, updated_at AS updatedAt")
        delete-one
        (.prepare connection
                  "DELETE FROM search_documents WHERE user_id = ? AND provider = ? AND document_id = ?")
        delete-provider
        (.prepare connection
                  "DELETE FROM search_documents WHERE user_id = ? AND provider = ?")
        delete-provider-except-prefix
        (.prepare connection
                  "DELETE FROM search_documents WHERE user_id = ? AND provider = ? AND substr(document_id, 1, ?) != ?")
        select-provider-generation
        (.prepare connection
                  "SELECT COALESCE(
                     (SELECT generation FROM search_provider_generations WHERE user_id = ? AND provider = ?),
                     (SELECT MAX(generation) FROM search_documents WHERE user_id = ? AND provider = ?),
                     0) AS generation")
        upsert-provider-generation
        (.prepare connection
                  "INSERT INTO search_provider_generations(user_id, provider, generation, updated_at) VALUES (?, ?, ?, ?)
                   ON CONFLICT(user_id, provider) DO UPDATE SET generation = excluded.generation, updated_at = excluded.updated_at")
        insert-reconciled
        (.prepare connection
                  "INSERT INTO search_documents(user_id, provider, document_id, title, body, url, generation, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        query
        (.prepare connection
                  "SELECT provider, document_id, title, body, url, generation, updated_at FROM search_documents WHERE user_id = ? AND (title LIKE ? ESCAPE '\\' OR body LIKE ? ESCAPE '\\') ORDER BY CASE WHEN title LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END, updated_at DESC, provider, document_id LIMIT 50")]
    #js {:index
         (fn [user-id provider document-id title body url]
           (when-not (and (string? user-id) (not (str/blank? provider))
                          (not (str/blank? document-id)) (string? title)
                          (string? body) (string? url))
             (throw (ex-info "Search document scope is invalid."
                             {:type :invalid-search-document})))
           (.write writer
                   (fn [_]
                     (let [row (.get upsert user-id provider document-id title body url
                                     (.toISOString (js/Date.)))]
                       {:provider provider :id document-id :title title :body body
                        :url url :generation (.-generation row)
                        :updatedAt (.-updatedAt row)}))))
         :delete
         (fn [user-id provider document-id]
           (when-not (and (string? user-id) (not (str/blank? provider))
                          (not (str/blank? document-id)))
             (throw (ex-info "Search document scope is invalid."
                             {:type :invalid-search-document})))
           (.write writer
                   (fn [_]
                     (.-changes (.run delete-one user-id provider document-id)))))
         :deleteProvider
         (fn [user-id provider]
           (when-not (and (string? user-id) (not (str/blank? provider)))
             (throw (ex-info "Search document scope is invalid."
                             {:type :invalid-search-document})))
           (.write writer
                   (fn [_]
                     (.-changes (.run delete-provider user-id provider)))))
         :reconcileProvider
         (fn [user-id provider documents retained-prefix]
           (when-not (and (string? user-id) (not (str/blank? provider))
                          (or (nil? retained-prefix)
                              (and (string? retained-prefix) (not (str/blank? retained-prefix))))
                          (sequential? documents) (<= (count documents) 5000)
                          (every? (fn [{:keys [id title body url]}]
                                    (and (not (str/blank? id)) (string? title)
                                         (string? body) (string? url)
                                         (or (nil? retained-prefix) (not (str/starts-with? id retained-prefix)))))
                                  documents))
             (throw (ex-info "Search provider reconciliation is invalid."
                             {:type :invalid-search-document})))
           ;; Providers stage filesystem/network work before this call. The
           ;; short synchronous transaction swaps one complete generation, so
           ;; readers see either the old set or the new set and a failed run
           ;; leaves the previous generation intact.
           (.write writer
                   (fn [_]
                     (let [row (.get select-provider-generation user-id provider
                                     user-id provider)
                           generation (inc (.-generation row))
                           now (.toISOString (js/Date.))]
                       (.exec connection "BEGIN IMMEDIATE")
                       (try
                         ;; Some contributions have independently updated,
                         ;; permission-backed documents in the same provider.
                         ;; Preserve that prefix inside this writer transaction
                         ;; so a filesystem scan cannot overwrite newer grants.
                         (if retained-prefix
                           (.run delete-provider-except-prefix user-id provider (count retained-prefix) retained-prefix)
                           (.run delete-provider user-id provider))
                         (doseq [{:keys [id title body url updatedAt]} documents]
                           (.run insert-reconciled user-id provider id title body url
                                 generation (or updatedAt now)))
                         (.run upsert-provider-generation user-id provider generation now)
                         (.exec connection "COMMIT")
                         {:provider provider :generation generation
                          :count (count documents)}
                         (catch :default error
                           (.exec connection "ROLLBACK")
                           (throw error)))))))
         :search
         (fn [user-id term]
           (when-not (string? user-id)
             (throw (ex-info "Search user scope is invalid."
                             {:type :invalid-search-document})))
           (let [clean-term (str/trim (or term ""))]
             (if (str/blank? clean-term)
               []
               (let [pattern (str "%" (escape-like clean-term) "%")]
                 (mapv row->document
                       (.all query user-id pattern pattern pattern))))))}))

(def search-plugin
  (cordis/plugin
   {:name "dar-search"
    :inject ["database" "server"]
    :apply
    (fn [^js ctx _config]
      (let [^js server (.-server ctx)
            store (service (.-database ctx))]
        (cordis/provide! ctx "search" store)
        (.get server "/api/v1/search"
              (fn [^js request response _next]
                (http/json! response
                            200
                            {:items (.search store
                                             (.-id (.-user request))
                                             (.get (.-query request) "q"))})))
        nil))}))
