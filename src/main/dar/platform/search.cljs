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
