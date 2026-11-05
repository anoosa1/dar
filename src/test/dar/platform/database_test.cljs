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
      (is (= 35 (query-value connection
                            "SELECT max(version) AS version FROM dar_migrations"
                            "version")))
      (is (= 1 (query-value connection "PRAGMA foreign_keys" "foreign_keys")))
      (is (= "wal" (query-value connection "PRAGMA journal_mode" "journal_mode")))
      (is (= 57 (query-value connection
                             "SELECT count(*) AS count FROM sqlite_schema WHERE type = 'table'"
                             "count")))
      (is (= 1 (query-value connection
                            "SELECT count(*) AS count FROM pragma_table_info('files_locks') WHERE name = 'token'"
                            "count")))
      (is (= 1 (query-value connection
                            "SELECT count(*) AS count FROM pragma_table_info('files_locks') WHERE name = 'expires_at'"
                            "count"))))))

(deftest widget-identity-upgrade-preserves-custom-layouts
  (with-database
    (fn [connection]
      (.run (.prepare connection "INSERT INTO users(id,subject,display_name,administrator,created_at,updated_at) VALUES ('owner','owner','Owner',0,'2026-10-04','2026-10-04')"))
      (let [layout {:layout [{:id "prayer.today" :x 6 :y 0 :size "wide"}
                            {:id "notes.recent" :x 0 :y 7 :size "compact"}
                            {:id "todo.tasks" :x 3 :y 7 :size "large"}
                            {:id "files.favorites" :x 0 :y 15 :size "standard"}]
                    :activeId "notes.recent"}
            expected (-> layout
                         (assoc-in [:layout 0 :id] "prayer.times")
                         (assoc-in [:layout 1 :id] "notes.list")
                         (assoc-in [:layout 2 :id] "todo.upcoming")
                         (assoc :activeId "notes.list"))]
        (.run (.prepare connection "INSERT INTO preferences(user_id,namespace,key,value_json,revision,updated_at) VALUES ('owner','dashboard','layout',?,7,'2026-10-04')")
              (js/JSON.stringify (clj->js layout)))
        (.run (.prepare connection "INSERT INTO preferences(user_id,namespace,key,value_json,revision,updated_at) VALUES ('owner','todo','settings',?,3,'2026-10-04')") "{\"notes.recent\":true}")
        (.exec connection "DELETE FROM dar_migrations WHERE version=25;")
        (database/migrate! connection)
        (let [row (.get (.prepare connection "SELECT * FROM preferences WHERE namespace='dashboard'"))]
          (is (= expected (js->clj (js/JSON.parse (.-value_json row)) :keywordize-keys true)))
          (is (= 8 (.-revision row))))
        (is (= "{\"notes.recent\":true}" (.-value_json (.get (.prepare connection "SELECT * FROM preferences WHERE namespace='todo'")))))
        (database/migrate! connection)
        (is (= 8 (.-revision (.get (.prepare connection "SELECT * FROM preferences WHERE namespace='dashboard'")))))))))

(deftest unsupported-database-migrations-are-refused-without-changing-data
  (with-database
    (fn [^js connection]
      (.run (.prepare connection "INSERT INTO dar_migrations(version, applied_at) VALUES (99, 'future')"))
      (let [before (.all (.prepare connection "SELECT version, applied_at FROM dar_migrations ORDER BY version"))]
        (try
          (database/migrate! connection)
          (is false "A future migration must prevent startup.")
          (catch :default error
            (is (= :unsupported-database (:type (ex-data error))))
            (is (= [99] (:versions (ex-data error))))))
        (is (= (js/JSON.stringify before)
               (js/JSON.stringify (.all (.prepare connection "SELECT version, applied_at FROM dar_migrations ORDER BY version")))))))))

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
