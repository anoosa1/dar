(ns dar.platform.atomic-files-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.atomic-files :as atomic-files]
            [dar.platform.database :as database]
            ["node:buffer" :refer [Buffer]]
            ["node:fs" :as fs]
            ["node:fs/promises" :as fsp]
            ["node:os" :as os]
            ["node:path" :as path]))

(defn- open-store! []
  (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-atomic-files-test-"))
        connection (database/open! (.resolve path directory "dar.sqlite"))]
    {:directory directory
     :connection connection
     :store (atomic-files/service #js {:connection connection
                                       :writer (database/writer-queue connection)})}))

(defn- close-store! [fixture]
  (let [^js connection (:connection fixture)]
    (.close connection))
  (.rmSync fs (:directory fixture) #js {:recursive true :force true}))

(defn- finish
  "Ends an async test exactly once, closing the store first on either path."
  [start fixture done]
  (-> start
      (.then (fn [_]
               (close-store! fixture)
               (done)))
      (.catch (fn [error]
                (is false (str "flow failed: " (.-message error)))
                (close-store! fixture)
                (done)))))

(defn- prepare [fixture sql]
  (let [^js connection (:connection fixture)]
    (.prepare connection sql)))

(defn- journal-states [fixture]
  (let [^js rows (.all (prepare fixture "SELECT operation, state FROM mutation_journal"))]
    (into {} (map (fn [^js row] [(.-operation row) (.-state row)])) rows)))

(defn- absent? [target]
  (nil? (try (.lstatSync fs target)
             (catch :default _ nil))))

(defn- write-step [store root name contents]
  (fn []
    (atomic-files/write-file! store root name contents)))

(deftest rejects-invalid-journal-entries-without-touching-the-database
  (let [fixture (open-store!)
        ^js store (:store fixture)]
    (try
      (is (thrown-with-msg? js/Error #"operation and payload"
                            ((.-journal store) {:payload {:a 1}})))
      (is (thrown-with-msg? js/Error #"operation and payload"
                            ((.-journal store) {:operation "x"})))
      (finally
        (close-store! fixture)))))

(deftest per-user-queue-requires-a-string-scope
  (let [serialize (atomic-files/per-user-queue)]
    (is (thrown-with-msg? js/Error #"user scope"
                          (serialize nil (fn [] (js/Promise.resolve nil)))))))

(deftest writes-files-atomically-and-closes-the-journal
  (async done
    (let [fixture (open-store!)
          root (.join path (:directory fixture) "storage")]
      (.mkdirSync fs root #js {:recursive true})
      (finish (-> (js/Promise.resolve nil)
                  (.then (write-step (:store fixture) root "notes.md" (.from Buffer "hello")))
                  (.then (fn [_]
                          (let [target (.join path root "notes.md")]
                            (is (= "hello" (.readFileSync fs target "utf8")))
                            (is (= 1 (.-length (.readdirSync fs root))))
                            (is (= "reconciled"
                                   ((journal-states fixture) "dar.atomic-write")))))))
              fixture done))))

(deftest failing-writes-create-no-directories-or-temporaries
  (async done
    (let [fixture (open-store!)
          root (.join path (:directory fixture) "storage")
          ^js store (:store fixture)]
      (.mkdirSync fs root #js {:recursive true})
      (finish (-> (js/Promise.resolve nil)
                  (.then (write-step store root "missing-dir/notes.md" (.from Buffer "hello")))
                  (.then (fn [_]
                          (is false "expected the write to fail")))
                  (.catch (fn [error]
                          (is (= "ENOENT" (.-code error)))
                          (is (absent? (.join path root "missing-dir"))))))
              fixture done))))
(defn- journal-atomic-write [^js store root path-name temp-name size]
  (fn [_]
    ((.-journal store)
     {:operation "dar.atomic-write"
      :user-id "user-1"
      :payload {:root root :path path-name :temp temp-name :size size}})))

(deftest reconciliation-completes-interrupted-writes-from-disk-state
  (async done
    (let [fixture (open-store!)
          root (.join path (:directory fixture) "storage")
          ^js store (:store fixture)
          leftover (.join path root ".dar-write-leftover")
          phantom (.join path root ".dar-write-phantom")
          rolled-back (.join path root "rolled-back.md")
          landed (.join path root "landed.md")]
      (.mkdirSync fs root #js {:recursive true})
      (finish (-> (.writeFile fsp leftover (.from Buffer "partial"))
                  (.then (journal-atomic-write store root "rolled-back.md" leftover 7))
                  (.then (fn [_]
                          (.writeFile fsp landed (.from Buffer "complete"))))
                  (.then (journal-atomic-write store root "landed.md" phantom 8))
                  (.then (fn [_] ((.-reconcile store))))
                  (.then (fn [_]
                          (is (absent? leftover))
                          (is (absent? rolled-back))
                          (is (= "complete" (.readFileSync fs landed "utf8")))
                          (is (= {"dar.atomic-write" "reconciled"}
                                 (journal-states fixture))))))
              fixture done))))

(defn- timed-operation [events label delay]
  (fn []
    (swap! events conj (str label "-start"))
    (js/Promise.
     (fn [resolve]
       (js/setTimeout
        (fn []
          (swap! events conj (str label "-end"))
          (resolve label))
        delay)))))

(deftest per-user-queues-serialize-operations
  (async done
    (let [serialize (atomic-files/per-user-queue)
          events (atom [])
          all (js/Promise.all
               #js [(serialize "user-1" (timed-operation events "a" 30))
                    (serialize "user-1" (timed-operation events "b" 5))
                    (serialize "user-2" (timed-operation events "c" 10))])]
      (-> all
          (.then (fn [_]
                  ;; The invariant is per-user ordering, not global order:
                  ;; user-2's operation may interleave freely.
                  (let [position (zipmap @events (range))]
                    (is (< (position "a-start") (position "a-end")
                           (position "b-start") (position "b-end")))
                    (is (< (position "c-start") (position "c-end")))
                    (is (= 6 (count @events))))))
          (.then (fn [_] (done)))
          (.catch (fn [error]
                   (is false (str "queue flow failed: " (.-message error)))
                   (done)))))))
