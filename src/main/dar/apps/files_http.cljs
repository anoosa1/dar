(ns dar.apps.files-http
  (:require [clojure.string :as str]
            [dar.apps.files :as files]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            [malli.core :as m]
            ["node:buffer" :refer [Buffer]]))

;; HTTP surface for the Files app. Routes are browser-session authenticated
;; and live under /api/v1 with camelCase JSON. Paths are user-rooted virtual
;; paths ("/a/b"). Stage 2 adds the editor, versions, tags, comments, and
;; locks on top of the Stage 1 browse and mutation surface.

(def upload-path "/api/v1/files/upload")

(def default-upload-body-limit (* 128 1024 1024))

(def path-schema
  [:and [:string {:min 1 :max 4096}]
   [:fn (fn [value] (str/starts-with? value "/"))]])

(def path-body-schema
  [:map {:closed true}
   [:path path-schema]])

(def two-path-body-schema
  [:map {:closed true}
   [:from path-schema]
   [:to path-schema]])

(def favorite-body-schema
  [:map {:closed true}
   [:path path-schema]
   [:favorited :boolean]])

(def editor-body-schema
  [:map {:closed true}
   [:content :string]
   [:name {:optional true} :string]])

(def tag-body-schema
  [:map {:closed true}
   [:name :string]
   [:color :string]])

(def file-tag-body-schema
  [:map {:closed true}
   [:path path-schema]
   [:tagId :string]
   [:added :boolean]])

(def comment-body-schema
  [:map {:closed true}
   [:path path-schema]
   [:body :string]
   [:parentId {:optional true} :string]])

(defn- if-match [^js request]
  "Reads the If-Match header; the engine owns precondition semantics."
  (.get (.-headers request) "if-match"))

(defn- editor-headers!
  "Editor documents carry their etag and must never be cached."
  [^js response doc]
  (.set (.-headers response) "etag" (str "\"" (:etag doc) "\""))
  (.set (.-headers response) "cache-control" "no-store"))

(defn- validate!
  "Validates one Malli schema, throwing an :invalid error on failure."
  [schema value]
  (when-not (m/validate schema value)
    (throw (ex-info "The request body is invalid." {:type :invalid})))
  value)

(defn- require-session! [^js request response]
  (if-let [user (.-user request)]
    user
    (do (http/error! response 401 "unauthenticated"
                     "Authentication is required." {} (.-requestId response))
        nil)))

(defn- query-path [^js request]
  (let [value (.get (.-query request) "path")]
    (cond
      (str/blank? value) "/"
      (str/starts-with? value "/") value
      :else (throw (ex-info "The path must be absolute."
                            {:type :invalid})))))

(defn- send-error! [response error]
  (let [data (ex-data error)
        request-id (.-requestId response)]
    (case (:type data)
      (:unsafe-path :invalid :invalid-path)
      (http/error! response 400 "invalid" (ex-message error) data request-id)
      :already-exists
      (http/error! response 409 "conflict" (ex-message error) data request-id)
      :not-found
      (http/error! response 404 "notFound" (ex-message error) data request-id)
      :quota-exceeded
      (http/error! response 507 "quotaExceeded" (ex-message error) data request-id)
      :too-large
      (http/error! response 413 "tooLarge" (ex-message error) data request-id)
      :unsupported-media-type
      (http/error! response 415 "unsupportedMediaType"
                   (ex-message error) data request-id)
      :precondition-required
      (http/error! response 428 "preconditionRequired"
                   (ex-message error) data request-id)
      :precondition-failed
      (http/error! response 412 "preconditionFailed"
                   (ex-message error) data request-id)
      :locked
      (http/error! response 423 "locked" (ex-message error) data request-id)
      (http/error! response 500 "internal"
                   "The Files operation failed." nil request-id))))

(defn- wrap-errors
  "Runs a handler, mapping Files engine failures to the error envelope."
  [handler]
  (fn [^js request ^js response _next]
    (when (require-session! request response)
      (-> (js/Promise.resolve)
          (.then (fn [] (handler request response)))
          (.catch (fn [error] (send-error! response error)))))))

(defn- json-body
  "Reads the request body as JSON and validates it against one schema."
  [^js request schema]
  (-> (.json request)
      (.then (fn [body]
               (validate! schema (js->clj body :keywordize-keys true))))))

(defn- register-routes!
  [^js server ^js store {:keys [upload-body-limit quota-bytes]}]
  (.get server "/api/v1/files"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.list store (.-id (.-user request)) (query-path request))
               (.then (fn [items]
                        (http/json! response
                                    {:items (vec items)
                                     :nextCursor nil})))))))
  (.post server "/api/v1/files/folders"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request path-body-schema)
                (.then (fn [value]
                         (.mkdir store (.-id (.-user request)) (:path value))))
                (.then (fn [created]
                         (http/json! response 201 {:path created})))))))
  (.post server "/api/v1/files/files"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request path-body-schema)
                (.then (fn [value]
                         (.touch store (.-id (.-user request)) (:path value))))
                (.then (fn [created]
                         (http/json! response 201 created)))))))
  (.post server upload-path
         (wrap-errors
          (fn [^js request ^js response]
            (let [path-value (query-path request)]
              (-> (.bytes request)
                  (.then (fn [^js bytes]
                           (when (> (.-length bytes) upload-body-limit)
                             (throw (ex-info "The upload is too large."
                                             {:type :too-large
                                              :limit upload-body-limit})))
                           (.save store (.-id (.-user request)) path-value bytes)))
                  (.then (fn [saved]
                           (http/json! response 201 saved))))))))
  (.delete server "/api/v1/files/files"
           (wrap-errors
            (fn [^js request ^js response]
              (-> (.delete store (.-id (.-user request)) (query-path request))
                  (.then (fn [deleted]
                           (http/json! response 200 deleted)))))))
  (.post server "/api/v1/files/rename"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request two-path-body-schema)
                (.then (fn [value]
                         (.rename store (.-id (.-user request))
                                  (:from value) (:to value))))
                (.then (fn [renamed]
                         (http/json! response 200 renamed)))))))
  (.post server "/api/v1/files/copy"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request two-path-body-schema)
                (.then (fn [value]
                         (.copy store (.-id (.-user request))
                                (:from value) (:to value))))
                (.then (fn [copied]
                         (http/json! response 200 copied)))))))
  (.post server "/api/v1/files/favorite"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request favorite-body-schema)
                (.then (fn [value]
                         (.favorite store (.-id (.-user request))
                                    (:path value) (:favorited value))))
                (.then (fn [_]
                         (http/json! response 200 {:ok true})))))))
  (.post server "/api/v1/files/trash/:id/restore"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (.restore store (.-id (.-user request)) (.-id (.-params request)))
                (.then (fn [restored]
                         (http/json! response 200 restored)))))))
  (.delete server "/api/v1/files/trash/:id"
           (wrap-errors
            (fn [^js request ^js response]
              (-> (.purge store (.-id (.-user request)) (.-id (.-params request)))
                  (.then (fn [purged]
                           (http/json! response 200 purged)))))))
  (.get server "/api/v1/files/quota"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.quotaUsage store (.-id (.-user request)))
               (.then (fn [used]
                        (http/json! response
                                    {:used used
                                     :total quota-bytes})))))))
  ;; Editor: read and save text documents. Saves require If-Match; the
  ;; engine rejects a missing matcher with 428 and a stale one with 412.
  (.get server "/api/v1/files/editor"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.readDocument store (.-id (.-user request)) (query-path request))
               (.then (fn [doc]
                        (editor-headers! response doc)
                        (http/json! response doc)))))))
  (.put server "/api/v1/files/editor"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (json-body request editor-body-schema)
               (.then (fn [body]
                        (.saveDocument store (.-id (.-user request))
                                       (query-path request)
                                       {:content (:content body)
                                        :name (:name body)
                                        :if-match (if-match request)})))
               (.then (fn [doc]
                        (editor-headers! response doc)
                        (http/json! response doc)))))))
  ;; Versions: list snapshots, read one raw, restore one over the file.
  (.get server "/api/v1/files/versions"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.listVersions store (.-id (.-user request)) (query-path request))
               (.then (fn [versions]
                        (http/json! response
                                    {:items (vec versions)
                                     :nextCursor nil})))))))
  (.get server "/api/v1/files/versions/:versionId/content"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.readVersion store (.-id (.-user request)) (query-path request)
                             (.-versionId (.-params request)))
               (.then (fn [^js bytes]
                        (set! (.-status response) 200)
                        (.set (.-headers response) "content-type"
                              "application/octet-stream")
                        (.bytes response bytes)
                        nil))))))
  (.post server "/api/v1/files/versions/:versionId/restore"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request path-body-schema)
                (.then (fn [value]
                         (.restoreVersion store (.-id (.-user request))
                                          (:path value)
                                          (.-versionId (.-params request)))))
                (.then (fn [restored]
                         (http/json! response 200 restored)))))))
  ;; Tags: user-owned labels, attachable to any file path.
  (.get server "/api/v1/files/tags"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.listTags store (.-id (.-user request)))
               (.then (fn [tags]
                        (http/json! response
                                    {:items (vec tags)
                                     :nextCursor nil})))))))
  (.post server "/api/v1/files/tags"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request tag-body-schema)
                (.then (fn [value]
                         (.createTag store (.-id (.-user request))
                                     {:name (:name value)
                                      :color (:color value)})))
                (.then (fn [tag]
                         (http/json! response 201 tag)))))))
  (.delete server "/api/v1/files/tags/:tagId"
           (wrap-errors
            (fn [^js request ^js response]
              (-> (.deleteTag store (.-id (.-user request))
                              (.-tagId (.-params request)))
                  (.then (fn [deleted]
                           (http/json! response 200 deleted)))))))
  (.put server "/api/v1/files/file-tags"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (json-body request file-tag-body-schema)
               (.then (fn [value]
                        (.setFileTag store (.-id (.-user request))
                                     (:path value) (:tagId value)
                                     (:added value))))
               (.then (fn [applied]
                        (http/json! response 200 applied)))))))
  ;; Comments: per-path discussion threads.
  (.get server "/api/v1/files/comments"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.listComments store (.-id (.-user request)) (query-path request))
               (.then (fn [comments]
                        (http/json! response
                                    {:items (vec comments)
                                     :nextCursor nil})))))))
  (.post server "/api/v1/files/comments"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request comment-body-schema)
                (.then (fn [value]
                         (.addComment store (.-id (.-user request))
                                      (:path value) (:body value)
                                      (:parentId value))))
                (.then (fn [comment]
                         (http/json! response 201 comment)))))))
  (.delete server "/api/v1/files/comments/:commentId"
           (wrap-errors
            (fn [^js request ^js response]
              (-> (.deleteComment store (.-id (.-user request))
                                  (query-path request)
                                  (.-commentId (.-params request)))
                  (.then (fn [deleted]
                           (http/json! response 200 deleted)))))))
  ;; Locks: explicit advisory locks that block mutations until unlocked.
  (.get server "/api/v1/files/lock"
        (wrap-errors
         (fn [^js request ^js response]
           (-> (.lockInfo store (.-id (.-user request)) (query-path request))
               (.then (fn [info]
                        (http/json! response info)))))))
  (.post server "/api/v1/files/lock"
         (wrap-errors
          (fn [^js request ^js response]
            (-> (json-body request path-body-schema)
                (.then (fn [value]
                         (.lock store (.-id (.-user request)) (:path value))))
                (.then (fn [locked]
                         (http/json! response 200 locked)))))))
  (.delete server "/api/v1/files/lock"
           (wrap-errors
            (fn [^js request ^js response]
              (-> (.unlock store (.-id (.-user request)) (query-path request))
                  (.then (fn [unlocked]
                           (http/json! response 200 unlocked)))))))
  nil)

(def files-plugin
  (cordis/plugin
   {:name "dar-files"
    :inject ["database" "atomicFiles" "server" "rateLimiter" "keyring"]
    :apply
    (fn [^js ctx {:keys [dataRoot quotaBytes uploadBodyLimit]}]
      (let [^js store (files/service dataRoot
                                     {:quota-bytes quotaBytes}
                                     (.-database ctx)
                                     (.-atomicFiles ctx)
                                     (.-rateLimiter ctx)
                                     (.-keyring ctx))]
        (cordis/provide! ctx "files" store)
        (register-routes! (.-server ctx) store
                          {:upload-body-limit (or uploadBodyLimit
                                                  default-upload-body-limit)
                           :quota-bytes (or quotaBytes 0)})))}))
