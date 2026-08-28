(ns dar.platform.database-test
  (:require [cljs.test :refer [async deftest is testing]]
            [dar.platform.database :as database]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(defn- with-database [test-fn]
  (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-db-test-"))
        connection (database/open! (.resolve path directory "dar.sqlite"))]
    (try
      (test-fn connection)
      (finally
        (.close connection)
        (.rmSync fs directory #js {:recursive true :force true})))))

(defn- query-value [^js connection sql column]
  (let [^js statement (.prepare connection sql)
        row (.get statement)]
    (aget row column)))

(deftest applies-numbered-migrations-and-platform-pragmas
  (with-database
    (fn [connection]
      (is (= 1 (query-value connection
                            "SELECT version FROM dar_migrations"
                            "version")))
      (is (= 1 (query-value connection "PRAGMA foreign_keys" "foreign_keys")))
      (is (= "wal" (query-value connection "PRAGMA journal_mode" "journal_mode")))
      (is (= 14 (query-value connection
                             "SELECT count(*) AS count FROM sqlite_schema WHERE type = 'table'"
                             "count"))))))

(deftest serializes-writes-and-survives-a-failed-operation
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-db-queue-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          ^js queue (database/writer-queue connection)
          order (atom [])]
      (-> (.write queue (fn [_]
                          (swap! order conj :first)
                          (throw (js/Error. "expected"))))
          (.catch (fn [_]
                    (.write queue (fn [_]
                                    (swap! order conj :second)
                                    :ok))))
          (.then (fn [result]
                   (is (= :ok result))
                   (is (= [:first :second] @order))))
          (.catch (fn [error]
                    (is false (str "writer queue failed: " (.-message error)))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))

(deftest rejects-asynchronous-writer-operations
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-db-async-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          ^js queue (database/writer-queue connection)]
      (-> (.write queue (fn [_] (js/Promise.resolve :too-late)))
          (.then (fn [_]
                   (is false "an asynchronous writer operation must be rejected")))
          (.catch (fn [error]
                    (is (= :asynchronous-database-write (:type (ex-data error))))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))
