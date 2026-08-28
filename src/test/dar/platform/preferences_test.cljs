(ns dar.platform.preferences-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.database :as database]
            [dar.platform.preferences :as preferences]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest stores-and-revises-user-scoped-preferences
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-preferences-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (preferences/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'),
              ('user-2', 'subject-2', 'User Two', 'two@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.set service "user-1" "calendar" "view" {:layout "week"} nil)
          (.then (fn [created]
                   (is (= 1 (:revision created)))
                   (.set service "user-1" "calendar" "view"
                         {:layout "month"} (:revision created))))
          (.then (fn [updated]
                   (is (= 2 (:revision updated)))
                   (is (= {:layout "month"}
                          (:value (.get service "user-1" "calendar" "view"))))
                   (is (= [] (.list service "user-2" "calendar")))
                   (is (= ["view"] (mapv :key (.list service "user-1" "calendar"))))))
          (.then #(.delete service "user-1" "calendar" "view" 2))
          (.then (fn [changes]
                   (is (= 1 changes))
                   (is (nil? (.get service "user-1" "calendar" "view")))))
          (.catch (fn [error]
                    (is false (str error))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))

(deftest requires-if-match-for-updates-and-deletes
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-preferences-conflict-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (preferences/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.set service "user-1" "calendar" "view" {:layout "week"} nil)
          (.then (fn [_created]
                   (-> (.set service "user-1" "calendar" "view"
                             {:layout "month"} nil)
                       (.then (fn [_]
                                (is false "an existing preference requires If-Match")))
                       (.catch (fn [error]
                                 (is (= :preference-conflict
                                        (:type (ex-data error)))))))))
          (.then (fn [_]
                   (.set service "user-1" "calendar" "view" {:layout "month"} 1)))
          (.then (fn [updated]
                   (is (= 2 (:revision updated)))
                   (-> (.delete service "user-1" "calendar" "view" 1)
                       (.then (fn [_]
                                (is false "delete requires the current revision")))
                       (.catch (fn [error]
                                 (is (= :preference-conflict
                                        (:type (ex-data error)))))))))
          (.catch (fn [error]
                    (is false (str error))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))
