(ns dar.apps.files
  (:require [clojure.string :as str]
            [dar.cordis :as cordis]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.path :as path]
            ["node:buffer" :refer [Buffer]]
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
  ;; Entries carry tag IDs (baseline loadFileTags), not tag names.
  (let [select (.prepare connection
                         "SELECT path, tag_id AS tag_id FROM files_file_tags
                          WHERE user_id = ? ORDER BY path, tag_id")
        rows (.all select user-id)]
    (reduce (fn [acc {:keys [path tagId]}]
              (update acc path (fnil conj []) tagId))
            {}
            (map (fn [^js row] {:path (.-path row) :tagId (.-tag_id row)}) rows))))

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

(def editor-limit
  "Maximum editable file size; larger files have no text editor."
  (* 1024 1024))

(defn- sha256-hex [^js bytes]
  (.digest (.update (crypto/createHash "sha256") bytes) "hex"))

(defn- hex-encode [text]
  (.toString (Buffer.from text "utf8") "hex"))

(defn- valid-name?
  "The editor may rename a file to a bare name only."
  [name]
  (and (string? name)
       (not (str/blank? name))
       (= name (node-path/basename name))
       (not (contains? #{".", ".."} name))
       (not (or (str/includes? name "/")
                (str/includes? name "\\")
                (str/includes? name "\u0000")))))

(defn- read-bytes!
  "Reads one validated file's bytes, rejecting directories and missing paths."
  [root relative]
  (let [full (.resolve node-path root relative)]
    (-> (path/verify-no-symlinks! root relative)
        (.then (fn [_] (.stat fs full)))
        (.then (fn [^js stat]
                 (when (.isDirectory stat)
                   (throw (ex-info "The entry is a folder."
                                   {:type :invalid})))
                 (.readFile fs full)))
        (.catch (fn [error]
                  (if (= "ENOENT" (.-code error))
                    (throw (ex-info "The file does not exist."
                                    {:type :not-found}))
                    (throw error)))))))

(defn- document-for [relative ^js bytes]
  (let [name (node-path/basename relative)]
    {:path (str "/" relative)
     :name name
     :content (.toString bytes "utf8")
     :etag (sha256-hex bytes)
     :markdown (contains? #{".md" ".markdown"} (str/lower-case (node-path/extname name)))
     :mimeType (mime-for-name name)
     :size (.-length bytes)}))

(defn- normalize-if-match [value]
  (let [trimmed (str/trim (or value ""))]
    (cond
      (or (str/blank? trimmed) (str/includes? trimmed ",")
          (str/starts-with? trimmed "W/"))
      nil
      (and (str/starts-with? trimmed "\"") (str/ends-with? trimmed "\"")
           (>= (count trimmed) 2))
      (subs trimmed 1 (dec (count trimmed)))
      :else trimmed)))

(defn service
  "Builds the Files engine over the data root, database, atomic files, shared
  rate limiter, and credential keyring. quota-bytes comes from server settings;
  0 or nil means unlimited. The limiter guards share unlock and drop uploads;
  the keyring derives share password digests and access cookies."
  [data-root {:keys [quota-bytes]} ^js database-service ^js atomic-store
   ^js rate-limiter ^js keyring]
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
        insert-version
        (.prepare connection "INSERT INTO files_versions(user_id, path, id, blob_path, size, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        select-versions
        (.prepare connection "SELECT id, size, created_at FROM files_versions WHERE user_id = ? AND path = ? ORDER BY created_at DESC, id DESC")
        select-version-blob
        (.prepare connection "SELECT blob_path FROM files_versions WHERE user_id = ? AND path = ? AND id = ?")
        select-tags
        (.prepare connection "SELECT id, name, color FROM files_tags WHERE user_id = ? ORDER BY name")
        insert-tag
        (.prepare connection "INSERT INTO files_tags(user_id, id, name, color) VALUES (?, ?, ?, ?)")
        delete-tag
        (.prepare connection "DELETE FROM files_tags WHERE user_id = ? AND id = ?")
        delete-tag-assignments
        (.prepare connection "DELETE FROM files_file_tags WHERE user_id = ? AND tag_id = ?")
        insert-file-tag
        (.prepare connection "INSERT OR IGNORE INTO files_file_tags(user_id, path, tag_id) VALUES (?, ?, ?)")
        delete-file-tag
        (.prepare connection "DELETE FROM files_file_tags WHERE user_id = ? AND path = ? AND tag_id = ?")
        select-comments
        (.prepare connection "SELECT id, author, body, parent_id, created_at FROM files_comments WHERE user_id = ? AND path = ? ORDER BY created_at, id")
        insert-comment
        (.prepare connection "INSERT INTO files_comments(user_id, path, id, author, body, parent_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        delete-comment
        (.prepare connection "DELETE FROM files_comments WHERE user_id = ? AND path = ? AND id = ?")
        select-lock-owner
        (.prepare connection "SELECT owner FROM files_locks WHERE user_id = ? AND path = ?")
        upsert-lock
        (.prepare connection "INSERT INTO files_locks(user_id, path, owner) VALUES (?, ?, ?) ON CONFLICT(user_id, path) DO UPDATE SET owner = excluded.owner")
        delete-lock
        (.prepare connection "DELETE FROM files_locks WHERE user_id = ? AND path = ?")
        insert-share
        (.prepare connection "INSERT INTO files_public_shares(token, user_id, path, password_salt, password_hash, expires_at, drop_only, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        select-shares
        (.prepare connection "SELECT token, path, password_salt, password_hash, expires_at, drop_only, created_at FROM files_public_shares WHERE user_id = ? ORDER BY created_at DESC")
        select-share
        (.prepare connection "SELECT token, user_id, path, password_salt, password_hash, expires_at, drop_only, created_at FROM files_public_shares WHERE token = ?")
        delete-share
        (.prepare connection "DELETE FROM files_public_shares WHERE user_id = ? AND token = ?")
        insert-user-share
        (.prepare connection "INSERT INTO files_user_shares(id, from_user, to_user, path, permission, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        select-user-share
        (.prepare connection "SELECT id, from_user, to_user, path, permission, created_at FROM files_user_shares WHERE id = ? AND (from_user = ? OR to_user = ?)")
        delete-user-share
        (.prepare connection "DELETE FROM files_user_shares WHERE id = ? AND (from_user = ? OR to_user = ?)")
        select-incoming-shares
        (.prepare connection "SELECT id, from_user, to_user, path, permission, created_at FROM files_user_shares WHERE to_user = ? ORDER BY created_at DESC, id")
        select-recipient
        (.prepare connection "SELECT id FROM users WHERE id = ? OR lower(email) = lower(?) LIMIT 1")

        ;; Returns the owner of an explicit lock on a path, or nil. Mutating
        ;; operations check both source and destination before touching bytes.
        locked-owner
        (fn [user-id relative]
          (let [row (.get select-lock-owner user-id relative)]
            (when (some? row) (.-owner row))))

        ensure-unlocked!
        (fn [user-id relatives]
          (doseq [relative (distinct relatives)]
            (when-some [owner (locked-owner user-id relative)]
              (throw (ex-info "The file is busy."
                              {:type :locked
                               :path (str "/" relative)
                               :owner owner})))))

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
                (.then (fn [_] (ensure-unlocked! user-id [source-relative
                                                           destination-relative])))
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
                (.then (fn [_] (ensure-unlocked! user-id [source-relative
                                                           destination-relative])))
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
                         {:copied (str "/" destination-relative)})))))


        ;; Returns the owner of an explicit lock on a path, or nil. Mutating
        ;; operations check both source and destination before touching bytes.
        locked-owner
        (fn [user-id relative]
          (let [row (.get select-lock-owner user-id relative)]
            (when (some? row) (.-owner row))))

        ensure-unlocked!
        (fn [user-id relatives]
          (doseq [relative (distinct relatives)]
            (when-some [owner (locked-owner user-id relative)]
              (throw (ex-info "The file is busy."
                              {:type :locked
                               :path (str "/" relative)
                               :owner owner})))))

        ;; Snapshots the current bytes of a file into the versions root as an
        ;; immutable blob (versions/<path-hex>/<id>) plus a metadata row.
        create-version!
        (fn [user-id vpath]
          (let [root (files-root data-root user-id)
                relative (vpath->relative vpath)]
            (-> (read-bytes! root relative)
                (.then (fn [^js bytes]
                         (let [id (new-id)
                               versions-dir (.resolve node-path
                                                      (versions-root data-root user-id)
                                                      (hex-encode (str "/" relative)))
                               blob (.resolve node-path versions-dir id)
                               write-blob!
                               (fn []
                                 (atomic-files/write-file!
                                  atomic-store versions-dir id bytes))
                               record!
                               (fn []
                                 (.write writer
                                         (fn [_]
                                           (.run insert-version
                                                 user-id (str "/" relative)
                                                 id blob (.-length bytes)
                                                 (now-iso)))))]
                           (-> (ensure-dir! versions-dir)
                               (.then (fn [_] (write-blob!)))
                               (.then (fn [_] (record!)))
                               (.then (fn [_]
                                        {:id id :size (.-length bytes)})))))))))


        list-versions-fn
        (fn [user-id vpath]
          (map (fn [^js row]
                 {:id (.-id row)
                  :size (.-size row)
                  :createdAt (.-created_at row)})
               ;; Version rows store the user-rooted path with its leading
               ;; slash (see create-version!); match that form here.
               (.all select-versions user-id (str "/" (vpath->relative vpath)))))

        read-version-fn
        (fn [user-id vpath version-id]
          (let [stored-path (str "/" (vpath->relative vpath))
                row (.get select-version-blob user-id stored-path version-id)]
            (if (nil? row)
              (js/Promise.reject
               (ex-info "Version not found." {:type :not-found}))
              (.readFile fs (.-blob_path row)))))

        ;; Restores a version by snapshotting the current content first and
        ;; then writing the old bytes back through the atomic path. The
        ;; restore write is awaited so callers only see the restored content
        ;; after it has landed.
        restore-version!
        (fn [user-id vpath version-id]
          (let [root (files-root data-root user-id)
                relative (vpath->relative vpath)]
            (-> (read-version-fn user-id vpath version-id)
                (.then (fn [^js bytes]
                         (ensure-unlocked! user-id [relative])
                         (-> (create-version! user-id vpath)
                             (.then (fn [_]
                                      (-> (atomic-files/write-file!
                                           atomic-store root relative bytes)
                                          (.then (fn [_]
                                                   {:restored (str "/" relative)})))))))))))

        ;; Editor save. Validates If-Match, size, and encoding; checks locks;
        ;; versions the previous content; supports rename with rollback on a
        ;; failed write; and returns the fresh document.
        save-document!
        ;; Everything (validation included) runs inside the per-user queue so
        ;; validation errors surface as promise rejections, never as
        ;; synchronous throws into the HTTP layer. The previous content is
        ;; versioned before any change; a rename whose write fails is rolled
        ;; back so the original file stays intact.
        (fn [user-id vpath {:keys [content name if-match]}]
          (mutate user-id
                  (fn []
                    (let [root (files-root data-root user-id)
                          source-relative (vpath->relative vpath)
                          match (normalize-if-match if-match)]
                      (when (str/blank? (or match ""))
                        (throw (ex-info "An If-Match header is required."
                                        {:type :precondition-required})))
                      (let [bytes (Buffer.from (or content "") "utf8")]
                        (when (> (.-length bytes) editor-limit)
                          (throw (ex-info "The file is too large to edit."
                                          {:type :too-large :limit editor-limit})))
                        (when (str/includes? (or content "") "\u0000")
                          (throw (ex-info "The file is not UTF-8 text."
                                          {:type :unsupported-media-type})))
                        (when-not (valid-name? (or name (node-path/basename source-relative)))
                          (throw (ex-info "The filename is invalid."
                                          {:type :invalid})))
                        (let [destination-relative
                              (if (and name (not= name (node-path/basename source-relative)))
                                (let [parent (node-path/dirname source-relative)]
                                  (if (= parent ".") name (str parent "/" name)))
                                source-relative)]
                          (-> (read-bytes! root source-relative)
                              (.then
                               (fn [current-bytes]
                                 (let [current (document-for source-relative
                                                             current-bytes)]
                                   (ensure-unlocked!
                                    user-id [source-relative destination-relative])
                                   (when-not (= (:etag current) match)
                                     (throw (ex-info "The file changed."
                                                     {:type :precondition-failed
                                                      :etag (:etag current)})))
                                   ;; The destination must not exist for a
                                   ;; renaming save; the stat is awaited here
                                   ;; (entry-stat resolves a promise).)))
                                   (-> (if (= destination-relative source-relative)
                                         (js/Promise.resolve nil)
                                         (entry-stat root destination-relative))
                                       (.then
                                        (fn [existing]
                                          (when (some? existing)
                                            (throw (ex-info
                                                    "A file with that name already exists."
                                                    {:type :already-exists})))
                                          {:current current
                                           :renamed? (not= destination-relative
                                                          source-relative)
                                           :changed? (not= (:content current)
                                                           (.toString bytes "utf8"))}))))))
                              (.then
                               (fn [ctx]
                                 ;; Version the previous content before any
                                 ;; change, then hand the context onward.
                                 (-> (if (:changed? ctx)
                                       (create-version! user-id vpath)
                                       (js/Promise.resolve nil))
                                     (.then (fn [_] ctx)))))
                              (.then
                               (fn [{:keys [renamed? changed?]}]
                                 (if (or renamed? changed?)
                                   (if-not renamed?
                                     (write-user-file! user-id vpath bytes)
                                     (-> (.rename fs
                                                  (.resolve node-path root source-relative)
                                                  (.resolve node-path root destination-relative))
                                         (.then
                                          (fn [_]
                                            (write-user-file!
                                             user-id
                                             (str "/" destination-relative)
                                             bytes)))
                                         (.catch
                                          (fn [error]
                                            ;; Roll the rename back so the
                                            ;; original file stays intact.
                                            (-> (.rename fs
                                                         (.resolve node-path root destination-relative)
                                                         (.resolve node-path root source-relative))
                                                (.catch (fn [_] nil))
                                                (.then (fn [_]
                                                         (throw error))))))))
                                   (js/Promise.resolve nil))))
                              (.then
                               (fn [_]
                                 (document-for destination-relative
                                               bytes))))))))))
        ;; Lists the user tags. Entries carry tag names joined per path.
        list-tags-fn
        (fn [user-id]
          (map (fn [^js row]
                 {:id (.-id row) :name (.-name row) :color (.-color row)})
               (.all select-tags user-id)))
        create-tag!
        (fn [user-id {:keys [name color]}]
          (when (or (str/blank? (or name "")) (not (string? color)))
            (throw (ex-info "The tag name or color is invalid."
                            {:type :invalid})))
          (let [id (new-id)]
            ;; Await the writer so callers only see the tag after commit.
            (.then (.write writer
                           (fn [_]
                             (.run insert-tag user-id id name color)))
                   (fn [_]
                     {:id id :name name :color color}))))

        delete-tag!
        (fn [user-id tag-id]
          (.then (.write writer
                         (fn [_]
                           (.run delete-tag-assignments user-id tag-id)
                           (.run delete-tag user-id tag-id)))
                 (fn [_]
                   {:deleted tag-id})))

        set-file-tag!
        ;; File tags are stored under the user-rooted virtual path (with its
        ;; leading slash) so listings can join entries directly (baseline
        ;; handleTag stores the cleaned virtual path).
        (fn [user-id vpath tag-id add?]
          (let [virtual (str "/" (vpath->relative vpath))]
            (.then (.write writer
                           (fn [_]
                             (if add?
                               (.run insert-file-tag user-id virtual tag-id)
                               (.run delete-file-tag user-id virtual tag-id))))
                   (fn [_]
                     {:path virtual :tag tag-id :added add?}))))

        list-comments-fn
        (fn [user-id vpath]
          (map (fn [^js row]
                 {:id (.-id row)
                  :author (.-author row)
                  :body (.-body row)
                  :parentId (.-parent_id row)
                  :createdAt (.-created_at row)})
               (.all select-comments user-id (vpath->relative vpath))))

        add-comment!
        (fn [user-id vpath {:keys [body parent-id]}]
          (when (str/blank? (or body ""))
            (throw (ex-info "The comment body is required."
                            {:type :invalid})))
          (let [id (new-id)
                comment {:id id
                         :author user-id
                         :body body
                         :parentId (or parent-id "")
                         :createdAt (now-iso)}]
            (.then (.write writer
                           (fn [_]
                             (.run insert-comment user-id (vpath->relative vpath) id
                                   user-id body (or parent-id "") (:createdAt comment))))
                   (fn [_]
                     comment))))

        delete-comment!
        (fn [user-id vpath comment-id]
          (.then (.write writer
                         (fn [_]
                           (.run delete-comment user-id (vpath->relative vpath) comment-id)))
                 (fn [_]
                   {:deleted comment-id})))

        lock-info
        (fn [user-id vpath]
          (let [owner (locked-owner user-id (vpath->relative vpath))]
            (js/Promise.resolve {:locked (some? owner) :owner owner})))

        lock!
        (fn [user-id vpath]
          (let [relative (vpath->relative vpath)]
            (.then (.write writer
                           (fn [_]
                             (.run upsert-lock user-id relative user-id)))
                   (fn [_]
                     {:locked true :owner user-id}))))

        unlock!
        (fn [user-id vpath]
          (let [relative (vpath->relative vpath)]
            (.then (.write writer
                           (fn [_]
                             (.run delete-lock user-id relative)))
                   (fn [_]
                     {:locked false}))))

        ;; Shares. Password-protected shares derive digests from the
        ;; credential keyring (never stored plaintext); crypto runs outside
        ;; the writer queue so the queue only holds short SQL statements.
        share-password-digest
        (fn [^js salt password]
          (try
            (.mac keyring "dar-share-password-v1"
                         (.concat Buffer #js [salt (.from Buffer password "utf8")]))
            (catch :default _
              (throw (ex-info "Password-protected shares are unavailable."
                              {:type :share-password-unavailable})))))

        share-access-value
        (fn [token]
          (try
            (.toString (.mac keyring "dar-share-access-v1" token) "hex")
            (catch :default _
              (throw (ex-info "Password-protected shares are unavailable."
                              {:type :share-password-unavailable})))))

        share-expired?
        (fn [expires-at now-seconds]
          (and (some? expires-at) (not= "" expires-at)
               (<= (/ (js/Date.parse expires-at) 1000) now-seconds)))

        share-row->map
        (fn [^js row]
          {:token (.-token row)
           :path (.-path row)
           :hasPassword (> (.-length (.-password_hash row)) 0)
           :expiresAt (let [value (.-expires_at row)] (when-not (= value "") value))
           :drop (= 1 (.-drop_only row))
           :createdAt (.-created_at row)})

        ;; Internal row with owner identity for the public share routes.
        share-entity->map
        (fn [^js row]
          {:token (.-token row)
           :userId (.-user_id row)
           :path (.-path row)
           :passwordSalt (.-password_salt row)
           :passwordHash (.-password_hash row)
           :expiresAt (let [value (.-expires_at row)] (when-not (= value "") value))
           :drop (= 1 (.-drop_only row))
           :createdAt (.-created_at row)})

        user-share-row->map
        (fn [^js row]
          {:id (.-id row)
           :path (.-path row)
           :fromUser (.-from_user row)
           :toUser (.-to_user row)
           :permission (.-permission row)
           :createdAt (.-created_at row)})

        ensure-shareable!
        (fn [user-id vpath]
          (let [relative (vpath->relative vpath)]
            (if (str/blank? relative)
              (js/Promise.reject
               (ex-info "The files root cannot be shared." {:type :invalid}))
              (-> (entry-stat (files-root data-root user-id) relative)
                  (.then (fn [stat]
                           (when (nil? stat)
                             (throw (ex-info "Shared path not found."
                                             {:type :not-found})))
                           (str "/" relative)))))))

        create-share!
        (fn [user-id vpath {:keys [password expires-at drop]}]
          (let [now-seconds (quot (.now js/Date) 1000)]
            (if (share-expired? expires-at now-seconds)
              (js/Promise.reject
               (ex-info "Share expiry must be in the future." {:type :invalid}))
              (-> (ensure-shareable! user-id vpath)
                  (.then (fn [virtual]
                           (let [salt (if password (crypto/randomBytes 16) (.alloc Buffer 0))
                                 digest (if password
                                          (share-password-digest salt password)
                                          (.alloc Buffer 0))
                                 token (.toString (crypto/randomBytes 32) "hex")
                                 created-at (.toISOString (js/Date.))]
                             (.then (.write writer
                                            (fn [_]
                                              (.run insert-share token user-id
                                                    virtual salt digest
                                                    (or expires-at "") (if drop 1 0)
                                                    created-at)))
                                    (fn [_]
                                      {:token token
                                       :path virtual
                                       :hasPassword (boolean password)
                                       :expiresAt expires-at
                                       :drop (boolean drop)
                                       :createdAt created-at})))))))))

        list-shares-fn
        (fn [user-id]
          (js/Promise.resolve
           (mapv share-row->map (.all select-shares user-id))))

        delete-share!
        (fn [user-id token]
          (.then (.write writer
                         (fn [_]
                           (let [^js result (.run delete-share user-id token)]
                             (when (= 0 (.-changes result))
                               (throw (ex-info "Share not found."
                                               {:type :not-found})))
                             nil)))
                 (fn [_]
                   {:deleted token})))

        read-share-fn
        (fn [token]
          (let [row (.get select-share token)]
            (js/Promise.resolve (when (some? row) (share-entity->map row)))))

        ;; Unlock checks are public-facing: rate limited per token and peer,
        ;; constant-time compared, and answered with a cookie value scoped to
        ;; the share namespace that the HTTP layer stores. The limiter key is
        ;; namespaced per operation because the shared service windows key on
        ;; the key string alone.
        unlock-share!
        (fn [{:keys [token passwordSalt passwordHash]} password peer]
          (let [^js verdict (.allow rate-limiter
                                    (str "unlock\u0000" token "\u0000" peer)
                                    5 60000)
                cookie-value (share-access-value token)]
            (cond
              (not (.-allowed verdict))
              (js/Promise.reject
               (ex-info "Too many unlock attempts."
                        {:type :rate-limited
                         :retry-after-ms (.-retryAfterMs verdict)}))

              (= 0 (.-length passwordHash))
              (js/Promise.resolve {:unlocked true :cookieValue cookie-value})

              :else
              (let [supplied (try
                               (share-password-digest passwordSalt password)
                               (catch :default _ nil))]
                (js/Promise.resolve
                 (if (and (some? supplied)
                          (try (crypto/timingSafeEqual supplied passwordHash)
                               (catch :default _ false)))
                   {:unlocked true :cookieValue cookie-value}
                   (js/Promise.reject
                    (ex-info "Invalid share password."
                             {:type :invalid-credentials}))))))))

        ;; A share is unlocked when it has no password or when the presented
        ;; cookie value matches the token's access value (timing-safe).
        share-unlocked?
        (fn [{:keys [token passwordHash]} cookie-value]
          (if (= 0 (.-length passwordHash))
            true
            (let [expected (try (share-access-value token) (catch :default _ nil))]
              (and (string? cookie-value) (some? expected)
                   (try (crypto/timingSafeEqual
                         (.from Buffer cookie-value "utf8")
                         (.from Buffer expected "utf8"))
                        (catch :default _ false))))))

        ;; Drop uploads overwrite existing files but version the replaced
        ;; content first (baseline saveUploadedStream); quota and atomic
        ;; Drop uploads overwrite existing files but version the replaced
        ;; content first (baseline saveUploadedStream); quota and atomic
        ;; journaling are owned by the existing write path.
        share-upload!
        (fn [{:keys [token userId path drop] :as share}
             cookie-value filename bytes peer]
          (let [name (str/trim (str filename))
                relative (vpath->relative path)
                target (str "/" relative "/" name)]
            (cond
              (not drop)
              (js/Promise.reject
               (ex-info "This share does not accept uploads." {:type :forbidden}))

              (not (share-unlocked? share cookie-value))
              (js/Promise.reject
               (ex-info "Share upload denied." {:type :forbidden}))

              (or (= name "") (not= name (node-path/basename name))
                  (str/includes? name "/") (str/includes? name "\\")
                  (str/includes? name "\u0000"))
              (js/Promise.reject
               (ex-info "Invalid filename." {:type :invalid}))

              :else
              (let [^js verdict (.allow rate-limiter
                                        (str "upload\u0000" token "\u0000" peer)
                                        20 60000)]
                (if (not (.-allowed verdict))
                  (js/Promise.reject
                   (ex-info "Too many uploads."
                            {:type :rate-limited
                             :retry-after-ms (.-retryAfterMs verdict)}))
                  (let [target-relative (str relative "/" name)]
                    (-> (entry-stat (files-root data-root userId)
                                    target-relative)
                        (.then (fn [stat]
                                 ;; The upload target may not exist yet; only
                                 ;; an existing file has a prior version to
                                 ;; snapshot before the overwrite.
                                 (if (some? stat)
                                   (create-version! userId target)
                                   (js/Promise.resolve nil))))
                      (.then (fn [_]
                               (write-user-file! userId target bytes)))
                      (.then (fn [_]
                               {:saved target}))))))))) 

        create-user-share!
        ;; User shares grant read or write access to a single recipient.
        ;; The recipient resolves by id or email (baseline handleUserShare).
        (fn [user-id {:keys [path to-user permission]}]
          (let [perm (or permission "read")]
            (cond
              (not (#{"read" "write"} perm))
              (js/Promise.reject
               (ex-info "Permission must be read or write." {:type :invalid}))

              (str/blank? (str to-user))
              (js/Promise.reject
               (ex-info "A recipient is required." {:type :invalid}))

              :else
              (-> (ensure-shareable! user-id path)
                  (.then (fn [virtual]
                           (let [^js row (.get select-recipient to-user to-user)]
                             (cond
                               (nil? row)
                               (js/Promise.reject
                                (ex-info "Recipient not found." {:type :not-found}))

                               (= (.-id row) user-id)
                               (js/Promise.reject
                                (ex-info "Cannot share with yourself."
                                         {:type :invalid}))

                               :else
                               (let [id (.randomUUID crypto)
                                     created-at (.toISOString (js/Date.))]
                                 (.then
                                  (.write writer
                                          (fn [_]
                                            (.run insert-user-share id user-id
                                                  (.-id row) virtual perm
                                                  created-at)))
                                  (fn [_]
                                    {:id id
                                     :path virtual
                                     :fromUser user-id
                                     :toUser (.-id row)
                                     :permission perm
                                     :createdAt created-at})))))))))))

        delete-user-share!
        ;; Either party may remove a user share; the id alone never deletes.
        (fn [user-id id]
          (.then (.write writer
                         (fn [_]
                           (let [^js row (.get select-user-share id user-id user-id)]
                             (when (nil? row)
                               (throw (ex-info "Share not found."
                                               {:type :not-found})))
                             (.run delete-user-share id user-id user-id)
                             nil)))
                 (fn [_]
                   {:deleted id})))

        shared-with-me-fn
        (fn [user-id]
          (js/Promise.resolve
           (mapv user-share-row->map (.all select-incoming-shares user-id))))
        ]

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
                     (let [relative (vpath->relative vpath)]
                       (require-nested-vpath! relative)
                       (ensure-unlocked! user-id [relative])
                       (move-to-trash! user-id vpath)))))
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
           (mutate user-id (fn [] (copy! user-id from to))))

         ;; Stage 2 surface: editor, versions, tags, comments, and locks.
         :readDocument
         (fn [user-id vpath]
           (mutate user-id
                   (fn []
                     (-> (read-bytes! (files-root data-root user-id)
                                      (vpath->relative vpath))
                         (.then (fn [^js bytes]
                                  (document-for (vpath->relative vpath) bytes)))))))
         :saveDocument
         ;; save-document! enqueues its own mutate; wrapping it again here
         ;; would deadlock the per-user queue.
         (fn [user-id vpath doc]
           (save-document! user-id vpath doc))
         :createVersion
         (fn [user-id vpath]
           (mutate user-id (fn [] (create-version! user-id vpath))))
         :listVersions
         (fn [user-id vpath]
           (mutate user-id (fn [] (js/Promise.resolve (vec (list-versions-fn user-id vpath))))))
         :readVersion
         (fn [user-id vpath version-id]
           (mutate user-id (fn [] (read-version-fn user-id vpath version-id))))
         :restoreVersion
         (fn [user-id vpath version-id]
           (mutate user-id (fn [] (restore-version! user-id vpath version-id))))
         :listTags
         (fn [user-id]
           (mutate user-id (fn [] (js/Promise.resolve (vec (list-tags-fn user-id))))))
         :createTag
         (fn [user-id tag]
           (mutate user-id (fn [] (create-tag! user-id tag))))
         :deleteTag
         (fn [user-id tag-id]
           (mutate user-id (fn [] (delete-tag! user-id tag-id))))
         :setFileTag
         (fn [user-id vpath tag-id add?]
           (mutate user-id (fn [] (set-file-tag! user-id vpath tag-id add?))))
         :listComments
         (fn [user-id vpath]
           (mutate user-id (fn [] (js/Promise.resolve (vec (list-comments-fn user-id vpath))))))
         :addComment
         (fn [user-id vpath body parent-id]
           (mutate user-id (fn [] (add-comment! user-id vpath {:body body :parent-id parent-id}))))
         :deleteComment
         (fn [user-id vpath comment-id]
           (mutate user-id (fn [] (delete-comment! user-id vpath comment-id))))
         :lockInfo
         (fn [user-id vpath]
           (mutate user-id (fn [] (lock-info user-id vpath))))
         :lock
         (fn [user-id vpath]
           (mutate user-id (fn [] (lock! user-id vpath))))
         :unlock
         (fn [user-id vpath]
           (mutate user-id (fn [] (unlock! user-id vpath))))
         ;; Shares: creation and deletes manage their own queue use; the
         ;; unlock/upload checks are public-surface and never enqueue.
         :createShare
         (fn [user-id vpath spec]
           (create-share! user-id vpath spec))
         :listShares
         (fn [user-id]
           (list-shares-fn user-id))
         :deleteShare
         (fn [user-id token]
           (delete-share! user-id token))
         :readShare
         (fn [token]
           (read-share-fn token))
         :unlockShare
         (fn [share password peer]
           (unlock-share! share password peer))
         :isShareUnlocked
         (fn [share cookie-value]
           (share-unlocked? share cookie-value))
         :uploadToShare
         (fn [share cookie-value filename bytes peer]
           (share-upload! share cookie-value filename bytes peer))
         :createUserShare
         (fn [user-id spec]
           (create-user-share! user-id spec))
         :deleteUserShare
         (fn [user-id id]
           (delete-user-share! user-id id))
         :sharedWithMe
         (fn [user-id]
           (shared-with-me-fn user-id))}))
