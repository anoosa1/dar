(ns dar.apps.files-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.apps.files :as files]
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
  ([{:keys [quota-bytes]}]
   (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-files-app-test-"))
         connection (database/open! (.resolve path directory "dar.sqlite"))
         database-service #js {:connection connection
                               :writer (database/writer-queue connection)}
         atomic-store (atomic-files/service database-service)
         data-root directory]
     {:directory directory
      :connection connection
      :data-root data-root
      :store (files/service data-root {:quota-bytes quota-bytes}
                            database-service atomic-store
                            (rate-limiter/service (fn [] (.now js/Date)))
                            (keyring/keyring
                             [{:id "test-key"
                               :material (.randomBytes crypto 32)}]))})))

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
           (is (false? (:favorited (first entries)))))])
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

(deftest quota-exceeded-rejects-writes-beyond-the-limit
  (async done
    (let [fixture (open-service! {:quota-bytes 10})
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

(deftest editor-save-rename-creates-snapshot-and-restore-rolls-back
  (async done
    (let [fixture (open-service! {:quota-bytes 40})
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
           ;; The rename lands first; the oversized write then fails and the
           ;; catch must restore the original name and content.
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
           ;; Both changed saves snapshotted the content they replaced.
           (is (= 2 (count versions)))
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
           (is (= 3 (count versions))))])
       fixture done))))

(deftest explicit-locks-block-mutations-until-unlocked
  (async done
    (let [fixture (open-service!)
          ^js store (:store fixture)]
      (seed-user! fixture)
      (finish
       (run-steps
        [(fn [_]
           (.save store "user-1" "/locked.md" (Buffer.from "shared doc" "utf8")))
         (fn [_]
           (.lock store "user-1" "/locked.md"))
         (fn [result]
           (is (= true (:locked result)))
           (is (= "user-1" (:owner result))))
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
           (.unlock store "user-1" "/locked.md"))
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
           (is (= "write" (:permission user-share)))
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
