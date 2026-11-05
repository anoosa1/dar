(ns dar.apps.files-http
  (:require [clojure.string :as str]
            [dar.apps.files :as files]
            [dar.apps.files-assistant :as files-assistant]
            [dar.apps.files-dav :as files-dav]
            [dar.cordis :as cordis]
            [dar.platform.auth :as auth]
            [dar.server.http :as http]
            ["#dar-files/http" :as files-http]
            ["node:fs" :as fs]
            ["node:path" :as path]))

;; Composition only: shared authentication, preferences, Activity, and Cordis
;; stay in the platform; all Files request handling lives in TypeScript.
(def upload-path "/api/v1/files/upload")
(def default-upload-body-limit (* 128 1024 1024))

(def files-plugin
  (cordis/plugin
   {:name "dar-files"
    :inject ["database" "atomicFiles" "server" "rateLimiter" "keyring"
             "appPasswords" "contributions" "activity" "preferences" "search" "eventStreams"]
    :apply
    (fn [^js ctx {:keys [dataRoot quotaBytes uploadBodyLimit baseUrl publicRoot]}]
      (let [^js store (files/service dataRoot {:quota-bytes quotaBytes}
                                     (.-database ctx) (.-atomicFiles ctx)
                                     (.-rateLimiter ctx) (.-keyring ctx) (.-search ctx)
                                     (fn [user] (.publish (.-eventStreams ctx) user "files")))
            ^js preferences (.-preferences ctx)
            ^js activity (.-activity ctx)]
        (cordis/effect! ctx "dar.files.operations" (fn [] (fn [] (.dispose store))))
        (-> (.recover store)
            (.then (fn [_]
        (cordis/provide! ctx "files" store)
        (try
          (doseq [^js entry (array-seq (.readdirSync fs (.resolve path dataRoot "users") #js {:withFileTypes true}))]
            (when (.isDirectory entry)
              (-> (.reconcileSearch store (.-name entry)) (.catch (fn [_] nil)))))
          (catch :default _ nil))
        (files-http/registerFilesHttp
         (.-server ctx) (.-typescriptEngine store)
         #js {:userId (fn [^js request] (some-> request .-user .-id))
              :readJson http/read-json!
              :error (fn [error]
                       #js {:type (name (or (:type (ex-data error)) :internal))
                            :message (or (ex-message error) "The Files operation failed.")
                            :details (clj->js (or (ex-data error) {}))})
              :sendError (fn [^js response status code message fields]
                           (http/error! response status code message
                                        (js->clj fields :keywordize-keys true)
                                        (.-requestId response)))
              :getSettings (fn [user] (clj->js (.get preferences user "files" "settings")))
              :setSettings (fn [user settings expected]
                             (-> (.set preferences user "files" "settings"
                                       (js->clj settings :keywordize-keys true) expected)
                                 (.then clj->js)))
              :record (fn [user event vpath recipients target-url]
                        (let [deleted? (contains? #{"fileTrashed" "filePurged"} event)
                              url (or target-url (if deleted? "/files?view=trash"
                                    (str "/files?path=" (js/encodeURIComponent (.dirname path vpath))
                                         "&item=" (js/encodeURIComponent vpath))))]
                          (.record activity user "files" event
                                   {:path vpath :name (.basename path vpath) :url url :deleted deleted?}
                                   (vec (array-seq recipients)))))
              :cookie auth/request-cookie
              :peer (fn [^js request] (or (some-> request .-_req .-socket .-remoteAddress) ""))
              :uploadLimit (or uploadBodyLimit default-upload-body-limit)
              :secureCookies (str/starts-with? (or baseUrl "") "https://")
              :indexFile (when publicRoot (.resolve path publicRoot "index.html"))})
        (files-dav/register-dav-routes! (.-server ctx) store (.-appPasswords ctx) activity)
        (files-assistant/register-tools! (.-contributions ctx) store activity)
        (files-assistant/register-widget! (.-contributions ctx) store)
        nil)))))}))
