(ns dar.apps.files-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.apps.files :as files]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.database :as database]
            ["node:buffer" :refer [Buffer]]
            ["node:fs" :as fs]
            ["node:fs/promises" :as fsp]
            ["node:os" :as os]
            ["node:path" :as path]))

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
                            database-service atomic-store)})))

(defn- seed-user! [fixture]
  (.run (.prepare ^js (:connection fixture)
                  "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
        "user-1" "files-subject" "Files" "files@example.test"
        (.toISOString (js/Date.)) (.toISOString (js/Date.))))

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
