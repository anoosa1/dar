(ns dar.apps.files-http
  (:require [clojure.string :as str]
            [dar.apps.files :as files]
            [dar.cordis :as cordis]
            [dar.server.http :as http]
            [malli.core :as m]
            ["node:buffer" :refer [Buffer]]))

;; HTTP surface for the Files app (Stage 1: browse and core mutations).
;; Routes are browser-session authenticated and live under /api/v1 with
;; camelCase JSON. Paths are user-rooted virtual paths ("/a/b").

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
  nil)

(def files-plugin
  (cordis/plugin
   {:name "dar-files"
    :inject ["database" "atomicFiles" "server"]
    :apply
    (fn [^js ctx {:keys [dataRoot quotaBytes uploadBodyLimit]}]
      (let [^js store (files/service dataRoot
                                     {:quota-bytes quotaBytes}
                                     (.-database ctx)
                                     (.-atomicFiles ctx))]
        (cordis/provide! ctx "files" store)
        (register-routes! (.-server ctx) store
                          {:upload-body-limit (or uploadBodyLimit
                                                  default-upload-body-limit)
                           :quota-bytes (or quotaBytes 0)})))}))
