(ns dar.apps.files-assistant
  "Files assistant tools and dashboard widget.

  Tools are registered on the contributions registry with Malli object
  schemas, an explicit user scope (every engine call passes the caller's
  id), stable :type-keyed errors, and one effect classification each.
  The widget mirrors the Go baseline's files.favorites list widget."
  (:require [clojure.string :as str]
            [dar.platform.assistant :as assistant]
            [dar.platform.dashboard :as dashboard]
            ["node:buffer" :refer [Buffer]]
            ["node:path" :as node-path]))

(def read-limit-chars
  "Assistant reads truncate text payloads at 64 KiB characters."
  65536)

(def write-limit-bytes
  "Assistant writes cap text files at 1 MiB (the editor limit)."
  (* 1024 1024))

(defn- user-id [user]
  (.-id user))

(defn- reject-binary!
  "Assistant reads are text-only; NUL bytes or invalid UTF-8 are rejected."
  [^js bytes]
  (let [text (.toString bytes "utf8")]
    (try
      (.decode (js/TextDecoder. "utf8" #js {:fatal true}) bytes)
      (catch :default _
        (throw (ex-info "The file is not UTF-8 text."
                        {:type :unsupported-media-type}))))
    (when (str/includes? text "\u0000")
      (throw (ex-info "The file is not UTF-8 text."
                      {:type :unsupported-media-type})))
    text))

(defn- truncate!
  [text]
  (if (> (count text) read-limit-chars)
    {:content (subs text 0 read-limit-chars) :truncated true}
    {:content text :truncated false}))

(defn- valid-name?
  [name]
  (and (string? name)
       (not (str/blank? name))
       (not (str/includes? name "/"))
       (not (str/includes? name "\\"))
       (not= "." name)
       (not= ".." name)))

(defn- clean-path
  "Normalizes an optional path argument to an absolute user-rooted path."
  [path]
  (let [value (if (or (nil? path) (str/blank? path)) "/" (str path))]
    (if (str/starts-with? value "/")
      value
      (throw (ex-info "The path must be absolute." {:type :invalid})))))

(defn- register-tool!
  [registry store id title effect schema invoke]
  (assistant/register-tool! registry id
                            {:name id
                             :title title
                             :effect effect
                             :schema schema
                             :invoke invoke})
  store)

(defn- note-activity!
  "Records one files activity event for the acting user; best-effort."
  [^js activity user-id event-type vpath]
  (when activity
    (-> (.record activity user-id "files" event-type
                 {:path vpath :name (node-path/basename vpath)}
                 [user-id])
        (.catch (fn [_] nil)))))

(defn register-tools!
  "Registers the Files assistant tools on the contributions registry."
  [^js registry ^js store ^js activity]
  (let [register! (partial register-tool! registry store)
        note! (fn [user event-type vpath]
                (note-activity! activity (user-id user) event-type vpath))]

    ;; Reads ------------------------------------------------------------
    (register! "files.list" "List files" "read"
               [:map {:closed true}
                [:path {:optional true} string?]]
               (fn [user {:keys [path]}]
                 (-> (.list store (user-id user) (clean-path path))
                     (.then (fn [items]
                              {:path (or path "/")
                               :items (mapv #(select-keys % [:name :path :isDir :size :mimeType
                                                             :updatedAt :favorited])
                                            (vec items))})))))

    (register! "files.search" "Search files" "read"
               [:map {:closed true}
                [:q {:optional true} string?]
                [:type {:optional true} [:enum "" "folder" "image" "document" "video" "audio"]]
                [:folder {:optional true} string?]
                [:limit {:optional true} pos-int?]
                [:offset {:optional true} nat-int?]]
               (fn [user {:keys [q type folder limit offset]}]
                 (-> (.search store (user-id user)
                              {:q (or q "")
                               :type (or type "")
                               :folder folder
                               :limit limit
                               :offset offset})
                     (.then (fn [result]
                              {:total (:total result)
                               :items (mapv #(select-keys % [:name :path :isDir :size :mimeType
                                                             :updatedAt :favorited])
                                            (:items result))})))))

    (register! "files.read" "Read a text file" "read"
               [:map {:closed true}
                [:path string?]]
               (fn [user {:keys [path]}]
                 (let [vpath (clean-path path)]
                   (-> (.stat store (user-id user) vpath)
                       (.then (fn [entry]
                                (when (:isDir entry)
                                  (throw (ex-info "Folders cannot be read as text."
                                                  {:type :invalid})))
                                (-> (.readFile store (user-id user) vpath)
                                    (.then (fn [^js bytes]
                                             (merge (truncate! (reject-binary! bytes))
                                                    {:path vpath
                                                     :name (:name entry)
                                                     :mimeType (:mimeType entry)
                                                     :size (:size entry)}))))))))))

    ;; Safe writes ------------------------------------------------------
    (register! "files.write-text" "Write a text file" "safeWrite"
               [:map {:closed true}
                [:path string?]
                [:content string?]]
               (fn [user {:keys [path content]}]
                 (let [vpath (clean-path path)]
                   (when (str/includes? content "\u0000")
                     (throw (ex-info "The file is not UTF-8 text."
                                     {:type :unsupported-media-type})))
                   (when (> (.-length (Buffer.from content "utf8")) write-limit-bytes)
                     (throw (ex-info "The content is too large for an assistant write."
                                     {:type :too-large})))
                   (-> (.save store (user-id user) vpath (Buffer.from content "utf8"))
                       (.then (fn [_]
                         (note! user "fileUploaded" vpath)
                         {:path vpath :status "ok"}))))))

    (register! "files.create-folder" "Create a folder" "safeWrite"
               [:map {:closed true}
                [:path string?]]
               (fn [user {:keys [path]}]
                 (let [vpath (clean-path path)]
                   (-> (.mkdir store (user-id user) vpath)
                       (.then (fn [_]
                         (note! user "folderCreated" vpath)
                         {:path vpath :status "ok"}))))))

    (register! "files.rename" "Rename a file or folder" "safeWrite"
               [:map {:closed true}
                [:path string?]
                [:name string?]]
               (fn [user {:keys [path name]}]
                 (when-not (valid-name? name)
                   (throw (ex-info "The filename is invalid." {:type :invalid})))
                 (let [vpath (clean-path path)
                       destination (str (node-path/dirname vpath)
                                        (if (= "/" (node-path/dirname vpath)) "" "/")
                                        name)]
                   (-> (.rename store (user-id user) vpath destination)
                       (.then (fn [_]
                         (note! user "fileRenamed" destination)
                         {:path destination :status "ok"}))))))

    (register! "files.move" "Move a file or folder" "safeWrite"
               [:map {:closed true}
                [:from string?]
                [:to string?]]
               (fn [user {:keys [from to]}]
                 (-> (.rename store (user-id user) (clean-path from) (clean-path to))
                     (.then (fn [_]
                       (note! user "fileRenamed" (clean-path to))
                       {:path (clean-path to) :status "ok"})))))

    (register! "files.copy" "Copy a file or folder" "safeWrite"
               [:map {:closed true}
                [:from string?]
                [:to string?]]
               (fn [user {:keys [from to]}]
                 (-> (.copy store (user-id user) (clean-path from) (clean-path to))
                     (.then (fn [_]
                       (note! user "fileCopied" (clean-path to))
                       {:path (clean-path to) :status "ok"})))))

    (register! "files.favorite" "Set a favorite" "safeWrite"
               [:map {:closed true}
                [:path string?]
                [:favorite boolean?]]
               (fn [user {:keys [path favorite]}]
                 (-> (.favorite store (user-id user) (clean-path path) (boolean favorite))
                     (.then (fn [_] {:status "ok"})))))

    (register! "files.tag" "Add or remove a tag" "safeWrite"
               [:map {:closed true}
                [:path string?]
                [:tagId string?]
                [:action [:enum "add" "remove"]]]
               (fn [user {:keys [path tagId action]}]
                 (-> (.setFileTag store (user-id user) (clean-path path) tagId
                                  (= "add" action))
                     (.then (fn [_] {:status "ok"})))))

    (register! "files.restore" "Restore a trashed file" "safeWrite"
               [:map {:closed true}
                [:id string?]]
               (fn [user {:keys [id]}]
                 (when (str/blank? id)
                   (throw (ex-info "A trash id is required." {:type :invalid})))
                 (-> (.restore store (user-id user) id)
                     (.then (fn [restored]
                       (note! user "fileRestored" (:restored restored))
                        {:status "ok"})))))

    ;; External ---------------------------------------------------------
    (register! "files.share" "Create or remove a share" "external"
               [:map {:closed true}
                [:path {:optional true} string?]
                [:action [:enum "create" "remove"]]
                [:token {:optional true} string?]
                [:password {:optional true} string?]
                [:expiresAt {:optional true} string?]]
               (fn [user {:keys [path action token password expiresAt]}]
                 (if (= "remove" action)
                   (do (when (str/blank? (or token ""))
                         (throw (ex-info "A share token is required."
                                         {:type :invalid})))
                       (-> (.deleteShare store (user-id user) token)
                           (.then (fn [_]
                               (note! user "shareRemoved" (str "/shares/" token))
                                {:status "ok"}))))
                   (let [vpath (clean-path path)]
                     (-> (.createShare store (user-id user) vpath
                                       (cond-> {}
                                         password (assoc :password password)
                                         expiresAt (assoc :expires-at expiresAt)))
                         (.then (fn [share]
                                   (note! user "fileShared" (:path share))
                                  {:token (:token share)
                                   :path (:path share)
                                   :hasPassword (:hasPassword share)})))))))

    (register! "files.trash" "Move a file or folder to trash" "destructive"
               [:map {:closed true}
                [:path string?]]
               (fn [user {:keys [path]}]
                 (let [vpath (clean-path path)]
                   (when (= "/" vpath)
                     (throw (ex-info "The root folder cannot be trashed."
                                     {:type :invalid})))
                   (-> (.delete store (user-id user) vpath)
                       (.then (fn [deleted]
                                 (note! user "fileTrashed" vpath)
                                {:id (:id deleted)
                                 :path vpath
                                 :status "trashed"}))))))

    (register! "files.delete-permanently" "Permanently remove a trashed file"
               "destructive"
               [:map {:closed true}
                [:id string?]]
               (fn [user {:keys [id]}]
                 (when (str/blank? id)
                   (throw (ex-info "A trash id is required." {:type :invalid})))
                 (-> (.purge store (user-id user) id)
                     (.then (fn [purged]
                       (note! user "filePurged" (:path purged))
                       {:status "ok"}))))))
  store)

(def widget-size-limits
  "Favorites shown per widget size (baseline parity: 1/3/6/10)."
  {"compact" 1 "standard" 3 "wide" 6 "large" 10})

(defn- widget-icon
  [entry]
  (cond
    (:isDir entry) "fa-regular fa-folder"
    (str/starts-with? (or (:mimeType entry) "") "image/") "fa-regular fa-image"
    :else "fa-regular fa-file"))

(defn register-widget!
  "Registers the Files favorites widget on the contributions registry."
  [^js registry ^js store]
  (dashboard/register-widget! registry "files.favorites"
                              {:id "files.favorites"
                               :title "Favorite files"
                               :kind "list"
                               :default-size "standard"
                               :supported-sizes ["compact" "standard" "wide" "large"]
                               :fetch
                               (fn [user-id _timezone size]
                                 (-> (.list store user-id "/")
                                     (.then (fn [items]
                                              (let [favorites (->> (vec items)
                                                                   (filter :favorited)
                                                                   (sort (fn [a b]
                                                                      (let [c (compare (:updatedAt b)
                                                                                       (:updatedAt a))]
                                                                        (if (zero? c)
                                                                          (compare (str/lower-case (:path a))
                                                                                   (str/lower-case (:path b)))
                                                                          c))))
                                                                   (take (get widget-size-limits size 3))
                                                                   (mapv (fn [entry]
                                                                           {:id (:path entry)
                                                                            :title (:name entry)
                                                                            :subtitle (:path entry)
                                                                            :url (str "/files?open="
                                                                                      (js/encodeURIComponent (:path entry)))
                                                                            :icon (widget-icon entry)})))]
                                                {:kind "list"
                                                 :items favorites
                                                 :emptyState "No favorite files"
                                                 :actions [{:id "open"
                                                            :label "Open Favorites"
                                                            :url "/files?filter=favorites"}]})))))})
  store)
