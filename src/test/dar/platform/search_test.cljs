(ns dar.platform.search-test
  (:require [cljs.test :refer [async deftest is]]
            [dar.platform.database :as database]
            [dar.platform.search :as search]
            ["node:fs" :as fs]
            ["node:os" :as os]
            ["node:path" :as path]))

(deftest indexes-updates-queries-and-deletes-scoped-documents
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-search-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (search/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES
              ('user-1', 'subject-1', 'User One', 'one@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'),
              ('user-2', 'subject-2', 'User Two', 'two@example.test', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.index service "user-1" "files" "file-1"
                  "Quarterly report" "Revenue and planning" "/files/quarter.pdf")
          (.then (fn [created]
                   (is (= 1 (:generation created)))
                   (.index service "user-1" "files" "file-1"
                           "Quarterly report" "Revenue and planning changed"
                           "/files/quarter.pdf")))
          (.then (fn [updated]
                   (is (= 2 (:generation updated)))
                   (.index service "user-2" "files" "file-1"
                           "Private report" "Another user's document" "/files/private.pdf")))
          (.then (fn [_]
                   (is (= 1 (count (.search service "user-1" "quarter"))))
                   (is (= 1 (count (.search service "user-2" "private"))))
                   (is (= 0 (count (.search service "user-1" "private"))))
                   (.delete service "user-1" "files" "file-1")))
          (.then (fn [_]
                   (is (= 0 (count (.search service "user-1" "quarter"))))))
          (.catch (fn [error]
                    (is false (str error))))
          (.finally (fn []
                      (.close connection)
                      (.rmSync fs directory #js {:recursive true :force true})
                      (done)))))))

(deftest reconciles-one-provider-generation-atomically
  (async done
    (let [directory (.mkdtempSync fs (.resolve path (.tmpdir os) "dar-search-reconcile-test-"))
          connection (database/open! (.resolve path directory "dar.sqlite"))
          service (search/service
                   #js {:connection connection
                        :writer (database/writer-queue connection)})
          cleanup! (fn []
                     (.close connection)
                     (.rmSync fs directory #js {:recursive true :force true})
                     (done))]
      (.exec connection
             "INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at)
              VALUES ('user-1', 'subject-1', 'User One', 'one@example.test', 0,
                      '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')")
      (-> (.index service "user-1" "files" "old"
                  "Old file" "old body" "/files?open=old")
          (.then (fn [_]
                   (.reconcileProvider
                    service "user-1" "files"
                    [{:id "one" :title "One" :body "first body"
                      :url "/files?open=one"}
                     {:id "two" :title "Two" :body "second body"
                      :url "/files?open=two"}])))
          (.then (fn [result]
                   (is (= 2 (:generation result)))
                   (is (= 2 (:count result)))
                   (is (empty? (.search service "user-1" "Old file")))
                   (is (= 1 (count (.search service "user-1" "second body"))))
                   (.reconcileProvider
                    service "user-1" "files"
                    [{:id "duplicate" :title "A" :body "a" :url "/a"}
                     {:id "duplicate" :title "B" :body "b" :url "/b"}])))
          (.then (fn [_]
                   (is false "duplicate provider documents must fail")))
          (.catch (fn [_]
                    ;; The failed generation rolled back its delete and inserts.
                    (is (= 1 (count (.search service "user-1" "second body"))))
                    (is (empty? (.search service "user-1" "duplicate")))
                    (.reconcileProvider service "user-1" "files" [])))
          (.then (fn [empty-result]
                   (is (= 3 (:generation empty-result)))
                   (is (empty? (.search service "user-1" "second body")))
                   (.reconcileProvider
                    service "user-1" "files"
                    [{:id "after-empty" :title "After" :body "again"
                      :url "/files?open=after-empty"}])))
          (.then (fn [result]
                   (is (= 4 (:generation result)))
                   (.index service "user-1" "files" "share:grant:/note.md"
                           "Shared orchid" "permission projected" "/files?view=shares")))
          (.then (fn [_]
                   (.reconcileProvider service "user-1" "files"
                                       [{:id "owned" :title "Owned" :body "fresh" :url "/files"}]
                                       "share:")))
          (.then (fn [_]
                   (is (= 1 (count (.search service "user-1" "Shared orchid"))))
                   (is (empty? (.search service "user-1" "again")))
                   (is (= 1 (count (.search service "user-1" "fresh"))))))
          (.finally cleanup!)))))
