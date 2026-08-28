(ns dar.apps.files
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.path :as path]
            ["node:crypto" :as crypto]
            ["node:fs/promises" :as fs]
            ["node:path" :as node-path]))

;; Files app engine. User bytes live under <data-root>/users/<id>/files;
;; trash and versions have dedicated roots per the fresh data-root contract
;; in docs/rewrite/storage-and-lifecycle.md. Virtual paths are user-rooted
;; ("/a/b.txt") and map to a validated relative path under the files root.
;; File bytes are written through the atomic-files journaling service; file
;; metadata changes go through the one writer queue; every mutating operation
;; serializes per user.

(def mime-types
  {".txt" "text/plain" ".md" "text/markdown" ".markdown" "text/markdown"
   ".html" "text/html" ".htm" "text/html" ".css" "text/css"
   ".js" "text/javascript" ".mjs" "text/javascript" ".json" "application/json"
   ".xml" "application/xml" ".csv" "text/csv" ".yaml" "text/yaml"
   ".yml" "text/yaml" ".toml" "text/plain" ".log" "text/plain"
   ".png" "image/png" ".jpg" "image/jpeg" ".jpeg" "image/jpeg" ".gif" "image/gif"
   ".webp" "image/webp" ".svg" "image/svg+xml" ".ico" "image/x-icon"
   ".avif" "image/avif" ".pdf" "application/pdf" ".zip" "application/zip"
   ".gz" "application/gzip" ".mp3" "audio/mpeg" ".ogg" "audio/ogg"
   ".wav" "audio/wav" ".flac" "audio/flac" ".mp4" "video/mp4"
   ".webm" "video/webm" ".mov" "video/quicktime" ".vcf" "text/vcard"
   ".ics" "text/calendar" ".woff" "font/woff" ".woff2" "font/woff2"
   ".ttf" "font/ttf" ".otf" "font/otf"})

(defn mime-for-name [name]
  (or (mime-types (str/lower-case (node-path/extname name)))
      "application/octet-stream"))

(defn- vpath->relative
  "Maps a user-rooted virtual path to a validated relative path. The root
  (\"/\" and descendants of it) maps to the empty string; anything else is
  validated as a relative user path."
  [vpath]
  (let [candidate (str/replace (or vpath "/") #"^/+" "")]
    (if (str/blank? candidate)
      ""
      (path/normalize-user-path candidate))))

(defn- require-nested-vpath!
  "Rejects the files root itself for operations that need a real entry."
  [relative]
  (when (str/blank? relative)
    (throw (ex-info "The Files root cannot be used here."
                    {:type :invalid-path}))))

(defn- child-vpath [parent name]
  (if (or (= parent "/") (str/blank? parent))
    (str "/" name)
    (str parent "/" name)))

(defn- files-root [data-root user-id]
  (.resolve node-path data-root "users" user-id "files"))

(defn- trash-root [data-root user-id]
  (.resolve node-path data-root "users" user-id "trash"))

(defn- versions-root [data-root user-id]
  (.resolve node-path data-root "users" user-id "versions"))

(defn- new-id []
  (.randomUUID crypto))

(defn- now-iso []
  (.toISOString (js/Date.)))

(defn- per-user-queue
  "Serializes asynchronous operations per user so filesystem and database
  mutations of one user's files never interleave. Operations always run
  asynchronously so synchronous throws become promise rejections."
  []
  (let [tails (js/Map.)]
    (fn [user-id operation]
      (let [^js tail (or (.get tails user-id) (js/Promise.resolve nil))
            run (fn [_] (operation))
            chained (.then tail run)]
        (.set tails user-id (.catch chained (fn [_] nil)))
        chained))))

(defn- ensure-dir!
  "Creates a directory idempotently (owner rwx) and returns the path."
  [directory]
  (.then (.mkdir fs directory #js {:recursive true :mode 448}) (fn [_] directory)))

(defn- entry-stat
  "Stats a child of the files root following symlinks, or nil when missing."
  [root relative]
  (-> (.stat fs (.resolve node-path root relative))
      (.catch (fn [_] nil))))

(defn- usage-of
  "Recursively sums file bytes beneath a directory; 0 when missing."
  [directory]
  (letfn [(walk [current]
            (-> (.readdir fs current #js {:withFileTypes true})
                (.then (fn [^js entries]
                         (-> (js/Promise.all
                              (.map (js/Array.from entries)
                                    (fn [^js entry]
                                      (let [child (.resolve node-path current (.-name entry))]
                                        (if (.isDirectory entry)
                                          (walk child)
                                          (-> (.stat fs child)
                                              (.then (fn [^js stat] (.-size stat)))
                                              (.catch (fn [_] 0))))))))
                             (.then (fn [^js sizes] (reduce + 0 (js->clj sizes)))))))
                (.catch (fn [_] 0))))]
    (walk directory)))

(defn quota-usage
  "Total bytes the user stores: files, trash, and versions."
  [data-root user-id]
  (-> (js/Promise.all #js [(usage-of (files-root data-root user-id))
                           (usage-of (trash-root data-root user-id))
                           (usage-of (versions-root data-root user-id))])
      (.then (fn [^js sizes] (reduce + 0 (js->clj sizes))))))

(defn- check-quota!
  "Rejects with :quota-exceeded when the write does not fit the user quota."
  [quota-bytes data-root user-id incoming-size existing-size]
  (if (or (not quota-bytes) (<= quota-bytes 0))
    (js/Promise.resolve nil)
    (-> (quota-usage data-root user-id)
        (.then (fn [used]
                 (let [projected (- (+ used (or incoming-size 0))
                                    (or existing-size 0))]
                   (when (> projected quota-bytes)
                     (throw (ex-info "The user storage quota is exhausted."
                                     {:type :quota-exceeded
                                      :used projected
                                      :total quota-bytes})))))))))

(defn- favorites-set [^js connection user-id]
  (let [select (.prepare connection
                         "SELECT path FROM files_favorites WHERE user_id = ?")]
    (into #{} (map #(.-path %)) (.all select user-id))))

(defn- tags-for-paths [^js connection user-id]
  (let [select (.prepare connection
                         "SELECT ft.path AS path, t.name AS name FROM files_file_tags ft
                          JOIN files_tags t ON t.user_id = ft.user_id AND t.id = ft.tag_id
                          WHERE ft.user_id = ? ORDER BY t.name")
        rows (.all select user-id)]
    (reduce (fn [acc {:keys [path name]}]
              (update acc path (fnil conj []) name))
            {}
            (map (fn [^js row] {:path (.-path row) :name (.-name row)}) rows))))

(defn- make-entry [^js stat vpath favorited tags]
  {:name (node-path/basename vpath)
   :path vpath
   :size (.-size stat)
   :mimeType (if (.isDirectory stat)
               "inode/directory"
               (mime-for-name vpath))
   :isDir (.isDirectory stat)
   :updatedAt (.-mtime stat)
   :favorited favorited
   :tags (vec (or tags []))})

(defn- list-entries
  "Lists one directory beneath the user's files root."
  [^js connection data-root user-id vpath]
  (let [root (files-root data-root user-id)
        relative (vpath->relative vpath)
        favorites (favorites-set connection user-id)
        tags (tags-for-paths connection user-id)
        directory (.resolve node-path root relative)]
    (-> (ensure-dir! root)
        (.then (fn [_]
                 ;; The root itself is managed by the platform; only nested
                 ;; components need the symlink walk.
                 (if (str/blank? relative)
                   nil
                   (path/verify-no-symlinks! root relative))))
        (.then (fn [_] (.readdir fs directory #js {:withFileTypes true})))
        (.then (fn [^js entries]
                 (-> (js/Promise.all
                      (.map (js/Array.from entries)
                            (fn [^js entry]
                              (let [child-relative (if (str/blank? relative)
                                                     (.-name entry)
                                                     (str relative "/" (.-name entry)))
                                    child-virtual (child-vpath (str "/" relative)
                                                               (.-name entry))]
                                (-> (entry-stat root child-relative)
                                    (.then (fn [^js stat]
                                             (when (some? stat)
                                               (make-entry stat
                                                           child-virtual
                                                           (contains? favorites child-virtual)
                                                           (get tags child-virtual))))))))))
                     (.then (fn [^js results]
                              (->> (js->clj results)
                                   (remove nil?)
                                   (sort-by (fn [e] [(not (:isDir e))
                                                     (str/lower-case (:name e))]))
                                   (into []))))))))))

(defn- copy-entry!
  "Copies one file or directory tree recursively within managed storage."
  [source destination]
  (-> (.stat fs source)
      (.then (fn [^js stat]
               (if (.isDirectory stat)
                 (-> (ensure-dir! destination)
                     (.then (fn [_]
                              (.readdir fs source #js {:withFileTypes true})))
                     (.then (fn [^js entries]
                              (js/Promise.all
                               (.map (js/Array.from entries)
                                     (fn [^js entry]
                                       (copy-entry!
                                        (.resolve node-path source (.-name entry))
                                        (.resolve node-path destination (.-name entry)))))))))
                 (.copyFile fs source destination))))))

(defn service
  "Builds the Files engine over the data root, database, and atomic files.

  quota-bytes comes from server settings; 0 or nil means unlimited."
  [data-root {:keys [quota-bytes]} ^js database-service ^js atomic-store]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        mutate (per-user-queue)

        insert-favorite
        (.prepare connection "INSERT INTO files_favorites(user_id, path) VALUES (?, ?)")
        delete-favorite
        (.prepare connection "DELETE FROM files_favorites WHERE user_id = ? AND path = ?")
        insert-trash
        (.prepare connection "INSERT INTO files_trash(user_id, id, original_path, trash_path, deleted_at) VALUES (?, ?, ?, ?, ?)")
        select-trash
        (.prepare connection "SELECT id, original_path, trash_path, deleted_at FROM files_trash WHERE user_id = ? AND id = ?")
        delete-trash
        (.prepare connection "DELETE FROM files_trash WHERE user_id = ? AND id = ?")

        set-favorite
        (fn [user-id vpath favorited]
          ;; Favorites are stored as user-rooted virtual paths so list
          ;; entries (which carry virtual paths) match directly.
          (let [virtual (str "/" (vpath->relative vpath))]
            (.write writer
                    (fn [_]
                      (if favorited
                        (.run insert-favorite user-id virtual)
                        (.run delete-favorite user-id virtual))))))

        trash-record
        (fn [user-id trash-id]
          (let [rows (.all select-trash user-id trash-id)]
            (when-let [^js row (first rows)]
              {:id (.-id row)
               :original-path (.-original_path row)
               :trash-path (.-trash_path row)
               :deleted-at (.-deleted_at row)})))

        ;; Writes bytes atomically after the quota check; the write journals
        ;; itself through the atomic-files service.
        write-user-file!
        (fn [user-id vpath ^js bytes]
          (let [root (files-root data-root user-id)
                relative (vpath->relative vpath)
                full (.resolve node-path root relative)]
            (-> (entry-stat root relative)
                (.then (fn [^js existing]
                         (check-quota! quota-bytes data-root user-id
                                       (.-length bytes)
                                       (when (some? existing) (.-size existing)))))
                (.then (fn [_]
                         (-> (ensure-dir! (.dirname node-path full))
                             (.then (fn [_]
                                      (atomic-files/write-file!
                                       atomic-store root relative bytes)))
                             (.then (fn [_]
                                      {:path (str "/" relative)
                                       :size (.-length bytes)}))))))))

        ;; Renames the entry into the trash root and records it. The rename is
        ;; journaled (files.trash-move) so a crash between the rename and the
        ;; record write is reconciled from disk state at startup.
        move-to-trash!
        (fn [user-id vpath]
          (let [root (files-root data-root user-id)
                relative (vpath->relative vpath)
                virtual (str "/" relative)
                trash-id (new-id)
                trash-destination (.resolve node-path
                                            (trash-root data-root user-id)
                                            trash-id)]
            (-> (path/verify-no-symlinks! root relative)
                (.then (fn [_]
                         (ensure-dir! (trash-root data-root user-id))))
                (.then (fn [_]
                         ((.-journal atomic-store)
                          {:operation "files.trash-move"
                           :payload {:from (.resolve node-path root relative)
                                     :to trash-destination}})))
                (.then (fn [entry]
                         (-> (.rename fs
                                      (.resolve node-path root relative)
                                      trash-destination)
                             (.then (fn [_]
                                      (.write writer
                                              (fn [_]
                                                (.run insert-trash user-id trash-id
                                                      virtual "/.trash" (now-iso))))))
                             (.then (fn [_]
                                      ((.-markReconciled atomic-store) (:id entry))))
                             (.then (fn [_] {:id trash-id :path virtual}))))))))

        ;; Renames a trash record back to its original path and deletes it.
        restore!
        (fn [user-id trash-id]
          (let [record (trash-record user-id trash-id)]
            (if (nil? record)
              (js/Promise.reject
               (ex-info "Trash record not found." {:type :not-found :id trash-id}))
              (let [root (files-root data-root user-id)
                    relative (vpath->relative (:original-path record))
                    destination (.resolve node-path root relative)]
                (-> (ensure-dir! (.dirname node-path destination))
                    (.then (fn [_]
                             (.rename fs
                                      (.resolve node-path
                                                (trash-root data-root user-id)
                                                trash-id)
                                      destination)))
                    (.then (fn [_]
                             (.write writer
                                     (fn [_]
                                       (.run delete-trash user-id trash-id)))))
                    (.then (fn [_] {:restored (:original-path record)})))))))

        ;; Removes a trash record's bytes and its record permanently.
        purge!
        (fn [user-id trash-id]
          (let [record (trash-record user-id trash-id)]
            (if (nil? record)
              (js/Promise.reject
               (ex-info "Trash record not found." {:type :not-found :id trash-id}))
              (-> (.rm fs
                       (.resolve node-path (trash-root data-root user-id) trash-id)
                       #js {:recursive true :force true})
                  (.then (fn [_]
                           (.write writer
                                   (fn [_]
                                     (.run delete-trash user-id trash-id)))))
                  (.then (fn [_] {:purged trash-id}))))))

        ;; Renames or moves an entry within the user's files root. The
        ;; destination directory must exist; favorites keep their virtual
        ;; path keys (matching the baseline, which does not cascade them).
        rename!
        (fn [user-id from to]
          (let [root (files-root data-root user-id)
                source-relative (vpath->relative from)
                destination-relative (vpath->relative to)]
            (require-nested-vpath! source-relative)
            (require-nested-vpath! destination-relative)
            (-> (path/verify-no-symlinks! root source-relative)
                (.then (fn [_] (path/verify-no-symlinks! root destination-relative)))
                (.then (fn [_]
                         (.rename fs
                                  (.resolve node-path root source-relative)
                                  (.resolve node-path root destination-relative))))
                (.catch (fn [error]
                          (cond
                            (= "ENOENT" (.-code error))
                            (throw (ex-info "The source path does not exist."
                                            {:type :not-found}))

                            (= "EEXIST" (.-code error))
                            (throw (ex-info "The destination already exists."
                                            {:type :already-exists}))

                            :else (throw error))))
                (.then (fn [_]
                         {:renamed (str "/" destination-relative)})))))

        ;; Copies an entry recursively within the user's files root (see
        ;; copy-entry! above).
        copy!
        (fn [user-id from to]
          (let [root (files-root data-root user-id)
                source-relative (vpath->relative from)
                destination-relative (vpath->relative to)]
            (require-nested-vpath! source-relative)
            (require-nested-vpath! destination-relative)
            (-> (path/verify-no-symlinks! root source-relative)
                (.then (fn [_] (path/verify-no-symlinks! root destination-relative)))
                (.then (fn [_]
                         (copy-entry!
                          (.resolve node-path root source-relative)
                          (.resolve node-path root destination-relative))))
                (.catch (fn [error]
                          (cond
                            (= "ENOENT" (.-code error))
                            (throw (ex-info "The source path does not exist."
                                            {:type :not-found}))

                            (= "EEXIST" (.-code error))
                            (throw (ex-info "The destination already exists."
                                            {:type :already-exists}))

                            :else (throw error))))
                (.then (fn [_]
                         {:copied (str "/" destination-relative)})))))]

    #js {:filesRoot (fn [user-id] (files-root data-root user-id))
         :quotaUsage (fn [user-id] (quota-usage data-root user-id))
         :list
         (fn [user-id vpath]
           (mutate user-id (fn [] (list-entries connection data-root user-id vpath))))
         :mkdir
         (fn [user-id vpath]
           (mutate user-id
                   (fn []
                     (let [root (files-root data-root user-id)
                           relative (vpath->relative vpath)]
                       (if (str/blank? relative)
                       ;; Creating the root is a conflict by definition.
                       (js/Promise.reject
                        (ex-info "The folder already exists."
                                 {:type :already-exists}))
                       (-> (ensure-dir! root)
                           (.then (fn [_]
                                    (path/verify-no-symlinks! root relative)))
                           (.then (fn [_]
                                    (.mkdir fs
                                            (.resolve node-path root relative)
                                            #js {:mode 448})))
                           (.then (fn [_] (str "/" relative)))
                           (.catch (fn [error]
                                     (if (= "EEXIST" (.-code error))
                                       (throw (ex-info "The folder already exists."
                                                       {:type :already-exists}))
                                       (throw error))))))))))
         :touch
         (fn [user-id vpath]
           (mutate user-id
                   (fn []
                     (let [root (files-root data-root user-id)
                           relative (vpath->relative vpath)]
                       (require-nested-vpath! relative)
                       (-> (entry-stat root relative)
                           (.then (fn [^js existing]
                                    (when (some? existing)
                                      (throw (ex-info "The file already exists."
                                                      {:type :already-exists})))
                                    (write-user-file! user-id vpath (js/Buffer.alloc 0)))))))))
         :save
         (fn [user-id vpath ^js bytes]
           (mutate user-id
                   (fn []
                     (require-nested-vpath! (vpath->relative vpath))
                     (write-user-file! user-id vpath bytes))))
         :favorite
         (fn [user-id vpath favorited]
           (mutate user-id (fn [] (set-favorite user-id vpath favorited))))
         :delete
         (fn [user-id vpath]
           (mutate user-id
                   (fn []
                     (require-nested-vpath! (vpath->relative vpath))
                     (move-to-trash! user-id vpath))))
         :restore
         (fn [user-id trash-id]
           (mutate user-id (fn [] (restore! user-id trash-id))))
         :purge
         (fn [user-id trash-id]
           (mutate user-id (fn [] (purge! user-id trash-id))))
         :rename
         (fn [user-id from to]
           (mutate user-id (fn [] (rename! user-id from to))))
         :copy
         (fn [user-id from to]
           (mutate user-id (fn [] (copy! user-id from to))))}))
