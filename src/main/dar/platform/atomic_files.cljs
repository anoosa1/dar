(ns dar.platform.atomic-files
  (:require [dar.cordis :as cordis]
            [dar.platform.path :as path]
            ["node:crypto" :refer [randomBytes randomUUID]]
            ["node:fs/promises" :as fs]
            ["node:path" :as node-path]))

;; Atomic durable file writes and the journal that makes the filesystem and
;; SQLite boundary recoverable. Every write follows the safety invariant:
;; sibling temporary file, flush, close, rename, directory sync. Callers own
;; path authorization; this module owns confinement, atomicity, and recovery.

(def temp-prefix ".dar-write-")

(defn- sync-directory! [directory]
  (-> (.open fs directory "r")
      (.then (fn [^js handle]
               (-> (.sync handle)
                   (.finally #(.close handle)))))))

(defn- remove-quietly! [target]
  (-> (.rm fs target #js {:force true})
      (.catch (fn [_] nil))))

(defn- temp-path [target]
  (.join node-path (.dirname node-path target)
         (str temp-prefix (.toString (randomBytes 12) "hex"))))

(defn write-file!
  "Atomically writes a Buffer to root + user-path and returns the target path.

  The parent directory must already exist. The write is journaled as
  dar.atomic-write so a crash around the rename is reconciled at startup."
  [^js store root user-path-value ^js bytes]
  (let [target (.resolve node-path root (path/normalize-user-path user-path-value))
        temporary (temp-path target)]
    (-> (path/verify-no-symlinks! root user-path-value)
        (.then (fn [_]
                 ((.-journal store)
                  {:operation "dar.atomic-write"
                   :payload {:root root
                             :path user-path-value
                             :temp temporary
                             :size (.-length bytes)}})))
        (.then (fn [entry]
                 (-> (.open fs temporary "wx" 384) ;; 0o600, owner read/write only
                     (.then (fn [^js handle]
                              (-> (.writeFile handle bytes)
                                  (.then #(.sync handle))
                                  (.finally #(.close handle)))))
                     (.then #(.rename fs temporary target))
                     (.then #(sync-directory! (.dirname node-path target)))
                     (.then (fn [_] ((.-markReconciled store) (:id entry))))
                     (.then (fn [_] target))
                     (.catch (fn [error]
                               (-> (remove-quietly! temporary)
                                   (.then (fn [] (throw error))))))))))))

(defn- stat-interruption
  "Decides between rolled-forward and unknown from the destination metadata."
  [target expected-size]
  (-> (.stat fs target)
      (.then (fn [^js stat]
               (if (= expected-size (.-size stat))
                 :renamed
                 :unknown-destination)))
      (.catch (fn [_] :rolled-back))))

(defn- classify-interruption
  "A leftover temporary file means the rename never happened. A missing
  temporary means the rename may have landed and the destination decides."
  [temporary target expected-size]
  (-> (.lstat fs temporary)
      (.then (fn [_] :leftover-temp))
      (.catch (fn [error]
                (if (= "ENOENT" (.-code error))
                  (stat-interruption target expected-size)
                  (throw error))))))

(defn- reconcile-atomic-write!
  "Decides the outcome of an interrupted dar.atomic-write from disk state.

  A leftover temporary file means the rename never happened: roll back. A
  destination whose size matches the payload means the rename landed: roll
  forward. Any other state rolled back by definition."
  [payload]
  (let [root (:root payload)
        temporary (:temp payload)
        expected-size (:size payload)
        target (try
                 (.resolve node-path root (path/normalize-user-path (:path payload)))
                 (catch :default _ nil))]
    (if (nil? target)
      (js/Promise.resolve :rolled-back)
      (-> (classify-interruption temporary target expected-size)
          (.then (fn [outcome]
                   (if (= :leftover-temp outcome)
                     (-> (remove-quietly! temporary)
                         (.then (fn [] :rolled-back)))
                     outcome)))))))

(defn per-user-queue
  "Serializes asynchronous operations per user so filesystem and database
  boundaries never interleave within one user's storage."
  []
  (let [tails (js/Map.)]
    (fn [user-id operation]
      (when-not (string? user-id)
        (throw (ex-info "A user scope is required for serialized operations."
                        {:type :invalid-operation-scope})))
      (let [^js tail (.get tails user-id)
            run (fn [_] (operation))
            chained (if (some? tail) (.then tail run) (run nil))]
        (.set tails user-id (.catch chained (fn [_] nil)))
        chained))))

(defn- reconcile-pending!
  "Replays every prepared or committed journal row through its registered
  reconciler, then closes the row. Unknown operations stay pending."
  [^js connection ^js writer mark-reconciled reconcilers]
  (let [select-pending
        (.prepare connection
                  "SELECT id, user_id, operation, payload_json, state FROM mutation_journal WHERE state IN ('prepared', 'committed') ORDER BY created_at, id LIMIT 500")
        rows (vec (.all select-pending))]
    (reduce
     (fn [prior ^js row]
       (.then prior
              (fn [_]
                (let [handler (.get reconcilers (.-operation row))]
                  (if (nil? handler)
                    (js/Promise.resolve :unhandled)
                    (-> (handler (js->clj (js/JSON.parse (.-payload_json row))
                                         :keywordize-keys true))
                        (.then (fn [_] (mark-reconciled (.-id row))))))))))
     (js/Promise.resolve nil)
     rows)))

(defn service
  "Builds the atomic-files service over one database service."
  [^js database-service]
  (let [^js connection (.-connection database-service)
        ^js writer (.-writer database-service)
        insert
        (.prepare connection
                  "INSERT INTO mutation_journal(id, user_id, operation, payload_json, state, created_at, updated_at) VALUES (?, ?, ?, ?, 'prepared', ?, ?)")
        mark-committed
        (.prepare connection
                  "UPDATE mutation_journal SET state = 'committed', updated_at = ? WHERE id = ?")
        mark-reconciled
        (.prepare connection
                  "UPDATE mutation_journal SET state = 'reconciled', updated_at = ? WHERE id = ?")
        reconcilers (js/Map.)
        serialize (per-user-queue)
        close-entry!
        (fn [entry-id]
          (.write writer
                  (fn [_]
                    (.run mark-reconciled (.toISOString (js/Date.)) entry-id)
                    {:id entry-id :state "reconciled"})))]
    (.set reconcilers "dar.atomic-write" reconcile-atomic-write!)
    #js {:registerReconciler
         (fn [operation handler]
           (.set reconcilers operation handler))
         :journal
         (fn [{:keys [operation payload user-id]}]
           (when-not (and (string? operation) (some? payload))
             (throw (ex-info "A journal entry needs an operation and payload."
                             {:type :invalid-journal-entry})))
           (let [id (randomUUID)
                 now (.toISOString (js/Date.))]
             (.write writer
                     (fn [_]
                       (.run insert id user-id operation
                             (js/JSON.stringify (clj->js payload)) now now)
                       {:id id :state "prepared"}))))
         :commit
         (fn [entry-id]
           (.write writer
                   (fn [_]
                     (.run mark-committed (.toISOString (js/Date.)) entry-id)
                     {:id entry-id :state "committed"})))
         :markReconciled close-entry!
         :reconcile
         (fn []
           (reconcile-pending! connection writer close-entry! reconcilers))
         :serialize serialize}))

(def atomic-files-plugin
  (cordis/plugin
   {:name "dar-atomic-files"
    :inject ["database"]
    :apply
    (fn [^js ctx _config]
      (let [^js store (service (.-database ctx))]
        (-> (.reconcile store)
            (.then (fn [_]
                     (cordis/provide! ctx "atomicFiles" store)
                     nil)))))}))
