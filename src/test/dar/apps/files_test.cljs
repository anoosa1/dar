(ns dar.apps.files-test
  (:require [cljs.test :refer [async deftest is]]
            [clojure.string :as str]
            [dar.apps.files :as files]
            [dar.apps.files-assistant :as files-assistant]
            [dar.apps.notes :as notes]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.database :as database]
            [dar.platform.keyring :as keyring]
            [dar.platform.rate-limiter :as rate-limiter]
            ["node:buffer" :refer [Buffer]]
            ["node:crypto" :as crypto]
            ["node:fs" :as fs]
            ["node:fs/promises" :as fsp]
            ["node:os" :as os]
            ["node:path" :as path]
            ["sharp" :as sharp]))

(defn- open-service!
  ([] (open-service! {}))
  ([{:keys [quota-bytes search]}]
   (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-files-app-test-"))
         connection (database/open! (.resolve path directory "dar.sqlite"))
         database-service #js {:connection connection
                               :writer (database/writer-queue connection)}
         atomic-store (atomic-files/service database-service)
         data-root directory]
     {:directory directory
      :connection connection
      :data-root data-root
      :database database-service
      :atomic-store atomic-store
      :store (files/service data-root {:quota-bytes quota-bytes}
                            database-service atomic-store
                            (rate-limiter/service (fn [] (.now js/Date)))
                            (keyring/keyring
                             [{:id "test-key"
                               :material (.randomBytes crypto 32)}])
                             search)})))

(defn- fake-search
  "In-memory unified-search double recording index mutations per provider."
  []
  (let [store-docs (atom {})]
    #js {:index (fn [user provider id title _body url]
                  (swap! store-docs assoc-in [user provider id]
                         {:title title :url url})
                  (js/Promise.resolve nil))
         :delete (fn [user provider id]
                   (swap! store-docs update-in [user provider] dissoc id)
                   (js/Promise.resolve nil))
         :reconcileProvider
         (fn [user provider entries]
           (let [docs (if (array? entries)
                        (js->clj entries :keywordize-keys true)
                        (vec entries))]
           ;; Mirrors the real service: one generation replaces the whole
           ;; provider set, so stale documents disappear.
             (swap! store-docs assoc-in [:calls provider]
                    (str "reconcile got " (pr-str docs)))
             (swap! store-docs assoc-in [user provider] {})
             (doseq [entry docs]
               (swap! store-docs assoc-in [user provider (:id entry)]
                      {:title (:title entry) :url (:url entry)}))
             (js/Promise.resolve
              #js {:provider provider :generation 1
                   :count (count docs)})))
         :docs store-docs}))

(defn- seed-user!
  ([fixture] (seed-user! fixture "user-1"))
  ([fixture user-id]
   (.run (.prepare ^js (:connection fixture)
                   "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
         user-id (str user-id "-subject") "Files"
         (str user-id "@example.test")
         (.toISOString (js/Date.)) (.toISOString (js/Date.)))))

(defn- close-service! [fixture]
  (.close (:connection fixture))
  (.rmSync fs (:directory fixture) #js {:recursive true :force true}))

(defn- finish
  "Ends an async test exactly once, closing the stores first on either path."
  [start fixture done]
  (-> start
      (.then (fn [_]
               (close-service! fixture)
               (done)))
      (.catch (fn [error]
                (is false (str "flow failed: " (.-message error)))
                (close-service! fixture)
                (done)))))

(defn- run-steps
  "Reduces promise-returning thunks left to right; a rejecting thunk fails the
  whole run. Each thunk owns its own assertions."
  [thunks]
  (reduce (fn [prior thunk] (.then prior thunk))
          (js/Promise.resolve nil)
          thunks))

(defn- settle
  "Maps a promise to a resolved {:outcome ...} step that never rejects."
  [promise outcome]
  (-> promise
      (.then (fn [value] {:outcome outcome :value value}))
      (.catch (fn [error] {:outcome outcome :error error}))))

(defn- read-file [fixture relative]
  (.readFileSync fs (.resolve path (:directory fixture) "users" "user-1" "files" relative) "utf8"))

(deftest note-snapshots-follow-renames-and-reject-symlinks
  (async done
    (let [fixture (open-service!) ^js store (:store fixture) original (atom nil)]
      (seed-user! fixture)
      (finish
        (run-steps
          [(fn [_] (.mkdir store "user-1" "/Notes"))
           (fn [_] (.save store "user-1" "/Notes/First.md" (Buffer.from "# First\nA thought")))
           (fn [_] (.readDocumentSnapshot store "user-1" "/Notes/First.md"))
           (fn [snapshot]
             (reset! original snapshot)
             (is (= "# First\nA thought" (:content snapshot)))
             (is (string? (:inode snapshot)))
             (.rename store "user-1" "/Notes/First.md" "/Notes/Renamed.md"))
           (fn [_] (.readDocumentSnapshot store "user-1" "/Notes/Renamed.md"))
           (fn [snapshot]
             (is (= (select-keys @original [:device :inode :etag]) (select-keys snapshot [:device :inode :etag])))
             (.symlink fsp "Renamed.md" (.resolve path (:directory fixture) "users/user-1/files/Notes/Link.md")))
           (fn [_] (settle (.readDocumentSnapshot store "user-1" "/Notes/Link.md") :rejected))
           (fn [result] (is (= :unsafe-path (:type (ex-data (:error result))))))])
        fixture done))))

(deftest note-snapshot-rejects-an-external-replacement-after-reading-and-closes-its-handle
  (async done
    (let [fixture (open-service!) ^js store (:store fixture)
          full (.resolve path (:directory fixture) "users/user-1/files/Notes/Racing.md")
          ^js original-open (.-open fsp) closed (atom false)]
      (seed-user! fixture)
      (finish
        (-> (run-steps
              [(fn [_] (.mkdir store "user-1" "/Notes"))
               (fn [_] (.save store "user-1" "/Notes/Racing.md" (Buffer.from "# Before\nOriginal")))
               (fn [_]
                 ;; Replace precisely after the handle's read, before the
                 ;; path's final identity check. No timing race or sleep.
                 (set! (.-open fsp)
                       (fn [& args]
                         (-> (.apply original-open fsp (into-array args))
                             (.then (fn [^js handle]
                                      (when (= full (first args))
                                        (let [^js read (.-read handle) ^js close (.-close handle)]
                                          (set! (.-read handle)
                                                (fn [& read-args]
                                                  (-> (.apply read handle (into-array read-args))
                                                      (.then (fn [value]
                                                               (.writeFileSync fs (str full ".swap") "# After\nReplacement")
                                                               (.renameSync fs (str full ".swap") full) value)))))
                                          (set! (.-close handle)
                                                (fn [] (-> (.call close handle) (.then (fn [_] (reset! closed true))))))))
                                      handle)))))
                 (settle (.readDocumentSnapshot store "user-1" "/Notes/Racing.md") :rejected))
               (fn [result]
                 (is (= :conflict (:type (ex-data (:error result)))))
                 (is @closed "An inconsistent snapshot still closes its file handle")
                 (set! (.-open fsp) original-open)
                 (.readDocumentSnapshot store "user-1" "/Notes/Racing.md"))
               (fn [snapshot] (is (= "# After\nReplacement" (:content snapshot))))])
            (.finally #(set! (.-open fsp) original-open)))
        fixture done))))

(deftest notes-use-files-revisions-versions-and-rename-reconciliation
  (async done
    (let [fixture (open-service!) ^js files (:store fixture)
          ^js notes (notes/service (:database fixture) (:atomic-store fixture) files (constantly false))
          location {:folder "/Notes" :suffix ".md"} saved (atom nil)]
      (seed-user! fixture)
      (finish
        (run-steps
          [(fn [_] (.create notes "user-1" location {:content "# Reading\nA thought"}))
           (fn [note]
             (reset! saved note) (is (= "Reading" (:title note)))
             (.update notes "user-1" location (:id note) {:content "# Reading\nA second thought" :category "Books" :favorite true} (:revision note)))
           (fn [note]
             (is (= (:id @saved) (:id note)))
             (is (= "Books" (:category note))) (is (:favorite note))
             (reset! saved note)
             (.versions notes "user-1" location (:id note)))
           (fn [versions]
             (is (seq versions))
             (settle (.update notes "user-1" location (:id @saved) {:content "A stale draft"} 1) :rejected))
           (fn [result]
             (is (= :precondition-failed (:type (ex-data (:error result)))))
             (.rename files "user-1" (:path @saved) "/Notes/Books/Renamed.md"))
           (fn [_] (.read notes "user-1" location (:id @saved)))
           (fn [note]
             (is (= "/Notes/Books/Renamed.md" (:path note)))
             (is (= "# Reading\nA second thought" (:content note)))
             (.trash notes "user-1" location (:id note) (:revision note)))
           (fn [note]
             (is (:deletedAt note))
             (.restore notes "user-1" location (:id note) (:revision note)))
           (fn [note]
             (is (= (:id @saved) (:id note)))
             (is (nil? (:deletedAt note)))
             (is (= "# Reading\nA second thought" (:content note))))])
        fixture done))))

(deftest rejects-unsafe-virtual-paths
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (settle (.list store "user-1" "/../etc") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :unsafe-path (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.list store "user-1" "/.trash/x") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :unsafe-path (:type (ex-data (:error step))))))])
       fixture done))))

(deftest mkdir-touch-list-and-save-round-trip
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.mkdir store "user-1" "/Documents"))
         (fn [_] (.touch store "user-1" "/Documents/notes.md"))
         (fn [_]
           (.save store "user-1" "/Documents/notes.md"
                  (Buffer.from "hello dar" "utf8")))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (= 1 (count entries)))
           (let [documents (first entries)]
             (is (= "Documents" (:name documents)))
             (is (= "/Documents" (:path documents)))
             (is (true? (:isDir documents)))
             (is (= "inode/directory" (:mimeType documents)))))
         (fn [_] (.list store "user-1" "/Documents"))
         (fn [entries]
           (is (= 1 (count entries)))
           (let [notes (first entries)]
             (is (= "notes.md" (:name notes)))
             (is (= "/Documents/notes.md" (:path notes)))
             (is (false? (:isDir notes)))
             (is (= 9 (:size notes)))
             (is (= "text/markdown" (:mimeType notes)))))
         (fn [_]
           (is (= "hello dar" (read-file fixture (.join path "Documents" "notes.md")))))])
       fixture done))))

(deftest mkdir-conflicts-on-existing-folder-and-touch-on-existing-file
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.mkdir store "user-1" "/Documents"))
         (fn [_]
           (settle (.mkdir store "user-1" "/Documents") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :already-exists (:type (ex-data (:error step))))))
         (fn [_] (.touch store "user-1" "/Documents/notes.md"))
         (fn [_]
           (settle (.touch store "user-1" "/Documents/notes.md") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :already-exists (:type (ex-data (:error step))))))])
       fixture done))))

(deftest mkdir-rejects-missing-parent-as-not-found
  ;; DAV MKCOL maps this to 409 Conflict (RFC 4918 9.3.1); the JSON API
  ;; answers 404. Either way it must be a classified error, never a raw
  ;; ENOENT leaking out of the engine.
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (settle (.mkdir store "user-1" "/Missing/Child") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))])
       fixture done))))

(deftest editor-read-rejects-oversized-and-non-utf8-files
  ;; The editor GET enforces the same limits as the editor PUT: files over
  ;; editor-limit answer 413 and non-UTF-8 bytes answer 415, on read too.
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/big.txt"
                  (js/Buffer.alloc (inc 1048576) 97)))
         (fn [_]
           (settle (.readDocument store "user-1" "/big.txt") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :too-large (:type (ex-data (:error step))))))
         (fn [_]
           (.save store "user-1" "/binary.bin"
                  (js/Buffer.from #js [255 254 0])))
         (fn [_]
           (settle (.readDocument store "user-1" "/binary.bin") :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :unsupported-media-type
                  (:type (ex-data (:error step))))))])
       fixture done))))

(deftest delete-moves-to-trash-and-restore-returns-it
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          trash-id (atom nil)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/notes.txt" (Buffer.from "keep me" "utf8")))
         (fn [_] (.delete store "user-1" "/notes.txt"))
         (fn [deleted]
           (reset! trash-id (:id deleted))
           (is (string? (:id deleted))))
          (fn [_] (.listTrash store "user-1"))
          (fn [trash]
            (is (= 1 (count trash)))
            (is (= @trash-id (:id (first trash))))
            (is (= "/notes.txt" (:originalPath (first trash))))
            (is (= "notes.txt" (:name (first trash)))))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (= 0 (count entries))))
         (fn [_] (.restore store "user-1" @trash-id))
         (fn [restored]
           (is (= "/notes.txt" (:restored restored))))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (= 1 (count entries)))
           (is (= "/notes.txt" (:path (first entries)))))
         (fn [_]
           (is (= "keep me" (read-file fixture "notes.txt"))))])
       fixture done))))

(deftest favorites-toggle-and-appear-in-list-entries
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/report.pdf" (Buffer.from "pdf" "utf8")))
         (fn [_] (.favorite store "user-1" "/report.pdf" true))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (true? (:favorited (first entries))))
           (is (= "application/pdf" (:mimeType (first entries)))))
         (fn [_] (.favorite store "user-1" "/report.pdf" false))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (false? (:favorited (first entries)))))
         (fn [_]
           (.then (.mkdir store "user-1" "/deep")
                  (fn [_]
                    (.then (.mkdir store "user-1" "/deep/nested")
                           (fn [_]
                             (.save store "user-1" "/deep/nested/notes.md"
                                    (Buffer.from "nested" "utf8")))))))
         (fn [_] (.favorite store "user-1" "/deep/nested/notes.md" true))
         (fn [_] (.favorite store "user-1" "/report.pdf" true))
         (fn [_] (.listFavorites store "user-1"))
         (fn [favorites]
           (is (= ["/deep/nested/notes.md" "/report.pdf"]
                  (mapv :path favorites)))
           (is (every? :favorited favorites)))])
       fixture done))))

(deftest rename-move-copy-and-purge-work
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/notes.txt" (Buffer.from "copy me" "utf8")))
         (fn [_] (.mkdir store "user-1" "/Documents"))
         (fn [_] (.rename store "user-1" "/notes.txt" "/Documents/renamed.txt"))
         (fn [_] (.list store "user-1" "/"))
         (fn [entries]
           (is (= 1 (count entries)))
           (is (= "/Documents" (:path (first entries)))))
         (fn [_]
           (settle (.rename store "user-1" "/missing.txt" "/elsewhere.txt") :rejected))
         (fn [step]
           (is (= :not-found (:type (ex-data (:error step))))))
         (fn [_] (.copy store "user-1" "/Documents" "/Backup"))
         (fn [copied]
           (is (= "/Backup" (:copied copied))))
         (fn [_] (.list store "user-1" "/Backup"))
         (fn [entries]
           (is (= 1 (count entries)))
           (is (= "/Backup/renamed.txt" (:path (first entries)))))
         (fn [_] (.delete store "user-1" "/Backup/renamed.txt"))
         (fn [deleted]
           (.purge store "user-1" (:id deleted)))
         (fn [_] (.list store "user-1" "/Backup"))
         (fn [entries]
           (is (= 0 (count entries))))
         (fn [_]
           (settle (.purge store "user-1" "no-such-id") :rejected))
         (fn [step]
           (is (= :not-found (:type (ex-data (:error step))))))])
       fixture done))))

(deftest search-index-follows-file-writes
  (async done
    (let [search (fake-search)
          fixture (open-service! {:search search})
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/notes.md" (Buffer.from "body" "utf8")))
         (fn [_]
           (let [doc (get-in @(.-docs search) ["user-1" "files" "/notes.md"])]
             (is (some? doc))
             (is (= "notes.md" (:title doc)))
             (is (= "/files?open=%2Fnotes.md" (:url doc)))))
         ;; Deleting the file drops it from the provider index.
         (fn [_] (.delete store "user-1" "/notes.md"))
         (fn [_]
           (is (nil? (get-in @(.-docs search) ["user-1" "files" "/notes.md"]))))
         ;; Renaming a directory reconciles the whole tree so nested
         ;; documents move to their new paths.
         (fn [_]
           (.then (.mkdir store "user-1" "/deep")
                  (fn [_]
                    (.then (.mkdir store "user-1" "/deep/nested")
                           (fn [_]
                             (.save store "user-1" "/deep/nested/notes.md"
                                    (Buffer.from "nested" "utf8")))))))
         (fn [_] (.rename store "user-1" "/deep" "/deeper"))
         (fn [_]
           (let [docs (get @(.-docs search) "user-1")]
             (is (nil? (get-in docs ["files" "/deep/nested/notes.md"])))
             (is (some? (get-in docs ["files" "/deeper/nested/notes.md"]))
                 (str "indexed docs: " (pr-str docs)))))])
       fixture done))))


(deftest raw-overwrite-snapshots-previous-content
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/doc.txt" (Buffer.from "first" "utf8")))
         (fn [_] (.save store "user-1" "/doc.txt" (Buffer.from "second" "utf8")))
         (fn [_] (.listVersions store "user-1" "/doc.txt"))
         (fn [versions]
           (is (= 1 (count versions)))
           (.readVersion store "user-1" "/doc.txt" (:id (first versions))))
         (fn [bytes]
           (is (= "first" (.toString bytes "utf8"))))
         ;; Saving byte-identical content does not add another snapshot.
         (fn [_] (.save store "user-1" "/doc.txt" (Buffer.from "second" "utf8")))
         (fn [_] (.listVersions store "user-1" "/doc.txt"))
         (fn [versions]
           (is (= 1 (count versions))))])
       fixture done))))


(deftest rename-carries-favorites-tags-and-locks
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          token (atom nil)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.then (.mkdir store "user-1" "/deep")
                  (fn [_]
                    (.then (.mkdir store "user-1" "/deep/nested")
                           (fn [_]
                             (.save store "user-1" "/deep/nested/notes.md"
                                    (Buffer.from "nested" "utf8")))))))
         (fn [_] (.favorite store "user-1" "/deep/nested/notes.md" true))
         (fn [_] (.createTag store "user-1" {:name "work" :color "#2563eb"}))
         (fn [tag]
           (.setFileTag store "user-1" "/deep/nested/notes.md" (:id tag) true))
         (fn [_] (.lock store "user-1" "/deep/nested/notes.md"))
         (fn [locked]
           (is (true? (:locked locked)))
           (is (string? (:token locked)))
           (reset! token (:token locked)))
         (fn [_] (.rename store "user-1" "/deep" "/deeper"))
         (fn [_] (.listFavorites store "user-1"))
         (fn [favorites]
           (is (= ["/deeper/nested/notes.md"] (mapv :path favorites))))
         (fn [_]
           (.all (.prepare ^js (:connection fixture)
                           "SELECT path, tag_id FROM files_file_tags WHERE user_id = ?")
                 "user-1"))
         (fn [tag-rows]
           (is (some (fn [row] (= (.-path row) "/deeper/nested/notes.md")) tag-rows)
               (str "tag rows: " (pr-str (js->clj tag-rows :keywordize-keys true)))))
         (fn [_] (.lockInfo store "user-1" "/deeper/nested/notes.md"))
         (fn [info]
           (is (= @token (:token info))))
         (fn [_]
           (settle (.unlock store "user-1" "/deeper/nested/notes.md"
                            @token)
                   :unlocked))
         (fn [step]
           (is (= :unlocked (:outcome step))))])
       fixture done))))


(deftest quota-exceeded-rejects-writes-beyond-the-limit
  (async done
    ;; Quota 20 leaves room for the automatic version snapshot the raw
    ;; overwrite takes before replacing /small.txt.
    (let [fixture (open-service! {:quota-bytes 20})
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/small.txt" (Buffer.from "12345678" "utf8")))
         (fn [_]
           (settle (.save store "user-1" "/big.txt"
                          (Buffer.from "0123456789ABCDEF" "utf8"))
                   :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :quota-exceeded (:type (ex-data (:error step))))))
         (fn [_]
           ;; Replacing the same file counts against the quota using the
           ;; freed space of the current content, so this must succeed.
           (.save store "user-1" "/small.txt" (Buffer.from "87654321" "utf8")))
         (fn [_]
           (is (= "87654321" (read-file fixture "small.txt"))))])
       fixture done))))


(deftest editor-save-validates-and-versions-documents
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/notes.md" (Buffer.from "first draft" "utf8")))
         (fn [_]
           (settle (.saveDocument store "user-1" "/notes.md"
                                  {:content "second draft"})
                   :missing-match))
         (fn [step]
           (is (= :missing-match (:outcome step)))
           (is (= :precondition-required (:type (ex-data (:error step))))))
         (fn [_]
           (.readDocument store "user-1" "/notes.md"))
         (fn [doc]
           (is (= "notes.md" (:name doc)))
           (is (= true (:markdown doc)))
           (is (= "text/markdown" (:mimeType doc)))
           (is (= 11 (:size doc)))
           (is (string? (:etag doc))))
         (fn [_]
           (.readDocument store "user-1" "/notes.md"))
         (fn [doc]
           (.saveDocument store "user-1" "/notes.md"
                          {:content "second draft" :if-match (:etag doc)}))
         (fn [saved]
           (is (= "second draft" (:content saved)))
           (is (= 12 (:size saved)))
           (.listVersions store "user-1" "/notes.md"))
         (fn [versions]
           (is (= 1 (count versions)))
           (.readVersion store "user-1" "/notes.md" (:id (first versions))))
         (fn [bytes]
           (is (= "first draft" (.toString bytes "utf8"))))])
       fixture done))))

(deftest versions-refuse-symlinked-blobs
  (async done
    (let [fixture (open-service!) ^js store (:store fixture) ^js db (:connection fixture)]
      (seed-user! fixture)
      (finish
        (run-steps
          [(fn [_] (.save store "user-1" "/version.txt" (Buffer.from "Original")))
           (fn [_] (.save store "user-1" "/version.txt" (Buffer.from "Changed")))
           (fn [_]
             (let [^js row (.get (.prepare db "SELECT id,blob_path FROM files_versions WHERE user_id='user-1' AND path='/version.txt'"))
                   blob (.-blob_path row)]
               (.unlinkSync fs blob)
               (.symlinkSync fs (.resolve path (:directory fixture) "dar.sqlite") blob)
               (settle (.readVersion store "user-1" "/version.txt" (.-id row)) :unsafe)))
           (fn [result] (is (= :unsafe (:outcome result)))
             (is (= :unsafe-path (:type (ex-data (:error result))))))])
        fixture done))))

(deftest editor-save-rename-creates-snapshot-and-restore-rolls-back
  (async done
    ;; Retained v1 (7), retained v2 (27), and restored v1 (7) need 41 bytes.
    (let [fixture (open-service! {:quota-bytes 41})
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/memo.md" (Buffer.from "memo v1" "utf8")))
         (fn [_]
           (.readDocument store "user-1" "/memo.md"))
         (fn [doc]
           (.saveDocument store "user-1" "/memo.md"
                          {:content "memo v2 with longer content"
                           :if-match (:etag doc)}))
         (fn [_]
           (.readDocument store "user-1" "/memo.md"))
         (fn [doc]
           ;; Reject the oversized save before adding a version or renaming.
           (settle (.saveDocument store "user-1" "/memo.md"
                                  {:content "memo v3 that is much longer and exceeds the quota"
                                   :if-match (:etag doc)
                                   :name "renamed.md"})
                   :quota-rejected))
         (fn [step]
           (is (= :quota-rejected (:outcome step)))
           (is (= :quota-exceeded (:type (ex-data (:error step))))))
         (fn [_]
           (is (= "memo v2 with longer content" (read-file fixture "memo.md")))
           (settle (.readDocument store "user-1" "/renamed.md") :gone))
         (fn [step]
           (is (= :gone (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))
         (fn [_]
           (.listVersions store "user-1" "/memo.md"))
         (fn [versions]
           ;; Rejected writes leave both the content and version quota intact.
           (is (= 1 (count versions)))
           (.readVersion store "user-1" "/memo.md" (:id (last versions))))
         (fn [bytes]
           (is (= "memo v1" (.toString bytes "utf8")))
           (.listVersions store "user-1" "/memo.md"))
         (fn [versions]
           (.restoreVersion store "user-1" "/memo.md" (:id (last versions))))
         (fn [_]
           (is (= "memo v1" (read-file fixture "memo.md")))
           (.listVersions store "user-1" "/memo.md"))
         (fn [versions]
           ;; Restore snapshots the pre-restore content first.
           (is (= 2 (count versions))))])
       fixture done))))

(deftest explicit-locks-block-mutations-until-unlocked
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          lock-token (atom nil)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/locked.md" (Buffer.from "shared doc" "utf8")))
         (fn [_]
           (.lock store "user-1" "/locked.md"))
         (fn [result]
           (is (= true (:locked result)))
            (is (string? (:token result)))
            (reset! lock-token (:token result))
           (is (= "user-1" (:owner result))))
          (fn [_]
            (settle (.then (js/Promise.resolve nil)
                           (fn [_] (.lock store "user-1" "/locked.md")))
                    :second-blocked))
          (fn [step]
            (is (= :locked (:type (ex-data (:error step))))))
         (fn [_]
           (.lockInfo store "user-1" "/locked.md"))
         (fn [info]
           (is (= true (:locked info)))
           (is (= "user-1" (:owner info))))
         (fn [_]
           (.readDocument store "user-1" "/locked.md"))
         (fn [doc]
           (settle (.saveDocument store "user-1" "/locked.md"
                                  {:content "blocked" :if-match (:etag doc)})
                   :save-blocked))
         (fn [step]
           (is (= :save-blocked (:outcome step)))
           (is (= :locked (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.rename store "user-1" "/locked.md" "/moved.md") :rename-blocked))
         (fn [step]
           (is (= :rename-blocked (:outcome step)))
           (is (= :locked (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.copy store "user-1" "/locked.md" "/copy.md") :copy-blocked))
         (fn [step]
           (is (= :copy-blocked (:outcome step)))
           (is (= :locked (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.delete store "user-1" "/locked.md") :delete-blocked))
         (fn [step]
           (is (= :delete-blocked (:outcome step)))
           (is (= :locked (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.then (js/Promise.resolve nil)
                          (fn [_] (.unlock store "user-1" "/locked.md"
                                           "wrong-token")))
                   :wrong-token))
         (fn [step]
           (is (= :locked (:type (ex-data (:error step)))))
           (.unlock store "user-1" "/locked.md" @lock-token))
         (fn [_]
           (.lockInfo store "user-1" "/locked.md"))
         (fn [info]
           (is (= false (:locked info)))
           (.readDocument store "user-1" "/locked.md"))
         (fn [doc]
           (.saveDocument store "user-1" "/locked.md"
                          {:content "writable again" :if-match (:etag doc)}))
         (fn [saved]
           (is (= "writable again" (:content saved))))])
       fixture done))))

(deftest tags-and-comments-attach-to-paths
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/doc.md" (Buffer.from "tagged doc" "utf8")))
         (fn [_]
           (settle (.createTag store "user-1" {:name "" :color "#ff0000"}) :blank))
         (fn [step]
           (is (= :blank (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (.createTag store "user-1" {:name "important" :color "#ff0000"}))
         (fn [tag]
           (is (string? (:id tag)))
           (is (= "important" (:name tag)))
           (.setFileTag store "user-1" "/doc.md" (:id tag) true))
         (fn [applied]
           (is (= "/doc.md" (:path applied)))
           (is (= true (:added applied)))
           (.listTags store "user-1"))
         (fn [tags]
           (is (= 1 (count tags)))
           (is (= "important" (:name (first tags))))
           (.deleteTag store "user-1" (:id (first tags))))
         (fn [deleted]
           (is (string? (:deleted deleted)))
           (.listTags store "user-1"))
         (fn [tags]
           ;; Deleting a tag cascades its file assignments.
           (is (= 0 (count tags)))
           (.addComment store "user-1" "/doc.md" "first!" nil))
         (fn [comment]
           (is (= "first!" (:body comment)))
           (is (= "user-1" (:author comment)))
           (settle (.addComment store "user-1" "/doc.md" "" nil) :blank-comment))
         (fn [step]
           (is (= :blank-comment (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (.listComments store "user-1" "/doc.md"))
         (fn [comments]
           (is (= 1 (count comments)))
           (.deleteComment store "user-1" "/doc.md" (:id (first comments))))
         (fn [_]
           (.listComments store "user-1" "/doc.md"))
         (fn [comments]
           (is (= 0 (count comments))))])
       fixture done))))

(deftest tagged-listing-follows-tags-across-depth-and-trash
  ;; The tag filter (baseline /api/files/list?filter=tag) lists every
  ;; tagged path at any depth with the tag id attached; trashed paths
  ;; drop out; favorites stay reported independently of tagging.
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.mkdir store "user-1" "/docs"))
         (fn [_]
           (.save store "user-1" "/docs/nested.md" (Buffer.from "deep" "utf8")))
         (fn [_]
           (.save store "user-1" "/top.md" (Buffer.from "top" "utf8")))
         (fn [_]
           (.createTag store "user-1" {:name "projects" :color "#00ff00"}))
         (fn [tag]
           (.setFileTag store "user-1" "/docs/nested.md" (:id tag) true))
         (fn [_]
           (.createTag store "user-1" {:name "top" :color "#0000ff"}))
         (fn [top-tag]
           (.setFileTag store "user-1" "/top.md" (:id top-tag) true))
         (fn [_]
           (.favorite store "user-1" "/docs/nested.md" true))
         (fn [_]
           (.listTags store "user-1"))
         (fn [tags]
           (.listTagged store "user-1" (:id (first tags))))
         (fn [tagged]
           ;; "projects" sorts first and matches only the nested file.
           (is (= 1 (count tagged)))
           (is (= "/docs/nested.md" (:path (first tagged))))
           (is (= "nested.md" (:name (first tagged))))
           (is (= true (:favorited (first tagged))))
           (.delete store "user-1" "/docs/nested.md"))
         (fn [_]
           (.listTags store "user-1"))
         (fn [tags]
           (.listTagged store "user-1" (:id (first tags))))
         (fn [after-trash]
           (is (= 0 (count after-trash))))])
       fixture done))))

(deftest share-visitors-can-descend-into-shared-folders
  ;; Folder descent: a share of /Public exposes sub-paths under /Public
  ;; (listing and download), but never escapes it (.., absolute, backslash,
  ;; or NUL segments are rejected before any filesystem access).
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          state (atom {})]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.then (.mkdir store "user-1" "/Public")
                  (fn [_]
                    (.then (.mkdir store "user-1" "/Public/Inner")
                           (fn [_]
                             (.then (.save store "user-1" "/Public/Inner/doc.md"
                                           (Buffer.from "inner doc" "utf8"))
                                    (fn [_]
                                      (.save store "user-1" "/secret.md"
                                             (Buffer.from "secret" "utf8")))))))))
         (fn [_]
           (.createShare store "user-1" "/Public" {}))
         (fn [share]
           ;; Share rows are re-read so the entity carries :userId, like the
           ;; HTTP visitor flow (token -> readShare).
           (.readShare store (:token share)))
         (fn [entity]
           (swap! state assoc :share entity)
           (.shareEntries store entity "Inner"))
         (fn [entries]
           (is (= ["doc.md"] (mapv :name entries)))
           (is (= [false] (mapv :isDir entries))))
         (fn [_]
           (.shareDownload store (:share @state) "Inner/doc.md"))
         (fn [bytes]
           (is (= "inner doc" (.toString bytes "utf8")))
           (settle (.shareDownload store (:share @state) "../secret.md")
                   :rejected))
         (fn [step]
           (is (= :rejected (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))])
       fixture done))))

(deftest sharing-covers-unlocks-drop-uploads-and-user-shares
  (async done
    ;; Share rows are re-read between steps because run-steps only threads
    ;; the previous step's result; state carries the values forward.
    (let [fixture (open-service!)
          ^js store (:store fixture)
          state (atom {})]
      (seed-user! fixture)
      (seed-user! fixture "user-2")
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/shared.md" (Buffer.from "shared bytes" "utf8")))
         (fn [_]
           (.mkdir store "user-1" "/Drops"))
         (fn [_]
           (settle (.createShare store "user-1" "/" {}) :root-share))
         (fn [step]
           (is (= :root-share (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.createShare store "user-1" "/shared.md"
                                 {:expires-at "2020-01-01T00:00:00.000Z"})
                   :past-expiry))
         (fn [step]
           (is (= :past-expiry (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (.createShare store "user-1" "/Drops"
                         {:password "s3cret" :drop true}))
         (fn [share]
           (reset! state {:share share})
           (is (re-matches #"[0-9a-f]{64}" (:token share)))
           (is (= "/Drops" (:path share)))
           (is (true? (:hasPassword share)))
           (is (true? (:drop share)))
           (.readShare store (:token share)))
         (fn [entity]
           (swap! state assoc :entity entity)
           (is (= "user-1" (:userId entity)))
           (is (> (.-length (:passwordHash entity)) 0))
           (settle (.unlockShare store entity "wrong" "203.0.113.9") :bad))
         (fn [step]
           (is (= :bad (:outcome step)))
           (is (= :invalid-credentials (:type (ex-data (:error step))))))
         (fn [_]
           (.unlockShare store (:entity @state) "s3cret" "203.0.113.9"))
         (fn [unlocked]
           (swap! state assoc :cookie-value (:cookieValue unlocked))
           (is (true? (:unlocked unlocked)))
           (is (string? (:cookieValue unlocked)))
           ;; Three more wrong attempts exhaust the five-per-minute window.
           (settle (.unlockShare store (:entity @state) "nope" "203.0.113.9")
                   :bad2))
         (fn [step]
           (is (= :bad2 (:outcome step)))
           (settle (.unlockShare store (:entity @state) "nope" "203.0.113.9")
                   :bad3))
         (fn [step]
           (is (= :bad3 (:outcome step)))
           (settle (.unlockShare store (:entity @state) "nope" "203.0.113.9")
                   :bad4))
         (fn [step]
           (is (= :bad4 (:outcome step)))
           (settle (.unlockShare store (:entity @state) "s3cret" "203.0.113.9")
                   :limited))
         (fn [step]
           (is (= :limited (:outcome step)))
           (is (= :rate-limited (:type (ex-data (:error step))))))
         ;; Drop uploads use their own window and the access cookie value.
         (fn [_]
           (.uploadToShare store (:entity @state) (:cookie-value @state)
                           "dropped.txt" (Buffer.from "dropped" "utf8")
                           "203.0.113.9"))
         (fn [uploaded]
           (is (= "/Drops/dropped.txt" (:saved uploaded)))
           (js/Promise.resolve (read-file fixture "Drops/dropped.txt")))
         (fn [contents]
           (is (= "dropped" contents))
           (.uploadToShare store (:entity @state) (:cookie-value @state)
                           "dropped.txt" (Buffer.from "dropped 2" "utf8")
                           "203.0.113.9"))
         (fn [_]
           ;; Overwriting drop uploads versions the replaced content.
           (.listVersions store "user-1" "/Drops/dropped.txt"))
         (fn [versions]
           (is (= 1 (count versions)))
           (settle (.uploadToShare store (:entity @state) "forged-cookie"
                                   "x.txt" (Buffer.from "x" "utf8")
                                   "198.51.100.7") :forged))
         (fn [step]
           (is (= :forged (:outcome step)))
           (is (= :forbidden (:type (ex-data (:error step))))))
         (fn [_]
           (.createShare store "user-1" "/shared.md" {}))
         (fn [open-share]
           (is (false? (:hasPassword open-share)))
           (settle (.uploadToShare store open-share nil
                                   "y.txt" (Buffer.from "y" "utf8")
                                   "203.0.113.9") :not-drop))
         (fn [step]
           (is (= :not-drop (:outcome step)))
           (is (= :forbidden (:type (ex-data (:error step))))))
         (fn [_]
           (.createUserShare store "user-1"
                             {:path "/shared.md" :to-user "user-2"
                              :permission "write"}))
         (fn [user-share]
           (is (= "user-2" (:toUser user-share)))
           (is (= "write" (:permission user-share))))
         (fn [_]
           (.listUserShares store "user-1"))
         (fn [outgoing]
           (is (= 1 (count outgoing)))
           (is (= "user-2" (:toUser (first outgoing))))
           (settle (.createUserShare store "user-1"
                                     {:path "/shared.md" :to-user "user-2"
                                      :permission "admin"}) :bad-perm))
         (fn [step]
           (is (= :bad-perm (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.createUserShare store "user-1"
                                     {:path "/shared.md" :to-user "user-1"})
                   :self-share))
         (fn [step]
           (is (= :self-share (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.createUserShare store "user-1"
                                     {:path "/shared.md" :to-user "ghost"})
                   :no-recipient))
         (fn [step]
           (is (= :no-recipient (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))
         (fn [_]
           (.sharedWithMe store "user-2"))
         (fn [incoming]
           (is (= 1 (count incoming)))
           (.deleteUserShare store "user-2" (:id (first incoming))))
         (fn [deleted]
           (is (string? (:deleted deleted)))
           (settle (.deleteUserShare store "user-2" (:deleted deleted))
                   :already-deleted))
         (fn [step]
           (is (= :already-deleted (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))
         (fn [_]
           (.deleteShare store "user-1" (:token (:share @state))))
         (fn [removed]
           (is (string? (:deleted removed)))
           (.readShare store (:token (:share @state))))
         (fn [gone]
           (is (nil? gone)))])
       fixture done))))

(deftest queued-shared-write-revalidates-a-revoked-grant
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          ^js engine (.-typescriptEngine store)
          host (aget engine "host")
          read-bounded (aget host "readBounded")
          started! (atom nil) release! (atom nil)
          started (js/Promise. (fn [resolve] (reset! started! resolve)))
          held (js/Promise. (fn [resolve] (reset! release! resolve)))
          grant (atom nil) queued (atom nil) active (atom nil)]
      (seed-user! fixture "user-1")
      (seed-user! fixture "user-2")
      (finish
       (-> (run-steps
            [(fn [_] (.save store "user-1" "/Team/note.txt" (Buffer.from "original")))
             (fn [_] (.createUserShare store "user-1" {:path "/Team" :to-user "user-2" :permission "write"}))
             (fn [share]
               (reset! grant (:id share))
               ;; Hold an accepted owner read so the recipient write is queued,
               ;; then revoke from the recipient's independent queue.
               (aset host "readBounded"
                     (fn [root relative maximum]
                       (@started!)
                       (.then held (fn [_] (read-bounded root relative maximum)))))
               (reset! active (.readFile store "user-1" "/Team/note.txt"))
               started)
             (fn [_]
               (reset! queued (settle (.save (.location engine "user-2" "shared" @grant)
                                             "/note.txt" (Buffer.from "unauthorized")) :write))
               (.deleteUserShare store "user-2" @grant))
             (fn [_]
               (aset host "readBounded" read-bounded)
               (@release!)
               @active)
             (fn [_] @queued)
             (fn [result]
               (is (= "not-found" (.-type (:error result))))
               (is (= "original" (read-file fixture "Team/note.txt"))))])
           (.finally (fn [] (aset host "readBounded" read-bounded) (@release!))))
       fixture done))))

(deftest incoming-shares-and-maildir-are-scoped-readable-locations
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          share-id (atom nil)
          mail-directory (.resolve path (:data-root fixture) "users" "user-2" "mail" "account" "INBOX" "cur")]
      (seed-user! fixture "user-1")
      (seed-user! fixture "user-2")
      (finish
       (run-steps
        [(fn [_] (.mkdir store "user-1" "/Project"))
         (fn [_] (.save store "user-1" "/Project/readme.txt"
                        (Buffer.from "shared bytes" "utf8")))
         (fn [_] (.createUserShare store "user-1"
                                   {:path "/Project" :to-user "user-2"
                                    :permission "read"}))
         (fn [share] (reset! share-id (:id share)))
         (fn [_] (.listShared store "user-2" @share-id "/"))
         (fn [entries]
           (is (= ["readme.txt"] (mapv :name entries)))
           (.readShared store "user-2" @share-id "/readme.txt"))
         (fn [bytes]
           (is (= "shared bytes" (.toString bytes "utf8")))
           (settle (.then (js/Promise.resolve nil)
                          (fn [_] (.listShared store "user-1" @share-id "/")))
                   :owner-denied))
         (fn [step]
           (is (= :not-found (:type (ex-data (:error step)))))
           (settle (.then (js/Promise.resolve nil)
                          (fn [_] (.saveShared store "user-2" @share-id "/note.txt"
                                               (Buffer.from "no" "utf8"))))
                   :read-only))
         (fn [step]
           (is (= :forbidden (:type (ex-data (:error step)))))
           (.createUserShare store "user-1"
                             {:path "/Project" :to-user "user-2"
                              :permission "write"}))
         (fn [write-share]
           (.saveShared store "user-2" (:id write-share) "/reply.txt"
                        (Buffer.from "reply bytes" "utf8")))
         (fn [_]
           (.readFile store "user-1" "/Project/reply.txt"))
         (fn [bytes]
           (is (= "reply bytes" (.toString bytes "utf8")))
           (-> (.mkdir fsp mail-directory #js {:recursive true})
               (.then (fn [_]
                        (.writeFile fsp (.resolve path mail-directory "message.eml")
                                    "Subject: Test\n\nBody")))))
         (fn [_] (.listMail store "user-2" "/account/INBOX/cur"))
         (fn [entries]
           (is (= ["message.eml"] (mapv :name entries)))
           (.readMail store "user-2" "/account/INBOX/cur/message.eml"))
         (fn [bytes]
           (is (= "Subject: Test\n\nBody" (.toString bytes "utf8"))))])
       fixture done))))

(deftest previews-serve-text-and-thumbnails-serve-images
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/notes.md" (Buffer.from "preview me" "utf8")))
         (fn [_]
           ;; A 400x300 PNG: wide enough to prove the thumbnail downscales.
           (.toBuffer (.png (sharp #js {:create #js {:width 400 :height 300
                                                     :channels 3
                                                     :background #js {:r 20 :g 40 :b 60}}}))))
         (fn [^js png]
           (.save store "user-1" "/pic.png" png))
         (fn [_]
           (.save store "user-1" "/blob.bin" (.from Buffer #js [0 1 2 3])))
         (fn [_]
           (.preview store "user-1" "/notes.md"))
         (fn [preview]
           (is (= "text/markdown" (:type preview)))
           (is (= "preview me" (:content preview)))
           (is (= "notes.md" (:name preview)))
           (settle (.preview store "user-1" "/pic.png") :png-preview))
         (fn [step]
           (is (= :png-preview (:outcome step)))
           (is (= :unsupported-media-type (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.preview store "user-1" "/blob.bin") :binary-preview))
         (fn [step]
           (is (= :binary-preview (:outcome step)))
           (is (= :unsupported-media-type (:type (ex-data (:error step))))))
         (fn [_]
           (.thumbnail store "user-1" "/pic.png"))
         (fn [^js jpeg]
           (is (= 255 (aget jpeg 0)) "JPEG magic first byte")
           (is (= 216 (aget jpeg 1)) "JPEG magic second byte")
           (settle (.thumbnail store "user-1" "/notes.md") :text-thumb))
         (fn [step]
           (is (= :text-thumb (:outcome step)))
           (is (= :unsupported-media-type (:type (ex-data (:error step))))))
         (fn [_]
           (settle (.thumbnail store "user-1" "/blob.bin") :binary-thumb))
         (fn [step]
           (is (= :binary-thumb (:outcome step)))
           (is (= :unsupported-media-type (:type (ex-data (:error step))))))])
       fixture done))))

(deftest search-finds-names-filters-types-and-pages
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.mkdir store "user-1" "/Docs"))
         (fn [_] (.save store "user-1" "/Docs/notes.md"
                        (Buffer.from "hello" "utf8")))
         (fn [_] (.save store "user-1" "/Docs/pic.png"
                        (Buffer.from "0123456789" "utf8")))
         (fn [_] (.save store "user-1" "/Docs/todo.txt"
                        (Buffer.from "tasks" "utf8")))
         ;; Substring name search is case-insensitive.
         (fn [_] (.search store "user-1" {:q "NOTE"}))
         (fn [result]
           (is (= 1 (:total result)))
           (is (= "/Docs/notes.md" (-> result :items first :path))))
         ;; Type filters map to the baseline categories.
         (fn [_] (.search store "user-1" {:q "" :type "image"}))
         (fn [result]
           (is (= 1 (:total result)))
           (is (= "pic.png" (-> result :items first :name))))
         (fn [_] (.search store "user-1" {:q "" :type "document"}))
         (fn [result]
           (is (= 2 (:total result))))
         ;; Folder scope plus paging keeps totals while slicing items.
         (fn [_] (.search store "user-1" {:q "" :folder "/Docs" :limit 1 :offset 1}))
         (fn [result]
           (is (= 3 (:total result)))
           (is (= 1 (count (:items result)))))
         ;; Misses return empty pages, not errors.
         (fn [_] (.search store "user-1" {:q "zzz-nothing"}))
         (fn [result]
           (is (= 0 (:total result)))
           (is (= [] (:items result))))])
       fixture done))))

(deftest stats-report-usage-against-quota
  (async done
    (let [fixture (open-service! {:quota-bytes 1000})
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/a.txt" (Buffer.from "12345" "utf8")))
         (fn [_] (.stats store "user-1"))
         (fn [stats]
           (is (= 1000 (:total stats)))
           (is (= 5 (:used stats))))])
       fixture done))))

(deftest diff-versions-compares-two-snapshots
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)
          first-version (atom nil)
          second-version (atom nil)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.save store "user-1" "/doc.txt"
                        (Buffer.from "line1\nline2\n" "utf8")))
         (fn [_] (.createVersion store "user-1" "/doc.txt"))
         (fn [_] (.listVersions store "user-1" "/doc.txt"))
         (fn [versions]
           (is (= 1 (count versions)))
           (reset! first-version (:id (first versions))))
         (fn [_] (.save store "user-1" "/doc.txt"
                        (Buffer.from "line1\nchanged\n" "utf8")))
         ;; The raw overwrite above auto-snapshotted the replaced
         ;; content, so the explicit version makes three.
         (fn [_] (.createVersion store "user-1" "/doc.txt"))
         (fn [_] (.listVersions store "user-1" "/doc.txt"))
         (fn [versions]
           (is (= 3 (count versions)))
           (reset! second-version (:id (first versions))))
         (fn [_] (.diffVersions store "user-1" "/doc.txt"
                                @first-version @second-version))
         (fn [diff]
           (is (str/includes? (:diff diff) "  line1"))
           (is (str/includes? (:diff diff) "- line2"))
           (is (str/includes? (:diff diff) "+ changed")))])
       fixture done))))

(deftest picker-lists-and-validates-selections
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_] (.mkdir store "user-1" "/Docs"))
         (fn [_] (.mkdir store "user-1" "/Docs/Sub"))
         (fn [_] (.save store "user-1" "/Docs/notes.md"
                        (Buffer.from "hello" "utf8")))
         (fn [_] (.save store "user-1" "/Docs/pic.png"
                        (Buffer.from "0123456789" "utf8")))
         (fn [_] (.save store "user-1" "/Docs/.hidden.txt"
                        (Buffer.from "secret" "utf8")))
         ;; Default browse lists everything except dotfiles, folders first.
         (fn [_] (.picker store "user-1" "/Docs" {}))
         (fn [items]
           (is (= 3 (count items)))
           (is (= "Sub" (:name (first items))))
           (is (true? (:isDir (first items)))))
         ;; Folders-only mode drops files.
         (fn [_] (.picker store "user-1" "/Docs" {:selection "folders"}))
         (fn [items]
           (is (= 1 (count items))))
         ;; Query narrows by name substring.
         (fn [_] (.picker store "user-1" "/Docs" {:q "PIC"}))
         (fn [items]
           (is (= 1 (count items)))
           (is (= "pic.png" (:name (first items)))))
         ;; MIME prefixes filter files by their entry type; folders stay so
         ;; a picker UI can keep navigating.
         (fn [_] (.picker store "user-1" "/Docs" {:mime-prefixes ["image/"]}))
         (fn [items]
           (is (= 2 (count items))))
         (fn [_] (.picker store "user-1" "/Docs"
                          {:selection "files" :mime-prefixes ["video/"]}))
         (fn [items]
           ;; Folders stay for navigation; no video files match.
           (is (= 1 (count items)))
           (is (= "Sub" (:name (first items)))))
         ;; Validation re-checks each selection against the mode.
         (fn [_] (.pickerValidate store "user-1"
                                  {:paths ["/Docs/notes.md"] :selection "files"}))
         (fn [items]
           (is (= 1 (count items)))
           (is (= "/Docs/notes.md" (:path (first items)))))
         ;; A folder when files are requested is rejected.
         (fn [_] (settle (.pickerValidate store "user-1"
                                          {:paths ["/Docs"] :selection "files"})
                         :type-mismatch))
         (fn [step]
           (is (= :type-mismatch (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))
         ;; Missing paths are not found; duplicates and empty lists are invalid.
         (fn [_] (settle (.pickerValidate store "user-1" {:paths ["/nope"]})
                         :missing))
         (fn [step]
           (is (= :missing (:outcome step)))
           (is (= :not-found (:type (ex-data (:error step))))))
         (fn [_] (settle (.pickerValidate store "user-1"
                                          {:paths ["/Docs/notes.md" "/Docs/notes.md"]
                                           :multiple true})
                         :duplicate))
         (fn [step]
           (is (= :duplicate (:outcome step)))
           (is (= :invalid (:type (ex-data (:error step))))))])
       fixture done))))


(deftest assistant-source-dispatch-routes-owned-shared-and-mail
  (async done
    ;; The assistant source mapping is pure routing: "owned" uses the
    ;; user's own tree, "shared" an incoming user share id, and "mail" the
    ;; read-only Maildir location. Writes never accept these sources.
    (let [calls (atom [])
          note! (fn [name] (fn [& _] (swap! calls conj name)
                             (js/Promise.resolve #js {:isDir false})))
          store #js {:list (note! "owned")
                     :listShared (note! "shared")
                     :listMail (note! "mail")
                     :stat (note! "owned")
                     :statShared (note! "shared")
                     :statMail (note! "mail")
                     :readFile (fn [& _] (js/Promise.resolve (Buffer.from "owned" "utf8")))
                     :readShared (fn [& _] (js/Promise.resolve (Buffer.from "shared" "utf8")))
                     :readMail (fn [& _] (js/Promise.resolve (Buffer.from "mail" "utf8")))}
          user #js {:id "user-1"}]
      (-> (.then (js/Promise.all
                  #js [(files-assistant/list-source-dispatch! store user "owned" nil "/")
                       (files-assistant/list-source-dispatch! store user "shared" "share-1" "/")
                       (files-assistant/list-source-dispatch! store user "mail" nil "/")])
                 (fn [_]
                   (is (= ["owned" "shared" "mail"] @calls))
                   (reset! calls [])
                   (js/Promise.all
                    #js [(files-assistant/read-source-dispatch! store user "owned" nil "/a.md")
                         (files-assistant/read-source-dispatch! store user "shared" "share-1" "/a.md")
                         (files-assistant/read-source-dispatch! store user "mail" nil "/a.md")])))
            (.then (fn [reads]
                     (is (= ["owned" "shared" "mail"] @calls))
                     (is (= "owned" (.toString (nth (nth reads 0) 1) "utf8")))
                     (is (= "shared" (.toString (nth (nth reads 1) 1) "utf8")))
                     (is (= "mail" (.toString (nth (nth reads 2) 1) "utf8")))))
            (.then (fn [_] (done)))
            (.catch (fn [error]
                      (is nil error)
                      (done)))))))
